CREATE TABLE users (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT,
    role TEXT NOT NULL CHECK(role IN ('owner','admin','member','viewer')),
    oauth_github_id TEXT UNIQUE,
    oauth_google_id TEXT UNIQUE,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_login_at TIMESTAMP,
    is_active BOOLEAN NOT NULL DEFAULT TRUE
);
CREATE INDEX idx_users_email ON users(email);

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id),
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at TIMESTAMP NOT NULL,
    ip TEXT,
    user_agent TEXT,
    csrf_token TEXT NOT NULL
);
CREATE INDEX idx_sessions_user ON sessions(user_id, expires_at);

CREATE TABLE invitations (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('admin','member','viewer')),
    token TEXT NOT NULL UNIQUE,
    invited_by TEXT REFERENCES users(id),
    expires_at TIMESTAMP NOT NULL,
    used_at TIMESTAMP,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_invitations_token ON invitations(token);

CREATE TABLE password_resets (
    token TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id),
    expires_at TIMESTAMP NOT NULL,
    used_at TIMESTAMP,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE login_attempts (
    email TEXT NOT NULL,
    attempted_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL
);
CREATE INDEX idx_login_attempts_email ON login_attempts(email, attempted_at DESC);

ALTER TABLE api_keys ADD COLUMN owner_user_id TEXT REFERENCES users(id);
ALTER TABLE audit_log ADD COLUMN actor_user_id TEXT REFERENCES users(id);
