-- Per-app BYO object storage config. Non-secret fields are plaintext (shown/edited
-- in the dashboard); the {access_key_id, secret_access_key} pair is AES-256-GCM
-- encrypted into (secret_ciphertext, secret_nonce) with LITEGEN__SECRETS_KEY.
-- One config per app.
CREATE TABLE app_storage_credentials (
    app_id              TEXT PRIMARY KEY REFERENCES applications(id),
    backend             TEXT NOT NULL DEFAULT 's3',
    bucket_name         TEXT NOT NULL,
    region              TEXT NOT NULL DEFAULT 'us-east-1',
    endpoint_url        TEXT,
    custom_public_url   TEXT,
    path_prefix         TEXT,
    access_key_id_hint  TEXT,
    secret_ciphertext   TEXT NOT NULL,
    secret_nonce        TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
