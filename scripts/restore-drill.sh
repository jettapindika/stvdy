#!/usr/bin/env bash
#
# The restore drill. Restores the newest dump into a scratch database, checks the schema it
# restored to, and compares the `+sync` tables' row counts against the live database.
#
#   scripts/restore-drill.sh --dump /var/backups/stvdy --target-url postgres://…/stvdy_drill
#   scripts/restore-drill.sh --dump latest.dump --target-url … --source-url "$DATABASE_URL"
#
# **A backup that has never been restored is a hope** (A347). Everything else in the backup
# section of `docs/OPERATIONS.md` is a statement about a mechanism; this script is the only
# thing that turns it into evidence, which is why the drill is run before launch and quarterly
# after, and why its output is pasted into that section with a date.
#
# What it establishes, in order:
#
#   1. the newest dump exists and is not empty — a 0-byte dump restores to a valid, empty
#      database, and "it restored" is true of that;
#   2. it restores at all, into a database this script creates and (by default) drops again;
#   3. `sqlx migrate info` reports **every** migration installed and **none** pending or
#      missing — a dump taken mid-migration, or restored by an older `sqlx`, is caught here
#      rather than by a query that fails in production;
#   4. the restored database is not merely a schema: it holds rows, and each of the sixteen
#      `+sync` tables is non-empty unless the source's own count is zero. This is the
#      assertion that separates "the schema restored" from "the data restored";
#   5. when `--source-url` is given, each `+sync` table's count is compared with the live
#      database's, within `--tolerance` (default 0).
#
# **The tolerance is the honest part of the design.** Production keeps writing while the
# restore runs, so exact equality between a dump and the live database is a claim a busy
# system cannot make. The default is 0 because the drill's first run is against a quiet
# database and equality is the stronger claim; a deployment that cannot stop writes passes
# `--tolerance`, and the number it passes is the drift it is willing to call acceptable. What
# the script never does is report equality it did not observe.
#
# Fail-closed: a missing tool, an unreadable dump or a query that errors is a failure with a
# message naming what could not be checked — never a pass. The one exception is exit 2, which
# means "the drill could not run at all" and is deliberately distinct from exit 1 ("the drill
# ran and the restore is not good"), because the two call for different responses.
#
# It needs `psql` (and `pg_restore` for a custom-format dump), which the production host has
# and a laptop may not; `sqlx` for the migration check. It never touches the source database:
# `--source-url` is read with `select count(*)` and nothing else.
set -euo pipefail

EXIT_FAIL=1
EXIT_CANNOT_RUN=2

usage() {
  awk 'NR > 1 { if ($0 !~ /^#/) exit; sub(/^# ?/, ""); print }' "$0"
}

cannot_run() {
  echo "restore-drill: $1" >&2
  exit "$EXIT_CANNOT_RUN"
}

dump_arg=""
target_url="${TARGET_URL:-}"
source_url="${SOURCE_URL:-}"
tolerance=0
keep=0
scratch_db=""
drop_first=1

while [ $# -gt 0 ]; do
  case "$1" in
    --dump)
      shift
      [ $# -gt 0 ] || cannot_run "--dump needs a file or a directory"
      dump_arg="$1"
      ;;
    --target-url)
      shift
      [ $# -gt 0 ] || cannot_run "--target-url needs a postgres:// URL"
      target_url="$1"
      ;;
    --source-url)
      shift
      [ $# -gt 0 ] || cannot_run "--source-url needs a postgres:// URL"
      source_url="$1"
      ;;
    --tolerance)
      shift
      [ $# -gt 0 ] || cannot_run "--tolerance needs a number"
      tolerance="$1"
      ;;
    --keep)
      keep=1
      ;;
    --no-drop)
      drop_first=0
      ;;
    --help | -h)
      usage
      exit 0
      ;;
    *)
      cannot_run "unknown argument $1 (run with --help)"
      ;;
  esac
  shift
done

[ -n "$dump_arg" ] || cannot_run "--dump is required: the drill restores a real backup, not a fixture"
[ -n "$target_url" ] || cannot_run "--target-url is required: name a scratch database, never production"
case "$tolerance" in
  '' | *[!0-9]*) cannot_run "--tolerance must be a whole number, got '$tolerance'" ;;
esac

# The target must not be the source. Restoring a dump over production is the one mistake this
# script must be structurally incapable of, and comparing the strings is enough to catch the
# case where someone pastes the same URL twice.
if [ -n "$source_url" ] && [ "$source_url" = "$target_url" ]; then
  cannot_run "--source-url and --target-url are the same database; the drill restores into a scratch database"
fi

for tool in psql sqlx; do
  command -v "$tool" >/dev/null 2>&1 ||
    cannot_run "$tool is not on PATH, so the drill cannot run (this is a 2, not a pass)"
done

