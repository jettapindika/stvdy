-- Generation bookkeeping for reports and cheatsheets.
--
-- Both tables are `+sync` and already carry the sync triggers attached by
-- `0017_sync_triggers.sql`, so this migration only alters existing tables and that file
-- stays the last one that attaches a trigger. What both were missing is the state every
-- generator carries: the status a client polls, the readable error, and the charge a
-- permanent failure refunds exactly once.
--
-- `reports` also gains the two things the report flow needs and the table never had. A
-- section call is a *separate* prompt from the outline call, so the settings it reads have
-- to survive the request that asked for the outline: without `source_ids`, `style` and
-- `length` the job has no chunks and no prompt variables, and every section generation
-- would fail with an empty prompt after the student had paid for it.
alter table reports
  add column status text not null default 'draft',
  add column error text,
  add column credits_spent int not null default 0,
  add column usage_event_id uuid,
  -- The sources the outline was proposed from, so a section call selects the same chunks
  -- the outline's labels refer to. `default '{}'` is for the rows written before this
  -- migration; the route always binds a validated, deduplicated list.
  add column source_ids uuid[] not null default '{}',
  -- `style` and `length` are the two settings `prompts/report_section.v1.md` declares as
  -- required. They are stored for the same reason `quizzes.requested_count` is: the job
  -- runs after the request returned and must rebuild the same prompt, and a prompt
  -- rebuilt from a default would be a different prompt than the one the credit bought.
  add column style text not null default '',
  add column length text not null default '',
  -- The generated section texts, index-aligned with `outline.sections`: null where a
  -- section has not been written yet. Stored rather than derived from `body_md`, because
  -- splitting Markdown on its headings cannot tell a section boundary from a `## ` line
  -- the model wrote inside a section, and a report that silently attributes one section's
  -- text to another is worse than the second copy of the text this costs.
  add column sections_md jsonb,
  -- The section index the last generation targeted. Non-null while a job is in flight, and
  -- left in place on a failure so the client can point at the section that failed rather
  -- than at the report as a whole.
  add column generating_index int;

-- The monthly cap counts this month's reports inside the charge's transaction, so the
-- count reads `(user_id, created_at)`. The table's only other index is
-- `(user_id, workspace_id)`, which the list route uses.
create index on reports (user_id, created_at);

-- The cheatsheet's two bookkeeping columns. `credits_spent` is what a permanent failure
-- refunds exactly once (`credits::refund_pending`), and `usage_event_id` carries the row
-- the provider's cost and token counts are attached to (A41), which only the spending
-- route can create.
alter table cheatsheets
  add column credits_spent int not null default 0,
  add column usage_event_id uuid;
