-- 038 (Postgres only): the tags that were there before migration 037's key.
--
-- 037 added the key from request_log_tags.request_id to request_logs(id)
-- NOT VALID, so the tags written before it were never checked. Those whose
-- log row is gone are deleted here: org and app deletes from before they
-- took their requests' tags left them, as did tags written for a request
-- whose log row was never written. Then the key is validated, so every tag
-- is known to have its row.
--
-- Writes go on meanwhile. The delete locks only the orphans' rows, and
-- VALIDATE CONSTRAINT takes SHARE UPDATE EXCLUSIVE on request_log_tags and
-- ROW SHARE on request_logs, which no insert, update or delete waits for.
-- No table lock is needed for the check to pass: since 037 every new tag is
-- checked and a log row's delete takes its tags, so no orphan can appear
-- between the delete and the check. A tag being written while 037 ran had
-- finished before 037 got its lock, so it is one of the tags this pass
-- looks at.
--
-- SQLite has no 038, as it has no 037.

DELETE FROM request_log_tags t
WHERE NOT EXISTS (SELECT 1 FROM request_logs l WHERE l.id = t.request_id);

ALTER TABLE request_log_tags VALIDATE CONSTRAINT request_log_tags_request_id_fkey;
