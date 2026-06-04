-- Per-app BYO object storage config (see postgres mirror). One config per app.
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
    created_at          TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at          TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
