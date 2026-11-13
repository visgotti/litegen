import { defineConfig } from '@playwright/test';
import path from 'path';
import { fileURLToPath } from 'url';

// Gateway-wide settings on the dashboard, on a hosted backend of their own so
// what they change never reaches another suite:
// - who may open stored outputs (docs/storage-access.md): an app set to
//   signed, then private links on the Organization page, the gateway default
//   on the Gateway page, and the Playground previewing a private app's files
//   with the dashboard's own session;
// - the cache, retries, the vendor timeout and the circuit breaker
//   (docs/gateway-settings.md). Run with
//   npx playwright test --config playwright.access.config.ts

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const BINARY_PATH = process.env.LITEGEN_E2E_BINARY
  ? path.resolve(process.env.LITEGEN_E2E_BINARY)
  : path.resolve(__dirname, '../litegen-core/target/release/litegen');

export default defineConfig({
  testDir: './e2e-access',
  outputDir: './test-results-access',
  timeout: 240_000,
  // The specs change gateway-wide settings on one backend: one at a time.
  workers: 1,
  fullyParallel: false,
  reporter: [['list']],
  use: {
    baseURL: 'http://127.0.0.1:5374/',
    viewport: { width: 1280, height: 720 },
    video: { mode: 'on', size: { width: 1280, height: 720 } },
    launchOptions: { slowMo: 250 },
  },
  webServer: [
    {
      command: BINARY_PATH,
      cwd: path.resolve(__dirname, '../litegen-core'),
      url: 'http://127.0.0.1:5399/health/live',
      reuseExistingServer: false,
      timeout: 60_000,
      env: {
        LITEGEN__SERVER__HOST: '127.0.0.1',
        LITEGEN__SERVER__PORT: '5399',
        LITEGEN__DATABASE_URL: 'sqlite://:memory:',
        LITEGEN__MODE: 'hosted',
        LITEGEN__MASTER_KEY: 'access-e2e-master',
        // Also keys the signed links to litegen's own files.
        LITEGEN__SECRETS_KEY: 'MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=',
        LITEGEN_MODELS_DIR: path.resolve(__dirname, '../models'),
        LITEGEN__PROVIDERS__MOCK__API_KEY: '',
        LITEGEN_CORS_ORIGINS: 'http://localhost:5374,http://127.0.0.1:5374',
        LITEGEN__CORS__ALLOW_CREDENTIALS: 'true',
        LITEGEN__COOKIE_INSECURE_DEV: 'true',
      },
    },
    {
      command: 'npm run dev -- --port 5374 --host 0.0.0.0',
      url: 'http://127.0.0.1:5374/',
      reuseExistingServer: false,
      timeout: 60_000,
      env: {
        VITE_PROXY_TARGET: 'http://127.0.0.1:5399',
        VITE_API_URL: 'http://127.0.0.1:5374',
      },
    },
  ],
});
