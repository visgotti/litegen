-- Reservations held by billed requests in flight (docs/scaling.md,
-- "Reservations"; src/management/reservations.rs).
--
-- A billed request reserves its price before it calls the vendor: on its
-- key's quota (api_keys.tokens_used) and on each limits row it counts against
-- (entity_limits.spend_usd). Each reservation is recorded here in the
-- statement that takes it, and its record deleted in the statement that
-- settles it or gives it back, so a record exists exactly while its amount is
-- held.
--
-- While its request runs, the replica holding it renews `expires_at` (a lease
-- of 2 minutes, renewed every 20 seconds). A record whose lease lapsed
-- belongs to a request that can no longer finish: its process crashed or was
-- killed, or stopped with the release lost. Every replica's sweep deletes
-- such records and gives their amounts back in one transaction, taking only
-- records no other sweep holds (FOR UPDATE SKIP LOCKED), so of two sweepers
-- exactly one does; it updates the keys and limits rows in one fixed order
-- (keys by id, then limits rows by type and id), so two sweeps can't
-- deadlock. Nothing is given back at a start: another replica's requests may
-- still be running. Times are on the database's clock (`now()`).
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
    amount DOUBLE PRECISION NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);
-- The sweep reads the lapsed records by when they lapsed. The table is new,
-- so building the index takes no time.
CREATE INDEX IF NOT EXISTS idx_reservations_expires_at ON reservations(expires_at);

-- An Idempotency-Key in progress is held the same way: `held_by` names the
-- request that holds it, and `held_until` is its lease, renewed while the
-- request runs. A record in progress whose lease lapsed is free at once, so
-- the retry of a request whose process died runs without waiting out the 15
-- minutes after which a run is judged abandoned (still the rule for a record
-- with no lease, one made before 030). Two nullable columns with no default:
-- adding them rewrites nothing. Renewed by the primary key, so no index.
ALTER TABLE idempotency_keys ADD COLUMN IF NOT EXISTS held_by TEXT;
ALTER TABLE idempotency_keys ADD COLUMN IF NOT EXISTS held_until TIMESTAMPTZ;
