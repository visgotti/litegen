import { test, expect } from '@playwright/test';

// Generation call hooks in a real browser (docs/hooks.md). "Acme", a customer's
// web app (e2e-hooks/acme-app.mjs) on another origin than litegen's, calls
// litegen with the customer's own credentials: the browser runs the real CORS
// preflight, and litegen runs the example hooks from examples/hooks/.
// playwright.hooks.config.ts starts the app and both litegen servers.
//
// The app is served from a real local server, not page.route: Chromium refuses
// a page outside the loopback address space a request to 127.0.0.1 (Local
// Network Access) before CORS is even asked.

const TOKEN_LITEGEN = 'http://127.0.0.1:5199'; // custom_auth.py
const COOKIE_LITEGEN = 'http://127.0.0.1:5198'; // cookie_auth.py
const APP = 'http://127.0.0.1:5299';

type Generated = { status: number; body: { data?: unknown[]; error?: { code: string; message: string } } };

/** What the Acme page's own script does on a click. */
const generate = (page: import('@playwright/test').Page, token: string) =>
  page.evaluate((t) => (window as unknown as { generate: (t: string) => Promise<Generated> }).generate(t), token);

test('a page on another origin generates through the custom-auth hook', async ({ page }) => {
  await page.goto(`${APP}/`);

  const ok = await generate(page, 'alice-token');
  expect(ok.status).toBe(200);
  expect(ok.body.data?.length).toBeGreaterThan(0);
  await expect(page.locator('#status')).toContainText('200 image generated');
  await expect(page.locator('#out')).toHaveAttribute('src', /.+/);

  const refused = await generate(page, 'wrong');
  expect(refused.status).toBe(401);
  expect(refused.body.error?.code).toBe('refused_by_hook');
  expect(refused.body.error?.message).toBe('send a valid x-acme-token');
  await expect(page.locator('#status')).toContainText('401 send a valid x-acme-token');
});

test('the preflight lets the custom header through', async ({ request }) => {
  const res = await request.fetch(`${TOKEN_LITEGEN}/v1/images/generations`, {
    method: 'OPTIONS',
    headers: {
      origin: APP,
      'access-control-request-method': 'POST',
      'access-control-request-headers': 'x-acme-token, content-type',
    },
  });
  expect(res.ok()).toBeTruthy();
  const allowed = (res.headers()['access-control-allow-headers'] ?? '').toLowerCase();
  expect(allowed).toContain('x-acme-token');
  expect(res.headers()['access-control-allow-origin']).toBe(APP);
});

test('the cookie hook refuses a cross-site request', async ({ request }) => {
  const call = (origin: string) =>
    request.post(`${COOKIE_LITEGEN}/v1/images/generations`, {
      headers: { cookie: 'acme_session=alice-token', origin },
      data: { model: 'mock/visual-image-gen', prompt: 'a fox' },
    });
  const forged = await call('https://evil.test');
  expect(forged.status()).toBe(403);
  expect((await forged.json()).error.message).toBe('cross-site request from https://evil.test refused');
  const own = await call(APP);
  expect(own.status()).toBe(200);
});
