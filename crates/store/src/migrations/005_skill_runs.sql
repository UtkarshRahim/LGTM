-- `skill` holds the skill id; the name is kept for display after the skill
-- file moves or disappears.
ALTER TABLE skill_runs ADD COLUMN skill_name TEXT NOT NULL DEFAULT '';
ALTER TABLE skill_runs ADD COLUMN error TEXT;
ALTER TABLE findings ADD COLUMN end_line INTEGER;
