-- OAuth 2.1 for the MCP endpoints: LiteGen is the authorization server for
-- `/mcp` and `/admin/mcp`. Clients register by metadata document (CIMD) or
-- dynamic registration (DCR); a person approves an authorization request on
-- the dashboard; the approval becomes a grant, and the grant's access and
-- refresh tokens are stored only as SHA-256 hashes.

CREATE TABLE IF NOT EXISTS mcp_oauth_clients (
    client_id TEXT PRIMARY KEY,
    registration TEXT NOT NULL,            -- 'dcr' | 'cimd'
    client_name TEXT NOT NULL,
    redirect_uris TEXT NOT NULL,           -- JSON array
    logo_uri TEXT NULL,
    client_uri TEXT NULL,
    metadata_json TEXT NOT NULL,
    created_at DATETIME NOT NULL,
    refreshed_at DATETIME NOT NULL
);

CREATE TABLE IF NOT EXISTS mcp_oauth_requests (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    state TEXT NULL,
    code_challenge TEXT NOT NULL,
    scopes TEXT NOT NULL,                  -- space-separated, as requested
    surface TEXT NOT NULL,                 -- 'ai' | 'admin'
    created_at DATETIME NOT NULL,
    expires_at DATETIME NOT NULL,          -- the request's, then the code's
    user_id TEXT NULL,
    org_id TEXT NULL,
    app_id TEXT NULL,
    granted_scopes TEXT NULL,
    code_hash TEXT NULL UNIQUE,
    decided_at DATETIME NULL,
    consumed_at DATETIME NULL
);

CREATE INDEX IF NOT EXISTS idx_mcp_oauth_requests_expires_at ON mcp_oauth_requests(expires_at);

CREATE TABLE IF NOT EXISTS mcp_oauth_grants (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    org_id TEXT NOT NULL,
    app_id TEXT NULL,
    surface TEXT NOT NULL,
    scopes TEXT NOT NULL,
    created_at DATETIME NOT NULL,
    last_used_at DATETIME NULL,
    revoked_at DATETIME NULL
);

CREATE INDEX IF NOT EXISTS idx_mcp_oauth_grants_user ON mcp_oauth_grants(user_id);
CREATE INDEX IF NOT EXISTS idx_mcp_oauth_grants_client_user ON mcp_oauth_grants(client_id, user_id);

CREATE TABLE IF NOT EXISTS mcp_oauth_tokens (
    token_hash TEXT PRIMARY KEY,
    grant_id TEXT NOT NULL,
    kind TEXT NOT NULL,                    -- 'access' | 'refresh'
    created_at DATETIME NOT NULL,
    expires_at DATETIME NOT NULL,
    rotated_at DATETIME NULL,
    revoked_at DATETIME NULL
);

CREATE INDEX IF NOT EXISTS idx_mcp_oauth_tokens_grant ON mcp_oauth_tokens(grant_id);
CREATE INDEX IF NOT EXISTS idx_mcp_oauth_tokens_expires_at ON mcp_oauth_tokens(expires_at);
