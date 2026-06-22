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

-- Postgres requires DISTINCT ON for first-writer-wins (no bare-column rule).
INSERT INTO org_provider_credentials (id, org_id, provider, ciphertext, nonce, display_hint, created_at, updated_at)
SELECT DISTINCT ON (a.org_id, pc.provider)
       pc.id, a.org_id, pc.provider, pc.ciphertext, pc.nonce, pc.display_hint, pc.created_at, pc.updated_at
FROM provider_credentials pc
JOIN applications a ON a.id = pc.app_id
ORDER BY a.org_id, pc.provider, pc.created_at ASC;

DROP TABLE provider_credentials;
