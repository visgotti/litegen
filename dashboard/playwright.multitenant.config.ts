import { defineConfig } from '@playwright/test';
import path from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const BINARY_PATH = path.resolve(__dirname, '../litegen-core/target/release/litegen');
const BACKEND_CWD = path.resolve(__dirname, '../litegen-core'); // so embedded ./migrations/sqlite resolves
const MODELS_DIR = path.resolve(__dirname, '../models');
const DB_PATH = '/tmp/litegen-e2e-mt.db';

export default defineConfig({
  testDir: './e2e-mt',
  timeout: 240_000,
  reporter: [['list'], ['html', { open: 'never' }]],
  use: {
    baseURL: 'http://127.0.0.1:5274/',
    video: 'on',
    trace: 'on',
    launchOptions: {
      slowMo: 250,
    },
  },
  webServer: [
    {
      // Recreate a fresh DB each run, then run the litegen binary in HOSTED mode.
      // `exec` so the binary inherits the PID (Playwright can SIGTERM it cleanly).
      command: `rm -f ${DB_PATH} && exec ${BINARY_PATH}`,
      cwd: BACKEND_CWD,
      // /health/live is unauthenticated and always returns 200 (bare /health requires
      // Scope::Read and returns 401, which is a flaky readiness signal).
      url: 'http://127.0.0.1:5199/health/live',
      reuseExistingServer: false,
      timeout: 60_000,
      env: {
        LITEGEN__SERVER__HOST: '127.0.0.1',
        LITEGEN__SERVER__PORT: '5199',
        LITEGEN__DATABASE_URL: `sqlite://${DB_PATH}?mode=rwc`,
        LITEGEN__MODE: 'hosted',
        LITEGEN__MASTER_KEY: 'mt-master-key',
        LITEGEN__SECRETS_KEY: 'MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=', // 32 bytes base64
        LITEGEN_MODELS_DIR: MODELS_DIR,
        LITEGEN__PROVIDERS__MOCK__API_KEY: '', // enable the credential-free mock provider
        LITEGEN_CORS_ORIGINS: 'http://localhost:5274,http://127.0.0.1:5274',
        LITEGEN__CORS__ALLOW_CREDENTIALS: 'true',
        LITEGEN__COOKIE_INSECURE_DEV: 'true',
        LITEGEN__DEV__EXPOSE_INVITE_TOKENS: 'true',
        LITEGEN__DEV__EXPOSE_RESET_TOKENS: 'true',
      },
    },
    {
      // Vite dev server on 5274. VITE_PROXY_TARGET makes vite.config.ts proxy /v1,
      // /health, etc. to the backend so session cookies are same-origin (SameSite=Lax).
      // VITE_API_URL points the SDK at the dashboard origin → uses the proxy path.
      command: 'npm run dev -- --port 5274 --host 0.0.0.0',
      url: 'http://127.0.0.1:5274/',
      reuseExistingServer: false,
      timeout: 60_000,
      env: {
        VITE_PROXY_TARGET: 'http://127.0.0.1:5199',
        VITE_API_URL: 'http://127.0.0.1:5274',
      },
    },
  ],
});