# ---------------------------------------------------------------------------
# The newest dump, and whether it is one.
# ---------------------------------------------------------------------------
dump=""
if [ -d "$dump_arg" ]; then
  # Newest by modification time. `find` rather than `ls -t` so a filename with a newline
  # cannot smuggle a second entry in, and `head -1` after the sort so the choice is the
  # script's rather than the filesystem's.
  dump="$(find "$dump_arg" -maxdepth 1 -type f \( -name '*.dump' -o -name '*.sql' -o -name '*.tar' -o -name '*.gz' \) -print 2>/dev/null |
    while IFS= read -r candidate; do
      printf '%s\t%s\n' "$(stat -f '%m' "$candidate" 2>/dev/null || stat -c '%Y' "$candidate")" "$candidate"
    done | sort -rn | head -1 | cut -f2-)"
  [ -n "$dump" ] || {
    echo "FAIL no dump found in $dump_arg" >&2
    echo "      (looked for *.dump, *.sql, *.tar, *.gz — an empty backup directory is a finding)"
    exit "$EXIT_FAIL"
  }
  echo "newest dump: $dump ($(date -r "$dump" '+%Y-%m-%d %H:%M:%S' 2>/dev/null || echo 'unknown mtime'))"
else
  dump="$dump_arg"
  [ -f "$dump" ] || {
    echo "FAIL $dump does not exist" >&2
    exit "$EXIT_FAIL"
  }
fi

size="$(wc -c <"$dump" | tr -d ' ')"
if [ "$size" -eq 0 ]; then
  echo "FAIL $dump is 0 bytes" >&2
  echo "      a 0-byte dump restores to a valid, empty database, so \"it restored\" would be true of it"
  exit "$EXIT_FAIL"
fi

# The format decides the tool. `PGDMP` is pg_dump's custom format; a tar archive has `toc.dat`
# at its start; anything else is treated as plain SQL and fed to psql, which is what a
# `pg_dump --format=plain` (or a `gzip`ped one) needs.
restore_tool="psql"
case "$(head -c 5 "$dump" | tr -d '\0')" in
  PGDMP) restore_tool="pg_restore" ;;
esac
if [ "$restore_tool" = "pg_restore" ]; then
  command -v pg_restore >/dev/null 2>&1 ||
    cannot_run "the dump is in pg_dump's custom format and pg_restore is not on PATH"
fi
if [ "${dump##*.}" = "gz" ]; then
  command -v gunzip >/dev/null 2>&1 || cannot_run "the dump is gzipped and gunzip is not on PATH"
  restore_tool="gunzip+psql"
fi
echo "format:     $restore_tool ($size bytes)"

# ---------------------------------------------------------------------------
# The scratch database.
# ---------------------------------------------------------------------------
# `psql` is pointed at the *maintenance* database on the target server, because the scratch
# database does not exist yet. Derived from the target URL rather than configured separately:
# two settings that must agree are two settings that can disagree.
maintenance_url="${target_url%/*}/postgres"
scratch_db="${target_url##*/}"
scratch_db="${scratch_db%%\?*}"

cleanup() {
  if [ "$keep" = "0" ]; then
    psql "$maintenance_url" -q -c "drop database if exists \"$scratch_db\" with (force)" >/dev/null 2>&1 || true
  else
    echo "kept:       $scratch_db (--keep)"
  fi
}
trap cleanup EXIT

if [ "$drop_first" = "1" ]; then
  psql "$maintenance_url" -q -c "drop database if exists \"$scratch_db\" with (force)" >/dev/null 2>&1 || true
fi
if ! psql "$maintenance_url" -q -c "create database \"$scratch_db\"" >/dev/null 2>&1; then
  echo "FAIL could not create the scratch database $scratch_db on $maintenance_url" >&2
  exit "$EXIT_FAIL"
fi

echo
echo "=== restoring $dump into $scratch_db"

# `--no-owner --no-privileges` on pg_restore: a dump taken by a superuser names roles the
# scratch server may not have, and an ownership failure is not a data failure. `ON_ERROR_STOP`
# on psql for the plain path, so a SQL error stops rather than scrolling past.
restore_ok=1
case "$restore_tool" in
  pg_restore)
    pg_restore --no-owner --no-privileges --exit-on-error --single-transaction \
      -d "$target_url" "$dump" >/dev/null 2>&1 || restore_ok=0
    ;;
  gunzip+psql)
    gunzip -c "$dump" | psql -v ON_ERROR_STOP=1 -q -d "$target_url" >/dev/null 2>&1 || restore_ok=0
    ;;
  *)
    psql -v ON_ERROR_STOP=1 -q -d "$target_url" -f "$dump" >/dev/null 2>&1 || restore_ok=0
    ;;
esac
if [ "$restore_ok" != "1" ]; then
  echo "FAIL the restore failed; the dump is not restorable" >&2
  exit "$EXIT_FAIL"
fi

