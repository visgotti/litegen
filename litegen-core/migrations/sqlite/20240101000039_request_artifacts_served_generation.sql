-- 039: the artifacts of the cache hits a generation answered, found by its id.
--
-- A cache hit's artifact names the generation it was served from
-- (`output_kind = 'generation'`, the generation's id in `output_value`) and
-- carries that generation's prompt and params. Deleting the generation
-- (`DELETE /v1/generations/{id}`, `redact_generation_request`) deletes those
-- artifacts too: `WHERE output_kind = 'generation' AND output_value = ?`,
-- which without this index reads all of request_artifacts each time. The
-- index is partial: only those rows are in it. SQLite runs one replica and
-- builds it in one pass at the upgrade (Postgres's 039 says what that costs
-- there).
CREATE INDEX IF NOT EXISTS idx_request_artifacts_served_generation
    ON request_artifacts (output_value) WHERE output_kind = 'generation';
