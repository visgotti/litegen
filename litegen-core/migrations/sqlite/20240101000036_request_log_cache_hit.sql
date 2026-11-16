-- Whether a request was answered from the cache: an image's
-- `X-Litegen-Cache: hit`. Such a request has a row of its own, at what it
-- was charged (the cached answer's price, or nothing with
-- `cache.charge_hits: false`); this says it was a hit. Rows from before this
-- read false (docs/gateway-settings.md).
ALTER TABLE request_logs ADD COLUMN cache_hit BOOLEAN NOT NULL DEFAULT 0;
