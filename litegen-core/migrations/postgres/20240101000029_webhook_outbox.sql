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
-- claims due rows with a lease (`claimed_by`, `claimed_until`, on the
-- database's clock), sends one attempt, records it in webhook_deliveries (its
-- `delivery_id`, and no copy of the payload: that is kept here once), and
-- writes when the next attempt is due, so a restart resumes where it was. A
-- delivery ends `delivered` (a 2xx) or `failed` (410 Gone, its last attempt
-- failed, or it can't be sent) and is kept for the log.
--
-- The delivery id comes from gen_random_uuid(), built in since PostgreSQL 13.
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
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- The dispatcher holding the delivery, and until when.
    claimed_by TEXT,
    claimed_until TIMESTAMPTZ,
    last_status_code INTEGER,
    last_error TEXT,
    -- The payload of the latest attempt.
    payload_json TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ
);
-- The dispatchers' claim reads the pending deliveries by when they are due.
CREATE INDEX IF NOT EXISTS idx_webhook_outbox_due ON webhook_outbox(status, next_attempt_at);
CREATE INDEX IF NOT EXISTS idx_webhook_outbox_key ON webhook_outbox(key_id, created_at);
CREATE INDEX IF NOT EXISTS idx_webhook_outbox_tenant ON webhook_outbox(org_id, app_id);

-- Each attempt names its delivery.
ALTER TABLE webhook_deliveries ADD COLUMN IF NOT EXISTS delivery_id TEXT;
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_delivery ON webhook_deliveries(delivery_id);

-- A trigger's WHEN can't hold a subquery, so the key's check is in the body.
CREATE OR REPLACE FUNCTION litegen_webhook_outbox_on_terminal() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.key_id IS NOT NULL
       AND NOT (NEW.status = 'completed' AND NEW.media_type = 'image' AND NEW.provider_job_id IS NULL)
       AND EXISTS (SELECT 1 FROM api_keys k WHERE k.id = NEW.key_id AND k.webhook_url IS NOT NULL) THEN
        INSERT INTO webhook_outbox (id, generation_id, key_id, org_id, app_id)
        VALUES ('whd-' || gen_random_uuid()::text, NEW.id, NEW.key_id, NEW.org_id, NEW.app_id)
        ON CONFLICT (generation_id) DO NOTHING;
    END IF;
    RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS webhook_outbox_on_terminal ON generations;
CREATE TRIGGER webhook_outbox_on_terminal
    AFTER UPDATE OF status ON generations
    FOR EACH ROW
    WHEN (OLD.status IN ('pending', 'processing') AND NEW.status IN ('completed', 'failed', 'cancelled'))
    EXECUTE FUNCTION litegen_webhook_outbox_on_terminal();