# ---------------------------------------------------------------------------
# The migration state.
# ---------------------------------------------------------------------------
# A dump of a migrated database carries `_sqlx_migrations`, so `sqlx migrate info` can say
# whether the schema in the dump is the schema this build expects. A dump taken mid-deploy
# shows a `pending` row; a dump restored by an older `sqlx` shows `missing`. Either is a
# restore that "succeeded" and must not be called good.
migrate_output="$(sqlx migrate info --database-url "$target_url" --source migrations 2>&1)" || {
  echo "FAIL sqlx migrate info failed against the restored database:" >&2
  printf '%s\n' "$migrate_output" >&2
  exit "$EXIT_FAIL"
}
installed="$(printf '%s\n' "$migrate_output" | grep -c '/installed ' || true)"
pending="$(printf '%s\n' "$migrate_output" | grep -c '/pending ' || true)"
missing="$(printf '%s\n' "$migrate_output" | grep -c '/missing ' || true)"
expected="$(find migrations -maxdepth 1 -name '*.sql' 2>/dev/null | wc -l | tr -d ' ')"

echo
echo "=== schema"
echo "migrations: $installed installed, $pending pending, $missing missing (expected $expected)"
if [ "$installed" -eq 0 ]; then
  echo "FAIL the restored database has no migrations applied at all" >&2
  exit "$EXIT_FAIL"
fi
if [ "$pending" -ne 0 ] || [ "$missing" -ne 0 ]; then
  echo "FAIL the dump's migration state does not match this build ($pending pending, $missing missing)" >&2
  printf '%s\n' "$migrate_output" >&2
  exit "$EXIT_FAIL"
fi

# ---------------------------------------------------------------------------
# The data.
# ---------------------------------------------------------------------------
# The sixteen `+sync` tables, from `crates/server/tests/schema.rs::SYNC_TABLES`. They are the
# tables a student's work lives in, which is what a backup exists for: a restore that brought
# back the schema and the billing tables but not the student's tasks has failed at the only
# thing that matters. Kept in step with that list by hand, and this comment says so rather
# than implying a check that is not here.
sync_tables=(
  workspaces sources messages decks flashcards flashcard_reviews quizzes quiz_questions
  quiz_attempts reports cheatsheets tasks goals events study_sessions pending_actions
)

count_in() {
  local url="$1" table="$2"
  psql "$url" -tAq -c "select count(*) from \"$table\"" 2>/dev/null
}

echo
echo "=== row counts, the +sync tables"
printf '%-20s %12s %12s\n' table restored source
failures=0
total=0
while IFS= read -r table; do
  restored="$(count_in "$target_url" "$table")"
  if [ -z "$restored" ]; then
    echo "FAIL $table: the query failed in the restored database (the table is absent)"
    failures=$((failures + 1))
    continue
  fi
  total=$((total + restored))

  if [ -z "$source_url" ]; then
    printf '%-20s %12s %12s\n' "$table" "$restored" "-"
    continue
  fi

  source="$(count_in "$source_url" "$table")"
  if [ -z "$source" ]; then
    echo "FAIL $table: the source query failed; is --source-url reachable and is $table there?"
    failures=$((failures + 1))
    continue
  fi
  printf '%-20s %12s %12s\n' "$table" "$restored" "$source"

  # The source's own zero is allowed to restore to zero — a deployment with no flashcards yet
  # is not a failed restore. A source count above zero restoring to zero is exactly the
  # failure this whole script exists for.
  if [ "$source" -eq 0 ]; then
    continue
  fi
  if [ "$restored" -eq 0 ]; then
    echo "FAIL $table: the source has $source rows and the restore has none"
    failures=$((failures + 1))
    continue
  fi
  delta=$((restored - source))
  [ "$delta" -lt 0 ] && delta=$((-delta))
  if [ "$delta" -gt "$tolerance" ]; then
    echo "FAIL $table: restored $restored vs source $source, off by $delta (tolerance $tolerance)"
    failures=$((failures + 1))
  fi
done < <(printf '%s\n' "${sync_tables[@]}")

if [ "$total" -eq 0 ]; then
  echo "FAIL every +sync table is empty in the restore; the schema came back and the data did not" >&2
  exit "$EXIT_FAIL"
fi

if [ "$failures" -ne 0 ]; then
  echo
  echo "FAIL $failures row-count check(s) failed — the backup is not a backup" >&2
  exit "$EXIT_FAIL"
fi

echo
echo "=== PASS"
echo "restored $(basename "$dump") ($size bytes) into $scratch_db:"
echo "  $installed migrations installed, none pending or missing"
echo "  $total rows across the ${#sync_tables[@]} +sync tables"
if [ -z "$source_url" ]; then
  echo "  no --source-url was given, so the counts were not compared against a live database"
fi
echo "record this output, dated, in docs/OPERATIONS.md § Backups and the restore drill (A347)"
