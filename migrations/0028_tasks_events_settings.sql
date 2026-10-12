-- Phase 9's three settings the table never had, and the column that stops a batch of
-- subtasks from coming back shuffled.
--
-- This migration adds no `+sync` table and attaches no trigger: `tasks` and
-- `user_settings` are already in `0017_sync_triggers.sql`'s arrays (and `user_settings`
-- in its own `log_change_by_user` pair), so that file stays the last migration that
-- attaches a trigger and `crates/server/tests/schema.rs` needs no new row. The precedent
-- is 0021-0023, which each say the same thing in their header.

-- A breakdown inserts 3-8 subtasks in one transaction, and a batch insert shares
-- `created_at` to the microsecond, so `order by created_at` is a random order (A196).
-- `position` is an ordinary client-writable field: the client numbers them 0..n-1 and
-- push carries it like any other.
alter table tasks
  add column position int not null default 0;

-- Quiet hours are a pair or neither: both null means "no quiet hours", and a window may
-- wrap midnight (`22 -> 7`), so the reader tests membership and never `start < end`.
alter table user_settings
  add column quiet_start_hour int,
  add column quiet_end_hour int,
  add constraint user_settings_quiet_hours_paired
    check ((quiet_start_hour is null) = (quiet_end_hour is null)),
  add constraint user_settings_quiet_hours_range
    check (quiet_start_hour is null
           or (quiet_start_hour between 0 and 23 and quiet_end_hour between 0 and 23)),
  -- The spec's DDL has these two on `user_settings`; 0012 dropped them. They are here
  -- rather than on `users` because a language chosen on the phone is the language the
  -- laptop should use, which is what the synced row is for.
  add column language text not null default 'en',
  add column theme text not null default 'system',
  add constraint user_settings_language check (language in ('en', 'id')),
  add constraint user_settings_theme check (theme in ('system', 'light', 'dark'));

-- The study-hours and block-length columns already have defaults and no bounds; a value
-- out of range reaches the planner in Phase 11 as a negative window. The bounds are added
-- here, where the columns are, rather than as a validation the caller must remember.
alter table user_settings
  add constraint user_settings_hours
    check (study_start_hour between 0 and 23
           and study_end_hour between 1 and 24
           and study_start_hour < study_end_hour),
  add constraint user_settings_limits
    check (max_study_min_per_day between 15 and 1440
           and block_len_min between 5 and 180
           and break_min between 0 and 60);
