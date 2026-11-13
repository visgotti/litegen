import { test, expect, type APIRequestContext, type Page } from '@playwright/test';
import { feature } from '../e2e/support';

// The cache, retries, the vendor timeout and the circuit breaker set on the
// Gateway page (docs/gateway-settings.md), against a real hosted backend
// (playwright.access.config.ts).

const BACKEND = 'http://127.0.0.1:5399';
const MASTER_KEY = 'access-e2e-master';
const PW = 'Password123!';
const RUN = Date.now().toString(36);

/** An org owner with the mock image model turned on, and an API key of theirs. */
async function ownerKey(page: Page): Promise<string> {
  await page.goto('/signup');
  await page.getByTestId('signup-org-name').fill('Cache Co');
  await page.getByTestId('signup-email').fill(`cache-${RUN}@cache.example`);
  await page.getByTestId('signup-password').fill(PW);
  await page.getByTestId('signup-confirm-password').fill(PW);
  await page.getByTestId('signup-submit').click();
  await expect(page.getByTestId('org-switcher')).toBeVisible({ timeout: 15_000 });
  await page.goto('/providers');
  await page.getByTestId('provider-toggle-mock').click();
  await page.getByTestId('provider-key-field-mock-key-0').fill('sk-mock-cache');
  await page.getByTestId('provider-key-save-mock').click();
  await expect(page.getByTestId('provider-section-mock')).toContainText('configured', { timeout: 15_000 });
  const enable = page.getByTestId('provider-model-enable-mock/image-gen');
  await enable.check();
  await expect(enable).toBeChecked();
  await page.goto('/keys');
  await page.getByTestId('new-key-name').fill('cache-key');
  await page.getByTestId('new-key-scopes').fill('generate,read');
  await page.getByTestId('create-key-btn').click();
  return (await page.getByTestId('key-secret').textContent())!.trim();
}

/** An image answer's `X-Litegen-Cache`. */
async function cacheSays(request: APIRequestContext, key: string, prompt: string): Promise<string> {
  const r = await request.post(`${BACKEND}/v1/images/generations`, {
    headers: { authorization: `Bearer ${key}` },
    data: { model: 'mock/image-gen', prompt },
  });
  expect(r.status(), await r.text()).toBe(200);
  return r.headers()['x-litegen-cache'];
}

