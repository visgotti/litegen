import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
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
