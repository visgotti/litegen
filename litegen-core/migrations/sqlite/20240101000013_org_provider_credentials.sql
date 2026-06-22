-- Org-scoped provider credentials (replaces per-app provider_credentials).
CREATE TABLE org_provider_credentials (
    id            TEXT PRIMARY KEY,
    org_id        TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    provider      TEXT NOT NULL,
    ciphertext    TEXT NOT NULL,
    nonce         TEXT NOT NULL,
    display_hint  TEXT,
    created_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at    TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (org_id, provider)
);

-- Best-effort copy-up: for each (org, provider) take the earliest-created app
-- credential. SQLite's bare-column rule returns the row matching MIN(created_at).
INSERT INTO org_provider_credentials (id, org_id, provider, ciphertext, nonce, display_hint, created_at, updated_at)
SELECT pc.id, a.org_id, pc.provider, pc.ciphertext, pc.nonce, pc.display_hint, MIN(pc.created_at), pc.updated_at
FROM provider_credentials pc
JOIN applications a ON a.id = pc.app_id
GROUP BY a.org_id, pc.provider;

DROP TABLE provider_credentials;
