/// <reference types="vitest/config" />
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig, searchForWorkspaceRoot } from 'vite';
import react from '@vitejs/plugin-react';

const here = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  // Relative asset URLs: the tool is a static site that works under any
  // path, tools.litegen.ai/3d-converter/ included. It has no router.
  base: './',
  plugins: [react()],
  // @litegen/3d-viewer is linked source with its own dev copies of these
  // (for its own tests): the app's copies serve it, so there is one React and
  // one three.js at runtime.
  resolve: { dedupe: ['react', 'react-dom', 'three', 'lucide-react'] },
  // The converter package awaits its wasm at the top level, which module
  // workers allow and classic (iife) ones do not.
  worker: { format: 'es' },
  // Its glue finds the .wasm through `new URL(…, import.meta.url)`, which
  // pre-bundling would rewrite away.
  optimizeDeps: { exclude: ['@litegen/3d-converter'] },
  build: { target: 'es2022' },
  server: {
    port: 5181,
    strictPort: true,
    // The `file:` dependencies resolve to packages/3d-converter/js and
    // packages/3d-viewer, outside this app.
    fs: {
      allow: [
        searchForWorkspaceRoot(process.cwd()),
        path.resolve(here, '../../packages/3d-converter/js'),
        path.resolve(here, '../../packages/3d-viewer'),
      ],
    },
  },
  preview: { port: 5182, strictPort: true },
  test: { include: ['src/**/*.test.ts'] },
});
