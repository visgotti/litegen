import { defineConfig } from '@playwright/test';
import path from 'path';
import { fileURLToPath } from 'url';

// Generation call hooks in a real browser (docs/hooks.md): two litegen
// servers, each running one example hook from examples/hooks/, called by a
// customer's app on another origin (e2e-hooks/acme-app.mjs). Run with
//   npx playwright test --config playwright.hooks.config.ts
// It keeps its own output folder and reporter, so it never clears the main
// suite's videos or report.

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const MASTER_KEY = 'hooks-e2e-master';
const BINARY_PATH = process.env.LITEGEN_E2E_BINARY
  ? path.resolve(process.env.LITEGEN_E2E_BINARY)
  : path.resolve(__dirname, '../litegen-core/target/release/litegen');

const server = (dir: string, port: number) => ({
  command: BINARY_PATH,
  cwd: path.resolve(__dirname, 'e2e-hooks', dir),
  url: `http://127.0.0.1:${port}/health`,
  reuseExistingServer: false,
  timeout: 30_000,
  env: {
    LITEGEN__SERVER__HOST: '127.0.0.1',
    LITEGEN__SERVER__PORT: String(port),
    LITEGEN__DATABASE_URL: 'sqlite://:memory:',
    LITEGEN__MASTER_KEY: MASTER_KEY,
    LITEGEN__PROVIDERS__MOCK__API_KEY: '',
    LITEGEN_MODELS_DIR: path.resolve(__dirname, '../models'),
    // The page calling litegen lives on this origin.
    LITEGEN_CORS_ORIGINS: 'http://127.0.0.1:5299',
    LITEGEN__CORS__ALLOW_CREDENTIALS: 'true',
    // Read by the example hooks, inherited from litegen's environment.
    ACME_TOKENS: 'alice-token:alice',
    LITEGEN_APP_KEY: MASTER_KEY,
    ALLOWED_ORIGINS: 'http://127.0.0.1:5299',
  },
});

export default defineConfig({
  testDir: './e2e-hooks',
  outputDir: './test-results-hooks',
  timeout: 120_000,
  reporter: [['list']],
  use: {
    viewport: { width: 1280, height: 720 },
    video: { mode: 'on', size: { width: 1280, height: 720 } },
    launchOptions: { slowMo: 250 },
  },
  webServer: [
    server('token', 5199),
    server('cookie', 5198),
    // "Acme", the customer's app on another origin (e2e-hooks/acme-app.mjs).
    { command: 'node e2e-hooks/acme-app.mjs', url: 'http://127.0.0.1:5299/', reuseExistingServer: false, timeout: 10_000 },
  ],
});
