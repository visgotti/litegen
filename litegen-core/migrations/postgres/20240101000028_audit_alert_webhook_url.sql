-- The budget alert webhook URL out of the audit log (leftovers N1, 2026-11-14).
--
-- A webhook URL can carry its secret in the path (Slack, Discord) or in the
-- host (Pipedream, an AWS Lambda function URL), and the budget alert POST is
-- unsigned, so the URL is all that guards the receiver.
-- `GET /v1/admin/settings` shows it to the gateway's admins only, but until
-- 2026-11-14 `PUT /v1/admin/settings` kept the whole URL in its
-- `settings.update` audit entry, which `GET /v1/audit` serves. In hosted mode
-- the master key's entries are filed under the default org, so that org's
-- Owners and Admins could read it.
--
-- Entries now show the URL as `[redacted]`, and this writes the same over the
-- stored `alert_webhook_url` of those entries, in `before_json` and
-- `after_json`. An empty or null URL stays as it is, so an entry still says
-- whether one was set. Rows of other actions, entries with no URL, text that
-- isn't JSON and values already redacted are left as they are, so running it
-- again changes nothing.
--
-- The columns are text: an entry is read as jsonb and a changed one written
-- back as jsonb's text (`{"a": "b"}`, keys in jsonb's order). Text jsonb
-- can't read (not JSON, or a `\u0000` escape, which jsonb refuses) is left as
-- it is, with a warning, instead of failing the upgrade.
--
-- A first draft of this file (2026-11-14, before release) kept each URL's
-- origin. A development database that applied that draft must be recreated.
DO $$
DECLARE
    col text;
    r record;
    doc jsonb;
BEGIN
    FOREACH col IN ARRAY ARRAY['before_json', 'after_json'] LOOP
        FOR r IN EXECUTE format(
            'SELECT id, %1$I AS body FROM audit_log WHERE action = %2$L AND %1$I IS NOT NULL',
            col, 'settings.update'
        ) LOOP
            BEGIN
                doc := r.body::jsonb;
            EXCEPTION WHEN invalid_text_representation OR untranslatable_character THEN
                RAISE WARNING 'audit entry %: % is not JSON jsonb can read, left as it is', r.id, col;
                CONTINUE;
            END;
            CONTINUE WHEN jsonb_typeof(doc -> 'alert_webhook_url') IS DISTINCT FROM 'string';
            CONTINUE WHEN doc ->> 'alert_webhook_url' IN ('', '[redacted]');
            doc := jsonb_set(doc, '{alert_webhook_url}', to_jsonb('[redacted]'::text));
            EXECUTE format('UPDATE audit_log SET %I = $1 WHERE id = $2', col) USING doc::text, r.id;
        END LOOP;
    END LOOP;
END $$;
