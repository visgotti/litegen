-- Per-app overrides for keeping outputs, retention and who signs links
-- (docs/storage-access.md). NULL uses the gateway setting, then litegen.yaml.
-- retention_days 0 keeps files until they are deleted.
ALTER TABLE app_file_access ADD COLUMN keep_outputs BOOLEAN;
ALTER TABLE app_file_access ADD COLUMN retention_days BIGINT;
ALTER TABLE app_file_access ADD COLUMN signed_links TEXT;
