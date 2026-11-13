import { defineConfig } from 'vitest/config';

// The pure parts only: what needs WebGL is proven in the apps' Storybooks
// and e2e suites, in a real browser.
export default defineConfig({
  test: { include: ['src/**/*.test.ts'] },
});
