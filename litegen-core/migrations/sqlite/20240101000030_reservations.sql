-- Reservations held by billed requests in flight (docs/scaling.md,
-- "Reservations"; src/management/reservations.rs).
--
-- A billed request reserves its price before it calls the vendor: on its
-- key's quota (api_keys.tokens_used) and on each limits row it counts against
-- (entity_limits.spend_usd). Each reservation is recorded here in the step
-- that takes it, and its record deleted in the step that settles it or gives
-- it back, so a record exists exactly while its amount is held.
--
-- While its request runs, the replica holding it renews `expires_at` (a lease
-- of 2 minutes, renewed every 20 seconds). A record whose lease lapsed
-- belongs to a request that can no longer finish: its process crashed or was
-- killed, or stopped with the release lost. Every replica's sweep gives such
-- a record's amount back and deletes it in one step, so of two sweepers
-- exactly one does. Nothing is given back at a start: another replica's
-- requests may still be running.
--
-- Who takes a reservation, here on SQLite: the trigger `reservations_take`
-- below. litegen inserts the record with a SELECT that finds the key or the
-- limits row only while the amount fits under its cap, and the trigger adds
-- the amount to that row, in the same statement. One statement, so it happens
-- whole or not at all, wherever the request that sends it stops. Postgres
-- takes it in one statement too, without a trigger: an UPDATE in a
-- data-modifying CTE feeds the INSERT of the record
-- (src/management/reservations_sql.rs). SQLite has no data-modifying CTEs,
-- and a transaction of two statements would hold the database's one write
-- lock between them for as long as its request's task takes to go on.
--
-- A database restored without the trigger records each reservation without
-- taking it: requests aren't held against their caps while they run, each
-- settles to its cost less the estimate its record says was held (about
-- nothing), and a dropped request's release takes its estimate off. litegen
-- checks for the trigger as it starts and logs the error `the database lacks
-- migration 030's trigger reservations_take`. Re-create it with the CREATE
-- TRIGGER statement below (docs/RUNBOOK.md, "Reservations recorded but not taken").
--
-- Times are SQLite's own `strftime('%Y-%m-%d %H:%M:%f', 'now', ...)` text,
-- written and compared only in that format, as the poll claims are (023).
CREATE TABLE IF NOT EXISTS reservations (
    id TEXT PRIMARY KEY,
    -- The replica (its process, `shared::replica_id`) whose request holds it.
    holder TEXT NOT NULL,
    -- Where the amount is held: `api_key`, the `tokens_used` of the key
    -- `entity_id`; or `limits`, the `spend_usd` of entity_limits' row
    -- (`entity_type`, `entity_id`).
    target TEXT NOT NULL,
    entity_type TEXT,
    entity_id TEXT NOT NULL,
    amount REAL NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);
-- The sweep reads the lapsed records by when they lapsed.
CREATE INDEX IF NOT EXISTS idx_reservations_expires_at ON reservations(expires_at);

-- Takes a reservation as its record is inserted (see the top of this file).
CREATE TRIGGER IF NOT EXISTS reservations_take
AFTER INSERT ON reservations
FOR EACH ROW
BEGIN
    UPDATE api_keys SET tokens_used = tokens_used + NEW.amount
     WHERE NEW.target = 'api_key' AND id = NEW.entity_id;
    UPDATE entity_limits SET spend_usd = spend_usd + NEW.amount
     WHERE NEW.target = 'limits' AND entity_type = NEW.entity_type AND entity_id = NEW.entity_id;
END;

-- An Idempotency-Key in progress is held the same way: `held_by` names the
-- request that holds it, and `held_until` is its lease, renewed while the
-- request runs. A record in progress whose lease lapsed is free at once, so
-- the retry of a request whose process died runs without waiting out the 15
-- minutes after which a run is judged abandoned (still the rule for a record
-- with no lease, one made before 030). Renewed by the primary key, so no
-- index.
ALTER TABLE idempotency_keys ADD COLUMN held_by TEXT;
ALTER TABLE idempotency_keys ADD COLUMN held_until TEXT;
