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
-- again changes nothing. The CASE keeps text that isn't JSON away from the
-- JSON functions, which would fail the upgrade on it.
--
-- A first draft of this file (2026-11-14, before release) kept each URL's
-- origin. A development database that applied that draft must be recreated.
UPDATE audit_log
SET before_json = json_set(before_json, '$.alert_webhook_url', '[redacted]')
WHERE action = 'settings.update'
  AND CASE WHEN json_valid(before_json) THEN
        json_type(before_json, '$.alert_webhook_url') = 'text'
        AND json_extract(before_json, '$.alert_webhook_url') NOT IN ('', '[redacted]')
      END;

UPDATE audit_log
SET after_json = json_set(after_json, '$.alert_webhook_url', '[redacted]')
WHERE action = 'settings.update'
  AND CASE WHEN json_valid(after_json) THEN
        json_type(after_json, '$.alert_webhook_url') = 'text'
        AND json_extract(after_json, '$.alert_webhook_url') NOT IN ('', '[redacted]')
      END;
