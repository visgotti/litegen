import type { StorybookConfig } from '@storybook/react-vite';

// Set up like the dashboard's (dashboard/.storybook/main.ts): Storybook 10's
// core already ships controls, actions, backgrounds and play functions, so no
// addons. The builder reuses ../vite.config.ts (module workers, the wasm
// package excluded from pre-bundling).
const config: StorybookConfig = {
  // The app's own stories, and the shared viewer package's
  // (packages/3d-viewer), which the dashboard's Storybook shows too.
  stories: ['../src/**/*.stories.@(ts|tsx)', '../../../packages/3d-viewer/src/**/*.stories.@(ts|tsx)'],
  addons: [],
  framework: { name: '@storybook/react-vite', options: {} },
  // Real model files, read-only: mesh's test vectors written by independent
  // tools (litegen's own content, see their READMEs), the dashboard's
  // Storybook meshes, and this app's samples.
  staticDirs: [
    { from: '../../../litegen-core/src/mesh/testdata', to: '/testdata' },
    { from: '../../../dashboard/.storybook/fixtures', to: '/dashboard-fixtures' },
    { from: '../public/samples', to: '/samples' },
    { from: '../../../packages/3d-viewer/fixtures', to: '/viewer-fixtures' },
  ],
  core: { disableTelemetry: true, disableWhatsNewNotifications: true },
};

export default config;
