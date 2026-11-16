-- 043: sign-in epochs and reset-link issuers (review round 4).
--
-- users.sign_in_epoch moves on, in the same transaction, with every confirmed
-- reset, deactivation and change of one's own password. A session, an OAuth
-- code (at its approval) and an OAuth grant (from its code) record the epoch
-- they began under, and work only while it is still the account's: a login,
-- an approval or a redemption in flight while one of those commits is
-- refused afterwards. Rows from before keep epoch 0, the accounts' too.
ALTER TABLE users ADD COLUMN sign_in_epoch INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN sign_in_epoch INTEGER NOT NULL DEFAULT 0;
ALTER TABLE mcp_oauth_requests ADD COLUMN sign_in_epoch INTEGER NULL;
ALTER TABLE mcp_oauth_grants ADD COLUMN sign_in_epoch INTEGER NOT NULL DEFAULT 0;
--
-- password_resets.issued_by: the account that issued the link, or NULL for
-- the master key. Confirming re-checks that it may still issue it.
ALTER TABLE password_resets ADD COLUMN issued_by TEXT NULL;
CREATE INDEX IF NOT EXISTS idx_password_resets_issued_by ON password_resets(issued_by);
--
-- 042's triggers now follow a link's issuer too (finding 5): an issuer's
-- unused links go when the issuer leaves or is removed from an org, has a
-- membership's role changed, has the account's role changed, or is
-- deactivated. And writing is_active false deletes the account's unused
-- links every time, also when it was already inactive (finding 3).
-- reset_links_on_member_insert stays as 042 made it: joining an org takes
-- nothing from an issuer.
DROP TRIGGER IF EXISTS reset_links_on_member_delete;
CREATE TRIGGER reset_links_on_member_delete
AFTER DELETE ON organization_members
FOR EACH ROW
BEGIN
    DELETE FROM password_resets WHERE used_at IS NULL AND (user_id = OLD.user_id OR issued_by = OLD.user_id);
END;

DROP TRIGGER IF EXISTS reset_links_on_member_update;
CREATE TRIGGER reset_links_on_member_update
AFTER UPDATE OF role, org_id, user_id ON organization_members
FOR EACH ROW
WHEN NEW.role IS NOT OLD.role OR NEW.org_id IS NOT OLD.org_id OR NEW.user_id IS NOT OLD.user_id
BEGIN
    DELETE FROM password_resets WHERE used_at IS NULL
      AND (user_id IN (OLD.user_id, NEW.user_id) OR issued_by IN (OLD.user_id, NEW.user_id));
END;

DROP TRIGGER IF EXISTS reset_links_on_user_role;
CREATE TRIGGER reset_links_on_user_role
AFTER UPDATE OF role ON users
FOR EACH ROW
WHEN NEW.role IS NOT OLD.role
BEGIN
    DELETE FROM password_resets WHERE used_at IS NULL AND (user_id = NEW.id OR issued_by = NEW.id);
END;

DROP TRIGGER IF EXISTS reset_links_on_user_deactivated;
CREATE TRIGGER reset_links_on_user_deactivated
AFTER UPDATE OF is_active ON users
FOR EACH ROW
WHEN NOT NEW.is_active
BEGIN
    DELETE FROM password_resets WHERE used_at IS NULL AND (user_id = NEW.id OR issued_by = NEW.id);
END;
