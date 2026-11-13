-- Which replica's poller holds an active generation, and until when. A
-- replica claims the rows it polls and renews them every tick; a claim that
-- lapses (a replica that died) leaves the row free for another one. NULL
-- means nobody holds it. `poll_claimed_until` is written and compared only as
-- SQLite's own `strftime('%Y-%m-%d %H:%M:%f', 'now', ...)` text, which keeps
-- milliseconds (CURRENT_TIMESTAMP stops at whole seconds).
ALTER TABLE generations ADD COLUMN poll_claimed_by TEXT NULL;
ALTER TABLE generations ADD COLUMN poll_claimed_until TIMESTAMP NULL;
-- The poller reads the oldest active generations (status, then age).
CREATE INDEX IF NOT EXISTS idx_generations_active ON generations(status, created_at);
