-- An app's bucket reached through a cross-account IAM role
-- (docs/storage-access.md): credential_mode 'assume_role' with role_arn; NULL
-- or 'access_key' is the encrypted key pair. fallback_to_litegen_store false
-- keeps the app's files out of litegen's own store when its bucket is down.
ALTER TABLE app_storage_credentials ADD COLUMN credential_mode TEXT;
ALTER TABLE app_storage_credentials ADD COLUMN role_arn TEXT;
ALTER TABLE app_storage_credentials ADD COLUMN fallback_to_litegen_store BOOLEAN;
