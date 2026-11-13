-- Which replica's poller holds an active generation, and until when. A
-- replica claims the rows it polls and renews them every tick; a claim that
-- lapses (a replica that died) leaves the row free for another one. NULL
-- means nobody holds it. `poll_claimed_until` is on the database's clock.
ALTER TABLE generations ADD COLUMN IF NOT EXISTS poll_claimed_by TEXT NULL;
ALTER TABLE generations ADD COLUMN IF NOT EXISTS poll_claimed_until TIMESTAMPTZ NULL;
-- No index here, on purpose. On Postgres the claim query uses the existing
-- `idx_generations_status`: the active set (pending and processing) is small,
-- because finished rows leave it, so a composite (status, created_at) index
-- would add little. Building one takes a lock that blocks every write to
-- `generations` for as long as the build lasts, which is long on a large table
-- at upgrade time. The usual remedy, CREATE INDEX CONCURRENTLY, can't be used
-- in a migration: sqlx 0.8 runs a migration file as one query under its own
-- advisory lock, and a concurrent build deadlocks with a replica that boots
-- meanwhile. (SQLite's 023 has an index; SQLite runs one replica.)
