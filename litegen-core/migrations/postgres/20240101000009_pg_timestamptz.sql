-- Convert any remaining `timestamp without time zone` columns to timestamptz so
-- DateTime<Utc> row fields decode under sqlx-postgres (strict). Idempotent.
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
