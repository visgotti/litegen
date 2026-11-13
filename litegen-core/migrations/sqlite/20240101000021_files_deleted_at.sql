-- When litegen deleted a generation's kept files, and why: `retention` (the
-- app's retention_days) or `deleted` (on request). The row itself stays, as
-- the billing record (docs/storage-access.md).
ALTER TABLE generations ADD COLUMN files_deleted_at TIMESTAMP NULL;
ALTER TABLE generations ADD COLUMN files_deleted_reason TEXT NULL;
-- The retention sweep reads one app's oldest generations.
CREATE INDEX IF NOT EXISTS idx_generations_app_created ON generations(app_id, created_at);
