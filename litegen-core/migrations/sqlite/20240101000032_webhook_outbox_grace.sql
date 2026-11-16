-- The webhook outbox, round two (review 2026-11-15, round 2; docs/management-api.md,
-- "Key webhooks"). Migration 029 is left as it was, since a database may have
-- applied it already: this one replaces its trigger.
--
-- 1. A cleared webhook is no webhook. Clearing a key's webhook stored "",
--    which 029's trigger took for one: every later job of that key got a
--    delivery that ended failed at once, with a warning. Such keys store
--    NULL now, as clearing one does from now on, and the trigger wants a
--    webhook_url that isn't empty.
-- 2. A new delivery is the replica's that wrote its terminal write for 30 s
--    (`OUTBOX_FIRST_ATTEMPT_GRACE`, src/proxy/webhook.rs). That replica
--    settles the job first (a cancel's refund comes up to 5 s after its
--    terminal write) and then sends the first attempt at once, claiming it
--    whatever its due time, so the webhook carries the settled row. No
--    dispatcher's tick takes the delivery before `next_attempt_at`, 30 s
--    after the terminal write: it sends one whose replica died first.
-- 3. `redelivered_at`: when POST /v1/keys/{id}/webhook-deliveries/{delivery_id}/redeliver
--    last made a finished delivery due again. A redeliver waits a minute
--    after the last one, and a redelivered delivery is sent even when an
--    earlier attempt for its job was answered 2xx.
-- 4. Indexes for the retention sweep, which deletes the deliveries that
--    ended, and the attempts made, more than `webhooks.log_retention_days`
--    ago (never a pending delivery).
--
-- Otherwise the trigger fires where 029's did (see its header). Times are
-- SQLite's own `strftime('%Y-%m-%d %H:%M:%f', 'now', ...)` text, as in 029.
UPDATE api_keys SET webhook_url = NULL WHERE webhook_url = '';

ALTER TABLE webhook_outbox ADD COLUMN redelivered_at TEXT;

CREATE INDEX IF NOT EXISTS idx_webhook_outbox_finished ON webhook_outbox(finished_at);
CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_created ON webhook_deliveries(created_at);

DROP TRIGGER IF EXISTS webhook_outbox_on_terminal;
CREATE TRIGGER webhook_outbox_on_terminal
AFTER UPDATE OF status ON generations
FOR EACH ROW
WHEN OLD.status IN ('pending', 'processing')
 AND NEW.status IN ('completed', 'failed', 'cancelled')
 AND NEW.key_id IS NOT NULL
 AND NOT (NEW.status = 'completed' AND NEW.media_type = 'image' AND NEW.provider_job_id IS NULL)
 AND EXISTS (SELECT 1 FROM api_keys k WHERE k.id = NEW.key_id AND k.webhook_url IS NOT NULL AND k.webhook_url <> '')
BEGIN
    INSERT OR IGNORE INTO webhook_outbox (id, generation_id, key_id, org_id, app_id, status, attempts, next_attempt_at, created_at)
    VALUES (
        -- A version 4 UUID.
        'whd-' || lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))), 2)
            || '-' || substr('89ab', 1 + (random() & 3), 1) || substr(lower(hex(randomblob(2))), 2) || '-' || lower(hex(randomblob(6))),
        NEW.id, NEW.key_id, NEW.org_id, NEW.app_id, 'pending', 0,
        -- Due once its first-attempt grace is over.
        strftime('%Y-%m-%d %H:%M:%f', 'now', '+30 seconds'), strftime('%Y-%m-%d %H:%M:%f', 'now')
    );
END;
