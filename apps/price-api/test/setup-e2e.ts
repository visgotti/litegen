/**
 * Env defaults for the e2e suite, applied before AppModule is imported.
 * Point TEST_DATABASE_URL at a disposable Postgres database. Scraping and tight
 * rate limits are disabled so tests are deterministic.
 */
process.env.NODE_ENV = 'test';
process.env.DATABASE_URL =
  process.env.TEST_DATABASE_URL ?? 'postgres://price:price@localhost:5433/price_api_test';
process.env.DB_SYNCHRONIZE = 'true';
process.env.DB_LOGGING = 'false';
process.env.JWT_SECRET = 'test-secret-test-secret-0123456789';
process.env.JWT_TTL_SECONDS = '3600';
process.env.BOOTSTRAP_CLIENT_ID = 'test-admin';
process.env.BOOTSTRAP_CLIENT_SECRET = 'test-admin-secret';
process.env.BOOTSTRAP_CLIENT_SCOPES = 'pricing:read,pricing:admin';
// Enabled so the scheduler's cron-registration path is exercised (jobs are
// registered but their schedules never fire during the short test run).
process.env.SCRAPING_ENABLED = 'true';
process.env.THROTTLE_LIMIT = '100000';
process.env.THROTTLE_TTL_SECONDS = '60';
