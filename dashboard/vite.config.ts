import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// Resolve the API base URL at BUILD time. In production VITE_API_URL is set
// (e.g. https://app.litegen.ai/api), so the bundle contains only that string —
// the localhost dev fallback literal is never emitted into a prod build (which
// the deploy localhost-guard would otherwise reject).
const API_BASE = (process.env.VITE_API_URL || 'http://localhost:4000').replace(/\/+$/, '');

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  define: {
    __LITEGEN_API_BASE__: JSON.stringify(API_BASE),
  },
  server: {
    // When VITE_PROXY_TARGET is set (e.g. in Playwright test env), proxy all
    // /v1, /health, /metrics, /openapi, and /mock API calls to the backend
    // so that session cookies are same-origin and SameSite=Lax works correctly.
    proxy: process.env.VITE_PROXY_TARGET
      ? {
          '/v1': { target: process.env.VITE_PROXY_TARGET, changeOrigin: true },
          '/health': { target: process.env.VITE_PROXY_TARGET, changeOrigin: true },
          '/metrics': { target: process.env.VITE_PROXY_TARGET, changeOrigin: true },
          '/openapi.json': { target: process.env.VITE_PROXY_TARGET, changeOrigin: true },
          '/mock': { target: process.env.VITE_PROXY_TARGET, changeOrigin: true },
        }
      : undefined,
  },
})
