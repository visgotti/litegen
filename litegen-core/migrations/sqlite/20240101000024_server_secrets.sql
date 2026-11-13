-- Secrets litegen makes for itself, by name, that every replica must share.
-- `link_signing_key` is the HMAC key behind signed file links when
-- LITEGEN__SECRETS_KEY is not set: the first process to boot stores one, and
-- every replica reads it back, so a link signed on one replica verifies on all
-- of them and survives a restart. A value is written once and never changed.
-- Anyone who can read this table can forge file links, so treat a copy of the
-- database (a backup, a dump) as holding the key.
CREATE TABLE IF NOT EXISTS server_secrets (
    name TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
