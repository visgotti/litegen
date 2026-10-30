-- Idempotency-Key records: one per (owner, key). A billed POST that carries
-- the header claims its key here before running and stores its response when
-- it finishes, so the same key with the same request replays that response
-- for 24 hours instead of billing again.
CREATE TABLE IF NOT EXISTS idempotency_keys (
    scope TEXT NOT NULL,
    idem_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    state TEXT NOT NULL,
    response_status INTEGER NULL,
    response_headers TEXT NULL,
    response_body BLOB NULL,
    created_at DATETIME NOT NULL,
    PRIMARY KEY (scope, idem_key)
);

CREATE INDEX IF NOT EXISTS idx_idempotency_keys_created_at ON idempotency_keys(created_at);
