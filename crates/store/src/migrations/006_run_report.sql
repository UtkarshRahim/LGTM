-- The skill's writeup, with the findings JSON removed. Null on older runs.
ALTER TABLE skill_runs ADD COLUMN report TEXT;
