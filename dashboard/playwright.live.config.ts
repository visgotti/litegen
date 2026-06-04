import { defineConfig } from '@playwright/test';

// Runs the multi-tenant UI flow against a LIVE deployed droplet (no local
// webServer — the server is the real hosted stack). Point at the droplet origin:
//   LIVE_URL=http://<droplet-ip> npx playwright test --config playwright.live.config.ts
const BASE = process.env.LIVE_URL ?? 'http://134.209.172.192';

export default defineConfig({
  testDir: './e2e-live',
  timeout: 120_000,
  reporter: [['list'], ['html', { open: 'never' }]],
  use: {
    baseURL: BASE,
    trace: 'on',
    video: 'off',
    // The droplet serves plain http; nothing TLS-sensitive here.
    ignoreHTTPSErrors: true,
  },
});
