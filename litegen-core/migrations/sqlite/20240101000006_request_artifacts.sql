CREATE TABLE request_artifacts (
    request_id TEXT PRIMARY KEY,
    media_type TEXT NOT NULL,
    prompt TEXT,
    negative_prompt TEXT,
    params_json TEXT,
    refs_meta_json TEXT,
    output_kind TEXT NOT NULL,
    output_value TEXT,
    output_mime TEXT,
    output_truncated INTEGER NOT NULL DEFAULT 0,
    error_message TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_request_artifacts_created_at ON request_artifacts(created_at DESC);
