-- 041: reset and invitation tokens are kept only as SHA-256 hashes
-- (`auth::tokens::token_hash`), so a copy of the database holds none that works.
--
-- Reset tokens live an hour: the plain ones stored before are deleted, so a
-- link handed out in the hour before the upgrade has to be issued again.
DELETE FROM password_resets;
--
-- An invitation made from now on stores only its token's hash, in token_hash
-- and in token (which stays NOT NULL). One made before keeps its plain token
-- and no hash, and is still found by that token until it expires: 7 days after
-- it was made, so at most 7 days after the upgrade.
--
-- While old and new replicas run side by side (a rolling upgrade), or after a
-- rollback past 041, an old binary looks a token up as it is stored
-- (`WHERE token = ?`). The link a new replica handed out doesn't work there,
-- but the stored hash does: anyone with a copy of the database could accept a
-- new invitation, or take an account over through an unused reset link, on an
-- old binary. A new replica accepts invitations that old ones made, and
-- neither can confirm a reset link that the other issued. So finish the
-- rollout before handing out links, and before rolling back past 041 delete
-- the unused password_resets rows and the unused invitations that have a
-- token_hash (docs/RUNBOOK.md, "Reset and invitation links across the 041
-- upgrade").
ALTER TABLE invitations ADD COLUMN token_hash TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_invitations_token_hash ON invitations(token_hash);
