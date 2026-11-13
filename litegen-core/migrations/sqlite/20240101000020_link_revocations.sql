-- Links litegen signed before `revoked_at` are refused (docs/storage-access.md).
-- `scope` is gateway (scope_id ''), app or generation. A later revocation
-- replaces an earlier one; an earlier time never un-revokes.
CREATE TABLE link_revocations (
    scope       TEXT NOT NULL,
    scope_id    TEXT NOT NULL,
    revoked_at  TIMESTAMP NOT NULL,
    PRIMARY KEY (scope, scope_id)
);
