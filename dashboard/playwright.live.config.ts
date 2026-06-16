import { defineConfig } from '@playwright/test';
import path from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

// Runs the hosted multi-tenant UI flow against a hosted-equivalent stack:
//   • No LIVE_URL  → self-boots a LOCAL hosted stack (mode=hosted, password auth +
//     a dummy OAuth provider enabled, /api routed like prod) and runs green out of
//     the box. This is what makes the suite runnable without a live deployment.
//   • LIVE_URL set → targets that real deployment, no local servers. Use the real
//     hostname so Caddy host-routing + auto-TLS apply:
//       LIVE_URL=https://app.litegen.ai npx playwright test --config playwright.live.config.ts
//     (Note: the signup flow needs password_enabled=true; an OAuth-only deployment
//     can only satisfy the read-only authgate spec.)
const LIVE_URL = process.env.LIVE_URL;
const BACKEND_PORT = '5199';
const UI_PORT = '5274';
const BASE = LIVE_URL ?? `http://127.0.0.1:${UI_PORT}`;

const MASTER_KEY = process.env.PLAYWRIGHT_MASTER_KEY ?? 'test-master-key-please-rotate';
const BINARY_PATH = path.resolve(__dirname, '../litegen-core/target/release/litegen');
const MODELS_DIR = path.resolve(__dirname, '../models');

export default defineConfig({
  testDir: './e2e-live',
  timeout: 120_000,
  reporter: [['list'], ['html', { open: 'never' }]],
  use: {
    baseURL: BASE,
    trace: 'on',
    video: 'off',
    ignoreHTTPSErrors: true,
  },
  // Self-boot a local hosted stack unless targeting a real LIVE_URL.
  webServer: LIVE_URL
    ? undefined
    : [
        {
          // Hosted mode → signup_open=true + multi-tenant orgs/apps. SECRETS_KEY is
          // required for the BYO-credential encryption the flow exercises. A dummy
          // Google provider makes the login page render an oauth-* button (authgate
          // asserts one is present); no real OAuth round-trip is performed.
          command: BINARY_PATH,
          url: `http://127.0.0.1:${BACKEND_PORT}/health/live`,
          reuseExistingServer: false,
          timeout: 30_000,
          env: {
            LITEGEN__SERVER__HOST: '127.0.0.1',
            LITEGEN__SERVER__PORT: BACKEND_PORT,
            LITEGEN__MODE: 'hosted',
            LITEGEN__SECRETS_KEY: 'RBw/W1IPa/KYpdXhe6F1g8/AjRpHw5MGm9kND7K37nw=',
            LITEGEN__DATABASE_URL: 'sqlite://:memory:',
            LITEGEN__MASTER_KEY: MASTER_KEY,
            LITEGEN_MODELS_DIR: MODELS_DIR,
            LITEGEN__PROVIDERS__MOCK__API_KEY: '',
            LITEGEN__OAUTH__GOOGLE__CLIENT_ID: 'dummy-client-id',
            LITEGEN__OAUTH__GOOGLE__CLIENT_SECRET: 'dummy-client-secret',
            LITEGEN_CORS_ORIGINS: `http://localhost:${UI_PORT},http://127.0.0.1:${UI_PORT}`,
            LITEGEN__CORS__ALLOW_CREDENTIALS: 'true',
            LITEGEN__COOKIE_INSECURE_DEV: 'true',
          },
        },
        {
          // VITE_API_URL points at /api so the SPA SDK uses the same prefix as prod;
          // the vite proxy (see vite.config.ts) strips /api before forwarding.
          command: `npm run dev -- --port ${UI_PORT} --host 0.0.0.0`,
          url: `http://127.0.0.1:${UI_PORT}/`,
          reuseExistingServer: false,
          timeout: 30_000,
          env: {
            VITE_PROXY_TARGET: `http://127.0.0.1:${BACKEND_PORT}`,
            VITE_API_URL: `http://127.0.0.1:${UI_PORT}/api`,
          },
        },
      ],
});
