-- Course identity on a workspace.
--
-- A workspace is a course, and the design's sidebar course list and course cards
-- present it as one: a code ("BIO 201"), a categorical colour, and the date of the
-- next exam. None of the three existed, and none of them can be derived — a code
-- read off the name would be a guess, a colour from the row's position would change
-- when a workspace is deleted, and an exam date is a fact only the student has.
--
-- All three are nullable/defaulted so every existing row stays valid and no backfill
-- is needed:
--
--   * `course_code`  null until the student sets it. Every surface that shows it
--                    falls back to the name, so a null is a smaller card, not a
--                    broken one.
--   * `color`        not null, defaulted to the row's ordinal position among the
--                    account's live workspaces, so an account that never touches
--                    it still gets distinguishable courses. The six hues are
--                    categorical and the value is an index into them; it is never
--                    a status colour.
--   * `exam_date`    null until the student sets it. It is a *date*, not an
--                    instant: an exam is on a day in the reader's calendar, and
--                    storing it as `timestamptz` would make "5 November" render as
--                    the 4th for a Jakarta student. The planner already anchors on
--                    the goal's date for the same reason.

alter table workspaces
  add column course_code text,
  add column color smallint not null default 0,
  add column exam_date date;

-- A course code is short by nature; the cap keeps a paste of a paragraph from
-- becoming a card title.
alter table workspaces
  add constraint workspaces_course_code_len check (course_code is null or char_length(course_code) <= 16);

-- The hue is an index into six categorical colours. Bounded here rather than only
-- in the handler, because the client picks a colour by this number and an
-- out-of-range value would render an undefined CSS variable.
alter table workspaces
  add constraint workspaces_color_range check (color between 0 and 5);

-- Seed the existing rows so a live account's courses are distinguishable
-- immediately rather than all rendering the first hue. The ordinal is taken per
-- user, oldest first, so it is stable for rows that already exist.
with numbered as (
  select id, (row_number() over (partition by user_id order by created_at, id) - 1) % 6 as hue
  from workspaces
)
update workspaces w
set color = numbered.hue
from numbered
where numbered.id = w.id;
