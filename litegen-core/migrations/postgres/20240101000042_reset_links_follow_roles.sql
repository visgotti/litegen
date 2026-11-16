-- 042: a password reset link follows the account's roles. Its issuer was
-- checked against them when it was issued (POST /v1/users/{id}/password-reset:
-- the master key, or a person who outranked the account in every org it
-- belonged to), so any change to them deletes the account's unused links, in
-- the transaction of the change, whatever code makes it (review round 3b, S3).
--
-- The triggers:
--   reset_links_on_member_insert, _delete, _update (SQLite), or
--   reset_links_on_membership and reset_links_on_membership_change (Postgres):
--     a membership added or removed, or its role, org or account changed:
--     joining (accepting an invitation, making an org), leaving or being
--     removed, a role change, an owner transfer in an org, an org deleted.
--   reset_links_on_user_role: the account's own role changed (PATCH
--     /v1/users/{id}, the deployment's owner transfer).
--   reset_links_on_user_deactivated: is_active going from true to false.
--     Deactivating also ends the account's sessions, OAuth grants and
--     unredeemed codes, in the same transaction, in litegen's own code.
-- Other writes to users (signing in, a new password) fire none of them, and a
-- used link stays, as the record of its use. The index serves the deletes.
-- A rollback past 042 leaves the triggers in place; an older binary doesn't
-- mind them.
CREATE INDEX IF NOT EXISTS idx_password_resets_user ON password_resets(user_id);

CREATE OR REPLACE FUNCTION litegen_reset_links_of_member() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') THEN
        DELETE FROM password_resets WHERE user_id = OLD.user_id AND used_at IS NULL;
    END IF;
    IF TG_OP IN ('INSERT', 'UPDATE') THEN
        DELETE FROM password_resets WHERE user_id = NEW.user_id AND used_at IS NULL;
    END IF;
    RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS reset_links_on_membership ON organization_members;
CREATE TRIGGER reset_links_on_membership
    AFTER INSERT OR DELETE ON organization_members
    FOR EACH ROW
    EXECUTE FUNCTION litegen_reset_links_of_member();

DROP TRIGGER IF EXISTS reset_links_on_membership_change ON organization_members;
CREATE TRIGGER reset_links_on_membership_change
    AFTER UPDATE OF role, org_id, user_id ON organization_members
    FOR EACH ROW
    WHEN (OLD.role IS DISTINCT FROM NEW.role OR OLD.org_id IS DISTINCT FROM NEW.org_id OR OLD.user_id IS DISTINCT FROM NEW.user_id)
    EXECUTE FUNCTION litegen_reset_links_of_member();

CREATE OR REPLACE FUNCTION litegen_reset_links_of_user() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM password_resets WHERE user_id = NEW.id AND used_at IS NULL;
    RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS reset_links_on_user_role ON users;
CREATE TRIGGER reset_links_on_user_role
    AFTER UPDATE OF role ON users
    FOR EACH ROW
    WHEN (OLD.role IS DISTINCT FROM NEW.role)
    EXECUTE FUNCTION litegen_reset_links_of_user();

DROP TRIGGER IF EXISTS reset_links_on_user_deactivated ON users;
CREATE TRIGGER reset_links_on_user_deactivated
    AFTER UPDATE OF is_active ON users
    FOR EACH ROW
    WHEN (OLD.is_active AND NOT NEW.is_active)
    EXECUTE FUNCTION litegen_reset_links_of_user();
