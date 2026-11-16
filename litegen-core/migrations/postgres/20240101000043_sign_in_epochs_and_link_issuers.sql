-- 043: sign-in epochs and reset-link issuers (review round 4).
--
-- users.sign_in_epoch moves on, in the same transaction, with every confirmed
-- reset, deactivation and change of one's own password. A session, an OAuth
-- code (at its approval) and an OAuth grant (from its code) record the epoch
-- they began under, and work only while it is still the account's: a login,
-- an approval or a redemption in flight while one of those commits is
-- refused afterwards. Rows from before keep epoch 0, the accounts' too.
ALTER TABLE users ADD COLUMN IF NOT EXISTS sign_in_epoch BIGINT NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS sign_in_epoch BIGINT NOT NULL DEFAULT 0;
ALTER TABLE mcp_oauth_requests ADD COLUMN IF NOT EXISTS sign_in_epoch BIGINT NULL;
ALTER TABLE mcp_oauth_grants ADD COLUMN IF NOT EXISTS sign_in_epoch BIGINT NOT NULL DEFAULT 0;
--
-- password_resets.issued_by: the account that issued the link, or NULL for
-- the master key. Confirming re-checks that it may still issue it.
ALTER TABLE password_resets ADD COLUMN IF NOT EXISTS issued_by TEXT NULL;
CREATE INDEX IF NOT EXISTS idx_password_resets_issued_by ON password_resets(issued_by);
--
-- 042's triggers now follow a link's issuer too (finding 5): an issuer's
-- unused links go when the issuer leaves or is removed from an org, has a
-- membership's role changed, has the account's role changed, or is
-- deactivated. And writing is_active false deletes the account's unused
-- links every time, also when it was already inactive (finding 3). On
-- insert, the membership trigger still deletes only the new member's links:
-- joining an org takes nothing from an issuer.
CREATE OR REPLACE FUNCTION litegen_reset_links_of_member() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') THEN
        DELETE FROM password_resets WHERE used_at IS NULL AND (user_id = OLD.user_id OR issued_by = OLD.user_id);
    END IF;
    IF TG_OP = 'INSERT' THEN
        DELETE FROM password_resets WHERE used_at IS NULL AND user_id = NEW.user_id;
    ELSIF TG_OP = 'UPDATE' THEN
        DELETE FROM password_resets WHERE used_at IS NULL AND (user_id = NEW.user_id OR issued_by = NEW.user_id);
    END IF;
    RETURN NULL;
END;
$$;

CREATE OR REPLACE FUNCTION litegen_reset_links_of_user() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM password_resets WHERE used_at IS NULL AND (user_id = NEW.id OR issued_by = NEW.id);
    RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS reset_links_on_user_deactivated ON users;
CREATE TRIGGER reset_links_on_user_deactivated
    AFTER UPDATE OF is_active ON users
    FOR EACH ROW
    WHEN (NOT NEW.is_active)
    EXECUTE FUNCTION litegen_reset_links_of_user();
