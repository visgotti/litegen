-- Organizations (tenants)
CREATE TABLE organizations (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    slug        TEXT NOT NULL UNIQUE,
    plan        TEXT NOT NULL DEFAULT 'free',
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE organization_members (
    org_id      TEXT NOT NULL REFERENCES organizations(id),
    user_id     TEXT NOT NULL REFERENCES users(id),
    role        TEXT NOT NULL CHECK (role IN ('owner','admin','member','viewer')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, user_id)
);
CREATE INDEX idx_org_members_user ON organization_members(user_id);

CREATE TABLE applications (
    id          TEXT PRIMARY KEY,
    org_id      TEXT NOT NULL REFERENCES organizations(id),
    name        TEXT NOT NULL,
    slug        TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, slug)
);
CREATE INDEX idx_applications_org ON applications(org_id);

CREATE TABLE provider_credentials (
    id            TEXT PRIMARY KEY,
    app_id        TEXT NOT NULL REFERENCES applications(id),
    provider      TEXT NOT NULL,
    ciphertext    TEXT NOT NULL,
    nonce         TEXT NOT NULL,
    display_hint  TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (app_id, provider)
);

-- Tenant columns on existing tables (nullable; backfilled below).
ALTER TABLE api_keys           ADD COLUMN org_id TEXT REFERENCES organizations(id),
                               ADD COLUMN app_id TEXT REFERENCES applications(id),
                               ADD COLUMN public_id TEXT;
ALTER TABLE generations        ADD COLUMN org_id TEXT REFERENCES organizations(id),
                               ADD COLUMN app_id TEXT REFERENCES applications(id);
ALTER TABLE request_logs       ADD COLUMN org_id TEXT REFERENCES organizations(id),
                               ADD COLUMN app_id TEXT REFERENCES applications(id);
ALTER TABLE request_artifacts  ADD COLUMN org_id TEXT REFERENCES organizations(id),
                               ADD COLUMN app_id TEXT REFERENCES applications(id);
ALTER TABLE webhook_deliveries ADD COLUMN org_id TEXT REFERENCES organizations(id),
                               ADD COLUMN app_id TEXT REFERENCES applications(id);
ALTER TABLE audit_log          ADD COLUMN org_id TEXT REFERENCES organizations(id);
ALTER TABLE invitations        ADD COLUMN org_id TEXT REFERENCES organizations(id);

-- Backfill: one default org + app, members from existing users, stamp existing rows.
INSERT INTO organizations (id, name, slug) VALUES ('00000000-0000-0000-0000-000000000001', 'Default', 'default');
INSERT INTO applications (id, org_id, name, slug)
    VALUES ('00000000-0000-0000-0000-000000000002', '00000000-0000-0000-0000-000000000001', 'Default', 'default');
INSERT INTO organization_members (org_id, user_id, role)
    SELECT '00000000-0000-0000-0000-000000000001', id, role FROM users;
UPDATE api_keys           SET org_id = '00000000-0000-0000-0000-000000000001', app_id = '00000000-0000-0000-0000-000000000002' WHERE org_id IS NULL;
UPDATE generations        SET org_id = '00000000-0000-0000-0000-000000000001', app_id = '00000000-0000-0000-0000-000000000002' WHERE org_id IS NULL;
UPDATE request_logs       SET org_id = '00000000-0000-0000-0000-000000000001', app_id = '00000000-0000-0000-0000-000000000002' WHERE org_id IS NULL;
UPDATE request_artifacts  SET org_id = '00000000-0000-0000-0000-000000000001', app_id = '00000000-0000-0000-0000-000000000002' WHERE org_id IS NULL;
UPDATE webhook_deliveries SET org_id = '00000000-0000-0000-0000-000000000001', app_id = '00000000-0000-0000-0000-000000000002' WHERE org_id IS NULL;
UPDATE audit_log          SET org_id = '00000000-0000-0000-0000-000000000001' WHERE org_id IS NULL;
UPDATE invitations        SET org_id = '00000000-0000-0000-0000-000000000001' WHERE org_id IS NULL;

CREATE INDEX idx_api_keys_tenant     ON api_keys(org_id, app_id, created_at);
CREATE INDEX idx_generations_tenant  ON generations(org_id, app_id, created_at);
CREATE INDEX idx_request_logs_tenant ON request_logs(org_id, app_id, created_at);
CREATE INDEX idx_audit_log_tenant           ON audit_log(org_id, created_at);
CREATE INDEX idx_request_artifacts_tenant  ON request_artifacts(org_id, app_id, created_at);
CREATE INDEX idx_webhook_deliveries_tenant ON webhook_deliveries(org_id, app_id, created_at);
CREATE UNIQUE INDEX idx_api_keys_public_id ON api_keys(public_id) WHERE public_id IS NOT NULL;
