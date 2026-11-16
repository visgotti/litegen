-- The durable webhook outbox: one row per terminal webhook, a delivery
-- (docs/management-api.md, "Key webhooks"; docs/scaling.md).
--
-- Who records a delivery: the trigger at the end of this file, in the same
-- statement as the generation's terminal write, so a crash right after that
-- write can't lose the webhook. It fires exactly where litegen sends a
-- terminal webhook (`dispatch_terminal_webhook`, src/proxy/poller.rs):
--   - the status moves from pending or processing to completed, failed or
--     cancelled: the guarded terminal writes of the poller, a reap, a client's
--     poll and a cancel (a row already terminal never moves again);
--   - the generation has a key, and the key has a webhook_url then;
--   - it isn't a sync image result: an image with no vendor job id that its
--     own request completes, which gets no webhook. A late image has its
--     vendor's job id, and an image row that fails (a reap) is recorded.
--
-- Who sends it: each replica's dispatcher (src/proxy/webhook.rs, `Outbox`)
-- claims due rows with a lease (`claimed_by`, `claimed_until`), sends one
-- attempt, records it in webhook_deliveries (its `delivery_id`, and no copy
-- of the payload: that is kept here once), and writes when the next attempt
-- is due, so a restart resumes where it was. A delivery ends `delivered` (a
-- 2xx) or `failed` (410 Gone, its last attempt failed, or it can't be sent)
-- and is kept for the log.
--
-- Times are SQLite's own `strftime('%Y-%m-%d %H:%M:%f', 'now', ...)` text,
-- written and compared only in that format, as the poll claims are (023).
--
-- A database restored without this trigger records no deliveries: litegen
-- then sends each terminal webhook from memory, as before 029 (lost on a
-- crash), and logs "no outbox delivery was recorded for a terminal webhook".
CREATE TABLE IF NOT EXISTS webhook_outbox (
    -- The delivery id, `whd-` and a UUID: X-Litegen-Delivery-Id on every attempt.
    id TEXT PRIMARY KEY,
    generation_id TEXT NOT NULL UNIQUE,
    key_id TEXT NOT NULL,
    org_id TEXT,
    app_id TEXT,
    -- pending, delivered or failed.
    status TEXT NOT NULL DEFAULT 'pending',
    -- The attempts sent so far.
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT NOT NULL,
    -- The dispatcher holding the delivery, and until when.
    claimed_by TEXT,
    claimed_until TEXT,
    last_status_code INTEGER,
    last_error TEXT,
    -- The payload of the latest attempt.
    payload_json TEXT,
    created_at TEXT NOT NULL,
    finished_at TEXT
);
-- The dispatchers' claim reads the pending deliveries by when they are due.
CREATE INDEX IF NOT EXISTS idx_webhook_outbox_due ON webhook_outbox(status, next_attempt_at);
CREATE INDEX IF NOT EXISTS idx_webhook_outbox_key ON webhook_outbox(key_id, created_at);
CREATE INDEX IF NOT EXISTS idx_webhook_outbox_tenant ON webhook_outbox(org_id, app_id);

-- Each attempt names its delivery.
ALTER TABLE webhook_deliveries ADD COLUMN delivery_id TEXT;
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_delivery ON webhook_deliveries(delivery_id);

CREATE TRIGGER IF NOT EXISTS webhook_outbox_on_terminal
AFTER UPDATE OF status ON generations
FOR EACH ROW
WHEN OLD.status IN ('pending', 'processing')
 AND NEW.status IN ('completed', 'failed', 'cancelled')
 AND NEW.key_id IS NOT NULL
 AND NOT (NEW.status = 'completed' AND NEW.media_type = 'image' AND NEW.provider_job_id IS NULL)
 AND EXISTS (SELECT 1 FROM api_keys k WHERE k.id = NEW.key_id AND k.webhook_url IS NOT NULL)
BEGIN
    INSERT OR IGNORE INTO webhook_outbox (id, generation_id, key_id, org_id, app_id, status, attempts, next_attempt_at, created_at)
    VALUES (
        -- A version 4 UUID.
        'whd-' || lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))), 2)
            || '-' || substr('89ab', 1 + (random() & 3), 1) || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6))),
        NEW.id, NEW.key_id, NEW.org_id, NEW.app_id, 'pending', 0,
        strftime('%Y-%m-%d %H:%M:%f', 'now'), strftime('%Y-%m-%d %H:%M:%f', 'now')
    );
END;
