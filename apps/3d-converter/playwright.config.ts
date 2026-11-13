import { defineConfig } from '@playwright/test';

// `stories` opens every story of the built Storybook (npm run
// build-storybook); `e2e` drives the built app (npm run build) and records a
// video of every test. PW_PROJECT picks which project's server starts.
const project = process.env.PW_PROJECT;
const wants = (name: string) => project === undefined || project === name;

export default defineConfig({
  testDir: './e2e',
  timeout: 180_000,
  // One at a time: WebGL is drawn in software, and the videos stay smooth.
  workers: 1,
  reporter: [['list']],
  use: { viewport: { width: 1280, height: 800 } },
  projects: [
    { name: 'stories', testMatch: /stories\.spec\.ts/, use: { baseURL: 'http://127.0.0.1:6107/' } },
    {
      name: 'e2e',
      testMatch: /(matrix|functional)\.spec\.ts/,
      use: {
        baseURL: 'http://127.0.0.1:5182/',
        video: { mode: 'on', size: { width: 1280, height: 800 } },
        trace: 'retain-on-failure',
      },
    },
  ],
  webServer: [
    ...(wants('stories')
      ? [{ command: 'python3 -m http.server 6107 --bind 127.0.0.1 --directory storybook-static', url: 'http://127.0.0.1:6107/index.json', reuseExistingServer: false, timeout: 30_000 }]
      : []),
    ...(wants('e2e')
      ? [{ command: 'npx vite preview --host 127.0.0.1 --port 5182 --strictPort', url: 'http://127.0.0.1:5182/', reuseExistingServer: false, timeout: 60_000 }]
      : []),
  ],
});
