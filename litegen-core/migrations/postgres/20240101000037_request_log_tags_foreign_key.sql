-- 037 (Postgres only): a request's spend tags hang off its request-log row.
--
-- request_log_tags had no foreign key. So a tag written while its org's or
-- app's delete ran, after the delete had deleted the tenant's tags and before
-- it committed, outlived the log row it belonged to: under READ COMMITTED
-- nothing stopped it, and the request's own check after its writes
-- (src/proxy/tenant_files.rs) still saw the tenant. Now `request_id`
-- references `request_logs(id)` ON DELETE CASCADE:
--   - a tag written before its log row is deleted is deleted with it;
--   - a tag of a row being deleted waits for that delete, and is refused
--     once it commits;
--   - no new tag names a row that isn't there.
-- Whatever deletes log rows (an org's or app's delete, a request forgotten
-- because its tenant went while it ran) deletes their tags. Those deletes
-- still delete tags first, which SQLite needs; here the cascade then finds
-- none.
--
-- The key is added NOT VALID: Postgres doesn't check the tags already there,
-- so adding it is a brief lock on both tables and no pass over them (it
-- waits for writes in progress, and writes wait for it). From then on every
-- new tag is checked and a log row's delete takes its tags. Request logs
-- build up, so on a long-running deployment both tables can be large, and a
-- check here would hold every write to them until it ended. Migration 038,
-- in a transaction of its own once this lock is gone, deletes the tags whose
-- log row was already gone and validates the key while writes go on.
--
-- SQLite has no 037. It runs one writer at a time, so no tag can arrive
-- between a delete's statements there, and its deletes delete tags
-- explicitly (src/db/sqlite.rs).

ALTER TABLE request_log_tags
    ADD CONSTRAINT request_log_tags_request_id_fkey
    FOREIGN KEY (request_id) REFERENCES request_logs(id) ON DELETE CASCADE NOT VALID;
