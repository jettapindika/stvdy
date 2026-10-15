-- Content sort order for manual reordering.
ALTER TABLE decks ADD COLUMN sort_order int NOT NULL DEFAULT 0;
ALTER TABLE quizzes ADD COLUMN sort_order int NOT NULL DEFAULT 0;
ALTER TABLE reports ADD COLUMN sort_order int NOT NULL DEFAULT 0;
ALTER TABLE cheatsheets ADD COLUMN sort_order int NOT NULL DEFAULT 0;
