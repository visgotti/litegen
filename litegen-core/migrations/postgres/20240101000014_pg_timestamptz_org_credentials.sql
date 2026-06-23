-- 20240101000013 created `org_provider_credentials` with `created_at`/`updated_at`
-- as `timestamp without time zone`, but the Rust row decodes `DateTime<Utc>`
-- (sqlx → strict `timestamptz`), so the list endpoint 500s with
-- "TIMESTAMPTZ … not compatible with SQL type TIMESTAMP". The earlier sweep
-- (20240101000009) ran before that table existed, so re-run it. Idempotent —
-- only columns still `without time zone` are widened.
DO $$
DECLARE r record;
BEGIN
  FOR r IN SELECT table_name, column_name FROM information_schema.columns
           WHERE table_schema = 'public' AND data_type = 'timestamp without time zone'
  LOOP
    EXECUTE format('ALTER TABLE %I ALTER COLUMN %I TYPE timestamptz USING %I AT TIME ZONE ''UTC''',
                   r.table_name, r.column_name, r.column_name);
  END LOOP;
END $$;
