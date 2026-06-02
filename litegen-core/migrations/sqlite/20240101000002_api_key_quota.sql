-- Add quota/scopes/rpm/webhook columns to api_keys
ALTER TABLE api_keys ADD COLUMN token_quota REAL;
ALTER TABLE api_keys ADD COLUMN tokens_used REAL NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN rpm_limit INTEGER;
ALTER TABLE api_keys ADD COLUMN scopes TEXT NOT NULL DEFAULT 'generate,read';
ALTER TABLE api_keys ADD COLUMN webhook_url TEXT;
