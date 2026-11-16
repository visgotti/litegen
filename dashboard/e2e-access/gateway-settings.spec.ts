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
  for (const model of ['mock/image-gen', 'mock/expensive-image']) {
    const enable = page.getByTestId(`provider-model-enable-${model}`);
    await enable.check();
    await expect(enable).toBeChecked();
  }
  await page.goto('/keys');
  await page.getByTestId('new-key-name').fill('cache-key');
  await page.getByTestId('new-key-scopes').fill('generate,read');
  await page.getByTestId('create-key-btn').click();
  return (await page.getByTestId('key-secret').textContent())!.trim();
}

/** An image answer's `X-Litegen-Cache`. */
async function cacheSays(request: APIRequestContext, key: string, prompt: string, model = 'mock/image-gen'): Promise<string> {
  const r = await request.post(`${BACKEND}/v1/images/generations`, {
    headers: { authorization: `Bearer ${key}` },
    data: { model, prompt },
  });
  expect(r.status(), await r.text()).toBe(200);
  return r.headers()['x-litegen-cache'];
}

/** An image answer's id: a hit's is the earlier generation's. */
async function answerId(request: APIRequestContext, key: string, prompt: string, model: string): Promise<string> {
  const r = await request.post(`${BACKEND}/v1/images/generations`, {
    headers: { authorization: `Bearer ${key}` },
    data: { model, prompt },
  });
  expect(r.status(), await r.text()).toBe(200);
  return ((await r.json()) as { id: string }).id;
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

  // Every range is the server's, read from the settings it serves (`limits`):
  // a value outside one never leaves the page.
  const ranges = before.limits as Record<string, { min: number; max: number }>;
  const isPut = (r: { method(): string; url(): string }) => r.method() === 'PUT' && new URL(r.url()).pathname === '/v1/admin/settings';
  let puts = 0;
  adminPage.on('request', r => { if (isPut(r)) puts += 1; });
  await adminPage.getByTestId('gateway-cache-max-bytes').fill('1024');
  await adminPage.getByTestId('gateway-settings-save').click();
  await expect(adminPage.getByTestId('gateway-settings-error'))
    .toContainText(`from ${ranges.cache_max_bytes.min} to ${ranges.cache_max_bytes.max}`);
  await adminPage.getByTestId('gateway-cache-max-bytes').fill('2097152');
  await adminPage.getByTestId('gateway-retries').fill('9');
  await adminPage.getByTestId('gateway-settings-save').click();
  await expect(adminPage.getByTestId('gateway-settings-error'))
    .toContainText(`Retries must be a whole number from ${ranges.retry_max_retries.min} to ${ranges.retry_max_retries.max}`);
  expect(puts, 'nothing was sent').toBe(0);

  // Not a copy: served narrower, the page holds to the narrower range.
  const narrowed = async (route: import('@playwright/test').Route) => {
    if (route.request().method() !== 'GET') return route.continue();
    const response = await route.fetch();
    const body = (await response.json()) as { limits: Record<string, { min: number; max: number }> };
    body.limits.retry_max_retries = { min: 0, max: 3 };
    return route.fulfill({ response, json: body });
  };
  await adminPage.route('**/v1/admin/settings', narrowed);
  await adminPage.reload();
  await adminPage.getByTestId('gateway-tab-settings').click();
  await adminPage.getByTestId('gateway-retries').fill('4');
  await adminPage.getByTestId('gateway-settings-save').click();
  await expect(adminPage.getByTestId('gateway-settings-error')).toContainText('Retries must be a whole number from 0 to 3');
  expect(puts, 'nothing was sent').toBe(0);
  await adminPage.unroute('**/v1/admin/settings', narrowed);
  await adminPage.reload();
  await adminPage.getByTestId('gateway-tab-settings').click();

  // A rule between two settings is the server's to say, in its words.
  await adminPage.getByTestId('gateway-backoff').fill('2000');
  await adminPage.getByTestId('gateway-max-backoff').fill('1000');
  saved = adminPage.waitForResponse(r => isPut(r.request()));
  await adminPage.getByTestId('gateway-settings-save').click();
  const refused = await saved;
  expect(refused.status()).toBe(400);
  const said = ((await refused.json()) as { error: { message: string } }).error.message;
  expect(said).toContain('retry_max_backoff_ms');
  await expect(adminPage.getByTestId('gateway-settings-error')).toContainText(said);
  await adminPage.getByTestId('gateway-max-backoff').fill('');

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
  expect(settings.effective_cache).toEqual({ enabled: true, ttl_seconds: 600, max_items: 50, max_bytes: 2_097_152, charge_hits: true });
  expect(settings.effective_retries).toEqual({ max_retries: 1, backoff_ms: 200, max_backoff_ms: 10000 });
  expect(settings.effective_vendor_timeout_seconds).toBe(30);
  expect(settings.effective_circuit_breaker).toEqual({ threshold: 3, open_for_seconds: 30 });

  await feature('The cache answers the repeat, and Clear empties it');
  expect(await cacheSays(request, key, `a fox ${RUN}`)).toBe('miss');
  expect(await cacheSays(request, key, `a fox ${RUN}`)).toBe('hit');

  await feature('A cache hit is marked in the logs, charged its price until Charge cache hits is off');
  const priced = 'mock/expensive-image';
  expect(await cacheSays(request, key, `a priced fox ${RUN}`, priced)).toBe('miss');
  const servedFrom = await answerId(request, key, `a priced fox ${RUN}`, priced);
  await page.goto('/logs');
  const hitRow = page.locator('tr[data-testid^="logs-row-"]').filter({ has: page.locator('[data-testid^="logs-cache-hit-"]') }).first();
  await expect(hitRow).toContainText('$5.00', { timeout: 10_000 });
  // The hit's trace names the generation it was served; the log keeps no copy of the image.
  await hitRow.click();
  await expect(page.getByTestId('trace-visual-from-cache')).toHaveText(`Served from the cache: generation ${servedFrom}`);
  await expect(page.getByTestId('trace-visual-image')).toHaveCount(0);
  await page.getByTestId('trace-panel-close').click();
  await page.goto('/spend');
  await expect(page.getByTestId('spend-log-cache-hit').first()).toBeVisible({ timeout: 10_000 });
  await adminPage.reload();
  await adminPage.getByTestId('gateway-tab-settings').click();
  await expect(adminPage.getByTestId('gateway-cache-effective')).toContainText('hits charged');
  await adminPage.getByTestId('gateway-cache-charge-hits').selectOption('off');
  saved = adminPage.waitForResponse(r => isPut(r.request()));
  await adminPage.getByTestId('gateway-settings-save').click();
  const freeHits = await saved;
  expect(freeHits.status(), await freeHits.text()).toBe(200);
  expect(freeHits.request().postDataJSON().cache_charge_hits).toBe(false);
  await expect(adminPage.getByTestId('gateway-cache-effective')).toContainText('hits free');
  expect(await cacheSays(request, key, `a priced fox ${RUN}`, priced)).toBe('hit');
  const latest = (await (await request.get(`${BACKEND}/v1/logs?page=1&per_page=1`, { headers: { authorization: `Bearer ${key}` } })).json()) as {
    data: Array<{ id: string; cache_hit?: boolean; cost_usd: number }>;
  };
  expect(latest.data[0], 'a free hit, still marked').toMatchObject({ cache_hit: true, cost_usd: 0 });
  await adminPage.getByTestId('gateway-cache-charge-hits').selectOption('');
  saved = adminPage.waitForResponse(r => isPut(r.request()));
  await adminPage.getByTestId('gateway-settings-save').click();
  expect((await saved).status()).toBe(200);
  await expect(adminPage.getByTestId('gateway-cache-effective')).toContainText('hits charged');

  await feature("A hit's trace deletes the generation it was served: never served again, and the hits' log entries go");
  const freeHit = latest.data[0].id;
  await page.goto('/logs');
  await page.getByTestId(`logs-row-${freeHit}`).click();
  await expect(page.getByTestId('trace-visual-from-cache')).toHaveText(`Served from the cache: generation ${servedFrom}`);
  let asked = '';
  page.once('dialog', dialog => {
    asked = dialog.message();
    void dialog.accept();
  });
  await page.getByTestId('trace-delete-files').click();
  await expect(page.getByText('Files and request deleted')).toBeVisible();
  expect(asked).toContain(`from generation ${servedFrom}`);
  await page.getByTestId('trace-panel-close').click();
  const hitEntry = await request.get(`${BACKEND}/v1/logs/${freeHit}/artifact`, { headers: { authorization: `Bearer ${key}` } });
  expect(hitEntry.status(), "the hit's log entry went with its generation").toBe(404);
  expect(await cacheSays(request, key, `a priced fox ${RUN}`, priced)).toBe('miss');
  await adminPage.reload();
  await adminPage.getByTestId('gateway-tab-settings').click();
  await expect(adminPage.getByTestId('gateway-cache-effective')).not.toContainText(' 0 cached now');
  await adminPage.getByTestId('gateway-cache-clear').click();
  await expect(adminPage.getByTestId('gateway-cache-effective')).toContainText(' 0 cached now');
  expect(await cacheSays(request, key, `a fox ${RUN}`)).toBe('miss');

  await feature('Provider credentials sealed at rest, switched on and back off on the page');
  // This backend has a secrets key, so the server takes the setting.
  const seal = adminPage.getByTestId('gateway-encrypt-credentials');
  await expect(seal).toHaveValue('');
  await seal.selectOption('on');
  saved = adminPage.waitForResponse(r => isPut(r.request()));
  await adminPage.getByTestId('gateway-settings-save').click();
  const sealed = await saved;
  expect(sealed.status(), await sealed.text()).toBe(200);
  expect(sealed.request().postDataJSON().encrypt_provider_credentials).toBe(true);
  expect((await (await request.get(`${BACKEND}/v1/admin/settings`, { headers: auth })).json()).settings.encrypt_provider_credentials).toBe(true);
  await adminPage.reload();
  await adminPage.getByTestId('gateway-tab-settings').click();
  await expect(seal).toHaveValue('on');
  await seal.selectOption('');
  saved = adminPage.waitForResponse(r => isPut(r.request()));
  await adminPage.getByTestId('gateway-settings-save').click();
  expect((await saved).status()).toBe(200);
  expect((await (await request.get(`${BACKEND}/v1/admin/settings`, { headers: auth })).json()).settings.encrypt_provider_credentials ?? null).toBeNull();

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
