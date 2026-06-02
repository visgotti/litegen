CREATE TABLE IF NOT EXISTS webhook_deliveries (
    id TEXT PRIMARY KEY,
    key_id TEXT NOT NULL,
    generation_id TEXT NOT NULL,
    url TEXT NOT NULL,
    attempt_number INTEGER NOT NULL,
    status_code INTEGER,
    success INTEGER NOT NULL,
    response_body TEXT,
    error_message TEXT,
    payload_json TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_key ON webhook_deliveries(key_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_gen ON webhook_deliveries(generation_id);
