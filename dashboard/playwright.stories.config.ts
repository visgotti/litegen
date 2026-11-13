import { defineConfig } from '@playwright/test';

// The dashboard's 3D stories, in a real browser, against a built Storybook
// (`npm run build-storybook`): the shared viewer (@litegen/3d-viewer) as the
// dashboard fills it. The package's own stories are checked in
// apps/3d-converter's stories project. No litegen binary needed.
export default defineConfig({
  testDir: './e2e-stories',
  outputDir: 'test-results-stories',
  workers: 1,
  reporter: [['list']],
  use: { baseURL: 'http://127.0.0.1:6116/', viewport: { width: 1280, height: 800 } },
  webServer: {
    command: 'python3 -m http.server 6116 --bind 127.0.0.1 --directory storybook-static',
    url: 'http://127.0.0.1:6116/index.json',
    reuseExistingServer: false,
    timeout: 30_000,
  },
});
