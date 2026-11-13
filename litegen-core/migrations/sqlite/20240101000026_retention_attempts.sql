-- When the retention sweep last failed to delete a generation's kept files, as
-- SQLite's own `strftime('%Y-%m-%d %H:%M:%f', 'now')` text. NULL means it never
-- failed. The sweep takes the rows it has not tried first, then the ones it
-- tried least recently, so rows whose deletes keep failing (a bucket that
-- refuses) can't fill every batch and hold up the rest.
ALTER TABLE generations ADD COLUMN files_delete_attempted_at TIMESTAMP NULL;
