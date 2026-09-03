-- Split the single review opt-out into independent daily and weekly
-- preferences. Existing rows keep their previous behaviour by seeding both
-- new columns from the old combined flag before it is dropped.
ALTER TABLE review_preferences ADD COLUMN daily_enabled INTEGER NOT NULL DEFAULT 1;
ALTER TABLE review_preferences ADD COLUMN weekly_enabled INTEGER NOT NULL DEFAULT 1;

UPDATE review_preferences
SET daily_enabled = reviews_enabled,
    weekly_enabled = reviews_enabled;

ALTER TABLE review_preferences DROP COLUMN reviews_enabled;
