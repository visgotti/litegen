-- Video/image generation tracking table
CREATE TABLE IF NOT EXISTS generations (
    id TEXT PRIMARY KEY NOT NULL,
    key_id TEXT NULL REFERENCES api_keys(id),
    model TEXT NOT NULL,
    provider TEXT NOT NULL,
    media_type TEXT NOT NULL DEFAULT 'video',
    status TEXT NOT NULL DEFAULT 'pending',
    progress INTEGER NOT NULL DEFAULT 0,
    provider_job_id TEXT NULL,
    result_url TEXT NULL,
    error_message TEXT NULL,
    cost_usd REAL NOT NULL DEFAULT 0,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMP NULL,
    metadata TEXT NULL
);

CREATE INDEX IF NOT EXISTS idx_generations_status ON generations(status);
CREATE INDEX IF NOT EXISTS idx_generations_key_id ON generations(key_id);
CREATE INDEX IF NOT EXISTS idx_generations_created_at ON generations(created_at);
