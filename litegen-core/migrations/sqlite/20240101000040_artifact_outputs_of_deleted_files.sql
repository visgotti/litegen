-- The request log's copies of outputs whose files were deleted
-- (docs/storage-access.md, "Retention and deleting"). The retention sweep
-- deleted a generation's files but left its artifact's output: an image's
-- base64 (up to 2 MiB), or a link to the deleted file. And a cache hit after
-- `DELETE /v1/generations/{id}` wrote the image back under the deleted
-- generation's id. The sweep now clears the output with the files; this
-- clears what was left before. The prompt and params stay: retention
-- promises files, and deleting a generation removes its request too.
UPDATE request_artifacts
SET output_kind = 'deleted', output_value = NULL, output_truncated = 0
WHERE output_kind IN ('b64', 'url')
  AND request_id IN (SELECT id FROM generations WHERE files_deleted_at IS NOT NULL);