test('the cache, retries, timeout and breaker, set on the Gateway page', async ({ page, request, browser }) => {
  await feature('An org with a key, to fill the cache with');
  const key = await ownerKey(page);
  expect(await cacheSays(request, key, `a fox ${RUN}`)).toBe('bypass');

  await feature('The gateway admin turns the cache on and tunes retries, the timeout and the breaker');
  const admin = await browser.newContext({ baseURL: 'http://127.0.0.1:5374/' });
  const adminPage = await admin.newPage();
  await adminPage.addInitScript(k => localStorage.setItem('litegen_api_key', k), MASTER_KEY);
  await adminPage.goto('/gateway');
  await adminPage.getByTestId('gateway-tab-settings').click();
  await expect(adminPage.getByTestId('gateway-cache-effective')).toContainText('off');
  await expect(adminPage.getByTestId('gateway-retry-effective')).toContainText('2 retries');
  await expect(adminPage.getByTestId('gateway-breaker-effective')).toContainText('after 5 failures');

  await feature('A setting the API set survives a save from the page (registry NEW 5)');
  const auth = { authorization: `Bearer ${MASTER_KEY}` };
  const before = await (await request.get(`${BACKEND}/v1/admin/settings`, { headers: auth })).json();
  const put = await request.put(`${BACKEND}/v1/admin/settings`, { headers: auth, data: { ...before.settings, cache_max_bytes: 2_097_152 } });
  expect(put.status(), await put.text()).toBe(200);
  await adminPage.reload();
  await adminPage.getByTestId('gateway-tab-settings').click();
  await expect(adminPage.getByTestId('gateway-cache-max-bytes')).toHaveValue('2097152');
  let saved = adminPage.waitForResponse(r => r.request().method() === 'PUT' && new URL(r.url()).pathname === '/v1/admin/settings');
  await adminPage.getByTestId('gateway-settings-save').click();
  expect((await saved).status()).toBe(200);
  const after = await (await request.get(`${BACKEND}/v1/admin/settings`, { headers: auth })).json();
  expect(after.settings.cache_max_bytes, 'the save sent it back').toBe(2_097_152);
  expect(after.effective_cache.max_bytes).toBe(2_097_152);
  await expect(adminPage.getByTestId('gateway-cache-effective')).toContainText('in 2 MiB');

  // The memory bound's range is the server's: the page shows its 400.
  await adminPage.getByTestId('gateway-cache-max-bytes').fill('1024');
  saved = adminPage.waitForResponse(r => r.request().method() === 'PUT' && new URL(r.url()).pathname === '/v1/admin/settings');
  await adminPage.getByTestId('gateway-settings-save').click();
  expect((await saved).status()).toBe(400);
  await expect(adminPage.getByTestId('gateway-settings-error')).toContainText('cache_max_bytes must be between 1048576 and 68719476736');
  await adminPage.getByTestId('gateway-cache-max-bytes').fill('2097152');

  // A value the gateway would refuse never leaves the page.
  await adminPage.getByTestId('gateway-retries').fill('9');
  await adminPage.getByTestId('gateway-settings-save').click();
  await expect(adminPage.getByTestId('gateway-settings-error')).toContainText('Retries must be a whole number from 0 to 5');

  await adminPage.getByTestId('gateway-cache-enabled').selectOption('on');
  await adminPage.getByTestId('gateway-cache-ttl').fill('600');
  await adminPage.getByTestId('gateway-cache-size').fill('50');
  await adminPage.getByTestId('gateway-retries').fill('1');
  await adminPage.getByTestId('gateway-backoff').fill('200');
  await adminPage.getByTestId('gateway-vendor-timeout').fill('30');
  await adminPage.getByTestId('gateway-breaker-threshold').fill('3');
  await adminPage.getByTestId('gateway-settings-save').click();
  await expect(adminPage.getByTestId('gateway-settings-error')).toHaveCount(0);
  await expect(adminPage.getByTestId('gateway-cache-effective')).toContainText('on · answers kept 600 s · up to 50');
  await expect(adminPage.getByTestId('gateway-retry-effective')).toContainText('1 retry · waits up to 200 ms');
  await expect(adminPage.getByTestId('gateway-retry-effective')).toContainText('vendor calls end after 30 s');
  await expect(adminPage.getByTestId('gateway-breaker-effective')).toContainText('after 3 failures');
  const settings = await (await request.get(`${BACKEND}/v1/admin/settings`, { headers: { authorization: `Bearer ${MASTER_KEY}` } })).json();
  expect(settings.effective_cache).toEqual({ enabled: true, ttl_seconds: 600, max_items: 50, max_bytes: 2_097_152 });
  expect(settings.effective_retries).toEqual({ max_retries: 1, backoff_ms: 200, max_backoff_ms: 10000 });
  expect(settings.effective_vendor_timeout_seconds).toBe(30);
  expect(settings.effective_circuit_breaker).toEqual({ threshold: 3, open_for_seconds: 30 });

  await feature('The cache answers the repeat, and Clear empties it');
  expect(await cacheSays(request, key, `a fox ${RUN}`)).toBe('miss');
  expect(await cacheSays(request, key, `a fox ${RUN}`)).toBe('hit');
  await adminPage.reload();
  await adminPage.getByTestId('gateway-tab-settings').click();
  await expect(adminPage.getByTestId('gateway-cache-effective')).not.toContainText(' 0 cached now');
  await adminPage.getByTestId('gateway-cache-clear').click();
  await expect(adminPage.getByTestId('gateway-cache-effective')).toContainText(' 0 cached now');
  expect(await cacheSays(request, key, `a fox ${RUN}`)).toBe('miss');

  // Back to the config's values, as the next spec on this backend expects.
  for (const id of ['gateway-cache-ttl', 'gateway-cache-size', 'gateway-cache-max-bytes', 'gateway-retries', 'gateway-backoff', 'gateway-vendor-timeout', 'gateway-breaker-threshold']) {
    await adminPage.getByTestId(id).fill('');
  }
  await adminPage.getByTestId('gateway-cache-enabled').selectOption('');
  await adminPage.getByTestId('gateway-settings-save').click();
  await expect(adminPage.getByTestId('gateway-cache-effective')).toContainText('off');
  await expect(adminPage.getByTestId('gateway-cache-effective')).toContainText('in 256 MiB');
  await admin.close();
});
