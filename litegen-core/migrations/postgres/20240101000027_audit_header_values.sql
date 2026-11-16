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
-- are, so running it again changes nothing. The tokens should still be
-- rotated (docs/RUNBOOK.md, "Provider header tokens in old audit entries").
--
-- The columns are text: an entry is read as jsonb and a changed one written
-- back as jsonb's text (`{"a": "b"}`, keys in jsonb's order). Text jsonb
-- can't read (not JSON, or a `\u0000` escape, which jsonb refuses) is left as
-- it is, with a warning, instead of failing the upgrade.
DO $$
DECLARE
    col text;
    r record;
    doc jsonb;
BEGIN
    FOREACH col IN ARRAY ARRAY['before_json', 'after_json'] LOOP
        FOR r IN EXECUTE format(
            'SELECT id, %1$I AS body FROM audit_log WHERE action = %2$L AND %1$I IS NOT NULL',
            col, 'provider.configure'
        ) LOOP
            BEGIN
                doc := r.body::jsonb;
            EXCEPTION WHEN invalid_text_representation OR untranslatable_character THEN
                RAISE WARNING 'audit entry %: % is not JSON jsonb can read, left as it is', r.id, col;
                CONTINUE;
            END;
            -- On its own line, so jsonb_each never sees anything but an object.
            CONTINUE WHEN jsonb_typeof(doc -> 'extra_headers') IS DISTINCT FROM 'object';
            IF EXISTS (
                SELECT 1 FROM jsonb_each(doc -> 'extra_headers') AS h WHERE h.value <> to_jsonb('[redacted]'::text)
            ) THEN
                doc := jsonb_set(doc, '{extra_headers}', (
                    SELECT jsonb_object_agg(h.key, '[redacted]'::text) FROM jsonb_each(doc -> 'extra_headers') AS h
                ));
                EXECUTE format('UPDATE audit_log SET %I = $1 WHERE id = $2', col) USING doc::text, r.id;
            END IF;
        END LOOP;
    END LOOP;
END $$;
