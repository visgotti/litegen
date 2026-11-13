-- Per-app overrides of who may open stored outputs (docs/storage-access.md).
-- A NULL column uses the gateway setting, then litegen.yaml. Values are the
-- strings the API takes; one another litegen version wrote and this one does
-- not know is skipped, not an error. Like the other app tables, app_id has no
-- ON DELETE CASCADE: delete_application removes this row first.
CREATE TABLE app_file_access (
    app_id                  TEXT PRIMARY KEY REFERENCES applications(id),
    mode                    TEXT,
    owner_scope             TEXT,
    signed_url_ttl_seconds  BIGINT,
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);
