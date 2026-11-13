-- When the retention sweep last failed to delete a generation's kept files, on
-- the database's clock. NULL means it never failed. The sweep takes the rows
-- it has not tried first, then the ones it tried least recently, so rows whose
-- deletes keep failing (a bucket that refuses) can't fill every batch and
-- hold up the rest. No index: the sweep sorts one pass's due rows, and a
-- nullable column with no default is a metadata-only change on Postgres 11+.
ALTER TABLE generations ADD COLUMN IF NOT EXISTS files_delete_attempted_at TIMESTAMPTZ NULL;
