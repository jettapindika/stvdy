-- Generation is asynchronous with visible progress, so the deck carries the state
-- the client polls. `credits_spent` exists so a generation that fails for good can
-- refund exactly what was charged, once; `usage_event_id` carries the row the
-- provider's cost and token counts are attached to, which the margin report needs
-- (A41) and which only the spending route can create.
alter table decks
  add column status text not null default 'ready',
  add column error text,
  add column requested_count int not null default 0,
  add column credits_spent int not null default 0,
  add column usage_event_id uuid;

-- A deck is created `generating`; the default is `ready` so any row written by a
-- path that does not generate (a future hand-made deck) is usable immediately.
