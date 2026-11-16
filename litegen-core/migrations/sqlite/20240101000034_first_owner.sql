-- 034: the deployment's first Owner, claimed once.
--
-- In single-tenant mode the first signup makes the deployment's Owner. It
-- counted the accounts and then made one, in separate steps, so first signups
-- at once could each become an Owner (on one replica or several). Now the
-- signup inserts this table's one row (id 1) first, in the transaction that
-- makes the account and its Owner membership: of two at once, the second's
-- insert waits for the first's transaction and then conflicts, and it makes
-- nothing (`DatabaseStore::create_first_owner`, 409 `signup_closed`). A
-- deployment whose accounts predate this table has no row; its accounts close
-- the signup, as before. The row stays: a database whose accounts were all
-- deleted by hand reopens the first signup only once this row goes too.
CREATE TABLE IF NOT EXISTS first_owner (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    user_id TEXT NOT NULL,
    claimed_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
