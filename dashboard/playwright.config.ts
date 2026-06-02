import { defineConfig } from '@playwright/test';
import path from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const MASTER_KEY = process.env.PLAYWRIGHT_MASTER_KEY ?? 'test-master-key-please-rotate';
const BINARY_PATH = path.resolve(__dirname, '../litegen-core/target/release/litegen');
const MODELS_DIR = path.resolve(__dirname, '../models');

export default defineConfig({
  testDir: './e2e',
  timeout: 240_000,
  reporter: [['list'], ['html', { open: 'never' }]],
  use: {
    baseURL: 'http://127.0.0.1:5174/',
    video: 'on',
    trace: 'on',
    launchOptions: {
      slowMo: 250,
    },
  },
  webServer: [
    {
      // Start the litegen binary (pre-built release binary)
      command: BINARY_PATH,
      url: 'http://127.0.0.1:5099/health',
      reuseExistingServer: false,
      timeout: 30_000,
      env: {
        LITEGEN__SERVER__HOST: '127.0.0.1',
        LITEGEN__SERVER__PORT: '5099',
        LITEGEN__DATABASE_URL: 'sqlite://:memory:',
        LITEGEN__MASTER_KEY: MASTER_KEY,
        LITEGEN_MODELS_DIR: MODELS_DIR,
        LITEGEN__PROVIDERS__MOCK__API_KEY: '',
        LITEGEN_CORS_ORIGINS: 'http://localhost:5174,http://127.0.0.1:5174',
        LITEGEN__CORS__ALLOW_CREDENTIALS: 'true',
        LITEGEN__COOKIE_INSECURE_DEV: 'true',
        LITEGEN__DEV__EXPOSE_INVITE_TOKENS: 'true',
        LITEGEN__DEV__EXPOSE_RESET_TOKENS: 'true',
      },
    },
    {
      // Start vite dev server (bind to all interfaces so 127.0.0.1 check works).
      // VITE_PROXY_TARGET causes vite.config.ts to proxy /v1, /health, etc. to the
      // backend, making session cookies same-origin (SameSite=Lax works correctly).
      // VITE_API_URL is set to the same origin as the dashboard so the SDK client
      // uses the proxy path rather than calling the backend directly.
      command: 'npm run dev -- --port 5174 --host 0.0.0.0',
      url: 'http://127.0.0.1:5174/',
      reuseExistingServer: false,
      timeout: 30_000,
      env: {
        VITE_PROXY_TARGET: 'http://127.0.0.1:5099',
        VITE_API_URL: 'http://127.0.0.1:5174',
      },
    },
  ],
});
