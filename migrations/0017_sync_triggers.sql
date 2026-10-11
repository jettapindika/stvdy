-- Attach the sync triggers to every +sync table.
--
-- THIS MIGRATION MUST ALWAYS BE LAST. It attaches triggers to every synced table
-- created before it, so a new +sync table is added by inserting its migration
-- ABOVE this one and appending the table name to the array below — never by
-- creating a new migration after this one.
--
-- The table list here is the contract: crates/server/tests/schema.rs asserts
-- exactly these tables carry both triggers, and asserts that no other table does.

do $$
declare
  t text;
  id_tables text[] := array[
    'workspaces', 'sources', 'messages', 'decks', 'flashcards',
    'flashcard_reviews', 'quizzes', 'quiz_questions', 'quiz_attempts',
    'reports', 'cheatsheets', 'tasks', 'goals', 'events', 'study_sessions',
    'pending_actions'
  ];
begin
  foreach t in array id_tables loop
    execute format(
      'create trigger %I_touch_updated_at before update on %I
         for each row execute function touch_updated_at()', t, t);
    execute format(
      'create trigger %I_log_change after insert or update or delete on %I
         for each row execute function log_change()', t, t);
  end loop;
end $$;

-- user_settings is keyed by user_id, has no tombstones, and only needs updates
-- to reach other devices, so it uses log_change_by_user() and has no DELETE
-- trigger (its rows are removed only by the users cascade).
create trigger user_settings_touch_updated_at
  before update on user_settings
  for each row execute function touch_updated_at();

create trigger user_settings_log_change
  after insert or update on user_settings
  for each row execute function log_change_by_user();
