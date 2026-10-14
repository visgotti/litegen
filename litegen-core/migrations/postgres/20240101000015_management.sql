-- Management API (LiteLLM-parity): budgets/limits for every principal, end-user
-- tracking, spend attribution, runtime model + provider config, gateway settings.

-- Budget + access limits for a key, org, app, org member or end user. One row
-- per entity; `spend_usd` is the spend inside the current budget window and is
-- zeroed when `budget_reset_at` passes (lazily, on the next charge or read).
--   entity_type: key | org | app | member | end_user
--   entity_id:   key uuid | org id | app id | "{org_id}:{user_id}" | "{org_id}:{end_user_id}"
CREATE TABLE entity_limits (
    entity_type            TEXT NOT NULL,
    entity_id              TEXT NOT NULL,
    org_id                 TEXT,
    max_budget_usd         DOUBLE PRECISION,
    soft_budget_usd        DOUBLE PRECISION,
    budget_duration        TEXT,
    spend_usd              DOUBLE PRECISION NOT NULL DEFAULT 0,
    budget_reset_at        TIMESTAMPTZ,
    soft_budget_alerted_at TIMESTAMPTZ,
    allowed_models         TEXT,
    max_parallel_requests  BIGINT,
    rpm_limit              BIGINT,
    metadata               TEXT,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (entity_type, entity_id)
);
CREATE INDEX idx_entity_limits_org ON entity_limits(org_id, entity_type);

-- The callers of a tenant's own users (LiteLLM "customers"), identified per
-- request by the `x-litegen-end-user` header.
CREATE TABLE end_users (
    org_id        TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    end_user_id   TEXT NOT NULL,
    alias         TEXT,
    blocked       BOOLEAN NOT NULL DEFAULT FALSE,
    metadata      TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at  TIMESTAMPTZ,
    PRIMARY KEY (org_id, end_user_id)
);

-- Spend attribution on every request log.
ALTER TABLE request_logs ADD COLUMN key_id TEXT;
ALTER TABLE request_logs ADD COLUMN user_id TEXT;
ALTER TABLE request_logs ADD COLUMN end_user TEXT;
CREATE INDEX idx_request_logs_key ON request_logs(org_id, key_id, created_at);
CREATE INDEX idx_request_logs_end_user ON request_logs(org_id, end_user, created_at);

CREATE TABLE request_log_tags (
    request_id TEXT NOT NULL,
    tag        TEXT NOT NULL,
    PRIMARY KEY (request_id, tag)
);
CREATE INDEX idx_request_log_tags_tag ON request_log_tags(tag);

-- Models created through the API, layered over the YAML catalog at boot.
CREATE TABLE runtime_models (
    id          TEXT PRIMARY KEY,
    schema_json TEXT NOT NULL,
    updated_by  TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Platform-wide on/off switch for any model id (catalog or runtime).
CREATE TABLE model_states (
    model_id   TEXT PRIMARY KEY,
    disabled   BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Provider instance config set through the API. Secrets are AES-GCM encrypted
-- (ciphertext/nonce); everything else is plain JSON in `settings_json`.
CREATE TABLE provider_configs (
    provider      TEXT PRIMARY KEY,
    ciphertext    TEXT,
    nonce         TEXT,
    display_hint  TEXT,
    settings_json TEXT NOT NULL DEFAULT '{}',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Gateway-wide settings (markup, routes, alerting), one JSON value per key.
CREATE TABLE gateway_settings (
    key        TEXT PRIMARY KEY,
    value_json TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
