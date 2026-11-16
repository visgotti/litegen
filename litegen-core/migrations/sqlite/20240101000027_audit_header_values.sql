-- Provider header values out of the audit log (leftovers N1, 2026-11-14).
--
-- A provider's extra headers can carry a gateway's own auth
-- (`cf-aig-authorization`, `Helicone-Auth`). The provider view shows their
-- names only, but until 2026-11-14 `PUT /v1/admin/providers/{name}` kept the
-- values in its `provider.configure` audit entry, which `GET /v1/audit`
-- serves. In hosted mode the master key's entries are filed under the
-- default org, so that org's Owners and Admins could read them.
--
-- This writes `[redacted]` over every header value in those entries, in
-- `before_json` and `after_json`, as entries are written now. The header
-- names and the rest of the settings stay. Rows of other actions, rows with
-- no headers or only redacted ones, and text that isn't JSON are left as they
-- are, so running it again changes nothing. The CASE keeps text that isn't
-- JSON away from the JSON functions, which would fail the upgrade on it. The
-- tokens should still be rotated (docs/RUNBOOK.md, "Provider header tokens in
-- old audit entries").
UPDATE audit_log
SET before_json = json_set(
        before_json,
        '$.extra_headers',
        json((SELECT json_group_object(h.key, '[redacted]') FROM json_each(audit_log.before_json, '$.extra_headers') AS h))
    )
WHERE action = 'provider.configure'
  AND CASE WHEN json_valid(before_json) THEN
        json_type(before_json, '$.extra_headers') = 'object'
        AND EXISTS (SELECT 1 FROM json_each(audit_log.before_json, '$.extra_headers') AS h WHERE h.value IS NOT '[redacted]')
      END;

UPDATE audit_log
SET after_json = json_set(
        after_json,
        '$.extra_headers',
        json((SELECT json_group_object(h.key, '[redacted]') FROM json_each(audit_log.after_json, '$.extra_headers') AS h))
    )
WHERE action = 'provider.configure'
  AND CASE WHEN json_valid(after_json) THEN
        json_type(after_json, '$.extra_headers') = 'object'
        AND EXISTS (SELECT 1 FROM json_each(audit_log.after_json, '$.extra_headers') AS h WHERE h.value IS NOT '[redacted]')
      END;
