ALTER TABLE github_pr ADD COLUMN added_labels TEXT[] NOT NULL DEFAULT '{}' CHECK (array_position(added_labels, NULL) IS NULL);
