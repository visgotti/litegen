---
paths:
  - "dashboard/e2e*/**"
  - "dashboard/playwright*.ts"
---

# Browser tests that call litegen from another origin

In Playwright's Chromium, a page served with `page.route(...)` can't `fetch`
litegen on `127.0.0.1`, whatever origin it claims. Chromium's Local Network
Access check blocks the request before CORS is even checked:

- the console says "Permission was denied for this request to access the
  `loopback` address space";
- the fetch throws "Failed to fetch".

Instead, serve the page from a real local HTTP server that starts as a
Playwright `webServer`, like `dashboard/e2e-hooks/acme-app.mjs` in
`playwright.hooks.config.ts`. A different port on the same loopback host is
still a different origin, so the real CORS preflight still runs.

When a cross-origin fetch fails, look at `page.on('console')` before anything
else.
