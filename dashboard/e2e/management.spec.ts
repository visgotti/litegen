import { test, expect, type Page, type APIRequestContext } from '@playwright/test';
import { feature, nextCall } from './support';

// Management pages (Gateway, Budgets, End users, Spend) against the real
// backend. Runs as the master key, so it never creates users and stays out of
// the way of god-test's single-owner assumptions. Every gateway change it makes
// is undone before it finishes, since the backend is shared with other specs.

const MASTER_KEY = process.env.PLAYWRIGHT_MASTER_KEY ?? 'test-master-key-please-rotate';
const API_BASE = process.env.LITEGEN_API_BASE ?? 'http://127.0.0.1:5099';
const RUN = Date.now().toString(36);
const MODEL = `mock/e2e-runtime-${RUN}`;
const END_USER = `cust-e2e-${RUN}`;

const auth = { authorization: `Bearer ${MASTER_KEY}` };

async function generate(request: APIRequestContext, key: string, headers: Record<string, string> = {}) {
  return request.post(`${API_BASE}/v1/images/generations`, {
    headers: { authorization: `Bearer ${key}`, ...headers },
    data: { model: MODEL, prompt: 'a lighthouse at dusk' },
  });
}

async function openAsMaster(page: Page, path: string) {
  await page.goto('/');
  await page.evaluate(key => localStorage.setItem('litegen_api_key', key), MASTER_KEY);
  await page.goto(path);
}

test('management pages drive budgets, end users, spend and gateway config', async ({ page, request }) => {
  // ─── Gateway: add a runtime model through the page ────────────────────────
  await feature('Gateway — add a runtime model; refuse a bad one; disable and enable it');
  await openAsMaster(page, '/gateway');
  await page.getByTestId('gateway-model-json').fill(JSON.stringify({
    id: MODEL,
    provider: 'mock',
    media_type: 'image',
    display_name: 'E2E runtime model',
    description: 'Added by the management e2e spec',
    pricing: { base_cost_usd: 2.0 },
    capabilities: { text_to_image: true },
    prompt: { required: true, max_length: 500 },
  }));
  await page.getByTestId('gateway-model-create').click();
  const modelRow = page.getByTestId(`gateway-model-${MODEL}`);
  await expect(modelRow).toBeVisible({ timeout: 10_000 });
  await expect(modelRow).toContainText('runtime');

  // An invalid schema is refused with the server's message.
  await page.getByTestId('gateway-model-json').fill('{"id": "mock/broken"}');
  const refusedCreate = nextCall(page, 'POST', /^\/v1\/admin\/models$/);
  await page.getByTestId('gateway-model-create').click();
  const refusal = await refusedCreate;
  expect(refusal.status(), 'the server refused the schema').toBeGreaterThanOrEqual(400);
  const refusalMessage = ((await refusal.json()) as { error: { message: string } }).error.message;
  await expect(page.getByTestId('gateway-models-error')).toContainText(refusalMessage);
  expect((await request.get(`${API_BASE}/v1/models/mock/broken`, { headers: auth })).status(), 'nothing was added').toBe(404);

  // Disable, then enable again — and the gateway follows: a disabled model is
  // neither served nor run.
  await page.getByTestId(`gateway-model-toggle-${MODEL}`).click();
  await expect(modelRow).toContainText('disabled');
  expect((await request.get(`${API_BASE}/v1/models/${encodeURIComponent(MODEL)}`, { headers: auth })).status(), 'a disabled model is not served').toBe(404);
  expect((await generate(request, MASTER_KEY)).status(), 'nor run').toBeGreaterThanOrEqual(400);
  await page.getByTestId(`gateway-model-toggle-${MODEL}`).click();
  await expect(modelRow).toContainText('enabled');

  // The runtime model is served to callers immediately.
  const served = await request.get(`${API_BASE}/v1/models/${encodeURIComponent(MODEL)}`, { headers: auth });
  expect(served.status()).toBe(200);

  // ─── Gateway: provider config (api_base only, no secret) then reset ───────
  await feature('Gateway — a provider\'s base URL set at runtime, then reset');
  // Settings the form has no field for, set through the API: a save from the
  // form changes only what the form shows, so they survive it.
  const viaApi = await request.put(`${API_BASE}/v1/admin/providers/openai`, {
    headers: auth,
    data: { extra_headers: { 'X-E2E-Gateway': 'e2e-token' }, model_mapping: { 'gpt-image-1': 'e2e-mapped-model' }, region: 'e2e-region', options: { e2e: true } },
  });
  expect(viaApi.status(), 'the API took the settings').toBe(200);
  await page.getByTestId('gateway-tab-providers').click();
  await page.getByTestId('gateway-provider-edit-openai').click();
  await page.getByTestId('gateway-provider-base').fill('https://openai-proxy.e2e.invalid/v1');
  await page.getByTestId('gateway-provider-save').click();
  const openai = page.getByTestId('gateway-provider-openai');
  await expect(openai).toContainText('openai-proxy.e2e.invalid');
  await expect(openai).toContainText('runtime');
  const providerCfg = await (await request.get(`${API_BASE}/v1/admin/providers/openai`, { headers: auth })).json();
  expect(JSON.stringify(providerCfg), 'the server holds the base URL').toContain('openai-proxy.e2e.invalid');
  expect(providerCfg, 'and every setting the form does not show').toMatchObject({
    extra_header_names: ['X-E2E-Gateway'],
    model_mapping: { 'gpt-image-1': 'e2e-mapped-model' },
    region: 'e2e-region',
    options: { e2e: true },
  });
  await page.getByTestId('gateway-provider-reset-openai').click();
  await expect(openai).not.toContainText('openai-proxy.e2e.invalid');
  const resetCfg = await (await request.get(`${API_BASE}/v1/admin/providers/openai`, { headers: auth })).json();
  expect(JSON.stringify(resetCfg), 'and has dropped it').not.toContain('openai-proxy.e2e.invalid');

  // ─── Gateway: routes ──────────────────────────────────────────────────────
  await feature('Gateway — routes: an unknown provider refused, a route saved, then reset');
  await page.getByTestId('gateway-tab-routes').click();
  await expect(page.getByTestId('gateway-routes-source')).toHaveText('config');
  await page.getByTestId('gateway-routes-json').fill('[{"model": "nope/*", "deployments": [{"provider": "not-a-provider"}]}]');
  await page.getByTestId('gateway-routes-save').click();
  await expect(page.getByTestId('gateway-routes-error')).toContainText('unknown provider');
  await page.getByTestId('gateway-routes-json').fill(`[{"model": "${MODEL}", "deployments": [{"provider": "mock"}]}]`);
  await page.getByTestId('gateway-routes-save').click();
  await expect(page.getByTestId('gateway-routes-source')).toHaveText('runtime');
  await page.getByTestId('gateway-routes-reset').click();
  await expect(page.getByTestId('gateway-routes-source')).toHaveText('config');

  // ─── Gateway: settings ────────────────────────────────────────────────────
  await feature('Gateway — settings validated and saved');
  await page.getByTestId('gateway-tab-settings').click();
  await page.getByTestId('gateway-eu-budget').fill('abc');
  await page.getByTestId('gateway-settings-save').click();
  await expect(page.getByTestId('gateway-settings-error')).toBeVisible();
  await page.getByTestId('gateway-eu-budget').fill('100');
  await page.getByTestId('gateway-eu-duration').fill('30d');
  await page.getByTestId('gateway-settings-save').click();
  await expect(page.getByTestId('gateway-settings-error')).toHaveCount(0);
  const settings = await (await request.get(`${API_BASE}/v1/admin/settings`, { headers: auth })).json();
  expect(settings.settings.default_end_user_max_budget_usd).toBe(100);
  await page.getByTestId('gateway-eu-budget').fill('');
  await page.getByTestId('gateway-eu-duration').fill('');
  await page.getByTestId('gateway-settings-save').click();
  await expect(page.getByTestId('gateway-effective-markup')).toContainText('0%');
  // Sealing provider credentials at rest needs a secrets key, which this
  // backend has not: the server says so, in its words.
  await page.getByTestId('gateway-encrypt-credentials').selectOption('on');
  const sealRefused = nextCall(page, 'PUT', /^\/v1\/admin\/settings$/);
  await page.getByTestId('gateway-settings-save').click();
  const sealAnswer = await sealRefused;
  expect(sealAnswer.status()).toBe(400);
  const sealSaid = ((await sealAnswer.json()) as { error: { message: string } }).error.message;
  await expect(page.getByTestId('gateway-settings-error')).toContainText(sealSaid);
  await page.getByTestId('gateway-encrypt-credentials').selectOption('');

  // ─── Budgets: a key budget set through the page is enforced ───────────────
  await feature('Budgets — a key budget set on the page is enforced by the gateway');
  const created = await request.post(`${API_BASE}/v1/keys`, { headers: auth, data: { name: `e2e-budget-${RUN}` } });
  expect(created.ok()).toBeTruthy();
  const { id: keyId, key: rawKey } = await created.json();

  await page.goto('/budgets');
  await page.getByTestId('key-select').selectOption(keyId);
  const editor = page.getByTestId('key-limits');
  await expect(editor.getByTestId('key-limits-state')).toHaveText('no limits');
  await page.getByTestId('key-limits-soft').fill('9');
  await page.getByTestId('key-limits-max').fill('3');
  await page.getByTestId('key-limits-save').click();
  await expect(page.getByTestId('key-limits-error')).toContainText('must not exceed');
  await page.getByTestId('key-limits-soft').fill('1');
  await page.getByTestId('key-limits-duration').selectOption('1d');
  await page.getByTestId('key-limits-save').click();
  await expect(editor.getByTestId('key-limits-state')).toHaveText('limits set');
  await expect(page.getByTestId('limits-row-key')).toBeVisible();

  const tagged = { 'x-litegen-end-user': END_USER, 'x-litegen-tags': 'e2e,launch' };
  expect((await generate(request, rawKey, tagged)).status()).toBe(200);
  const refused = await generate(request, rawKey, tagged);
  expect(refused.status()).toBe(402);
  expect((await refused.json()).error.type).toBe('budget_exceeded');

  await page.reload();
  await page.getByTestId('key-select').selectOption(keyId);
  await expect(page.getByTestId('key-limits-meter-spend')).toHaveText('$2.00');
  await expect(page.getByTestId('key-limits-alerted')).toBeVisible();
  await page.getByTestId('key-limits-reset').click();
  await expect(page.getByTestId('key-limits-meter-spend')).toHaveText('$0.00');
  expect((await generate(request, rawKey, tagged)).status()).toBe(200);

  // ─── End users: auto-registered, blocked, budgeted ────────────────────────
  await feature('End users — auto-registered, blocked (403), budgeted, deleted');
  await page.goto('/end-users');
  const euRow = page.getByTestId(`end-user-row-${END_USER}`);
  await expect(euRow).toBeVisible({ timeout: 10_000 });
  await page.getByTestId(`end-user-block-${END_USER}`).click();
  await expect(page.getByTestId(`end-user-status-${END_USER}`)).toHaveText('blocked');
  const blocked = await generate(request, rawKey, { 'x-litegen-end-user': END_USER });
  expect(blocked.status()).toBe(403);
  expect((await blocked.json()).error.type).toBe('end_user_blocked');
  await page.getByTestId(`end-user-unblock-${END_USER}`).click();
  await expect(page.getByTestId(`end-user-status-${END_USER}`)).toHaveText('active');

  const vip = `vip-${RUN}`;
  await page.getByTestId('end-user-new-id').fill(vip);
  await page.getByTestId('end-user-new-alias').fill('VIP customer');
  await page.getByTestId('end-user-new-budget').fill('5');
  await page.getByTestId('end-user-create').click();
  await expect(page.getByTestId(`end-user-row-${vip}`)).toContainText('VIP customer');
  await page.getByTestId(`end-user-limits-${vip}`).click();
  await expect(page.getByTestId('end-user-limits-state')).toHaveText('limits set');
  await page.getByTestId('end-user-limits-rpm').fill('30');
  await page.getByTestId('end-user-limits-save').click();
  await expect(page.getByTestId('end-user-limits-rpm')).toHaveValue('30');
  const vipLimits = await (await request.get(`${API_BASE}/v1/end-users/${vip}/limits`, { headers: auth })).json();
  expect(vipLimits.rpm_limit).toBe(30);
  expect(vipLimits.max_budget_usd).toBe(5);
  await page.getByTestId(`end-user-delete-${vip}`).click();
  await expect(page.getByTestId(`end-user-row-${vip}`)).toHaveCount(0);
  expect((await request.get(`${API_BASE}/v1/end-users/${vip}`, { headers: auth })).status(), 'deleted on the server').toBe(404);

  // ─── Spend: grouped and itemised ──────────────────────────────────────────
  await feature('Spend — totals and groupings for one end user');
  await page.goto('/spend');
  await page.getByTestId('spend-end-user').fill(END_USER);
  await page.getByTestId('spend-apply').click();
  await expect(page.getByTestId('spend-requests')).toHaveText('2', { timeout: 10_000 });
  await expect(page.getByTestId('spend-total')).toHaveText('$4.00');
  await page.getByTestId('spend-group-by').selectOption('tag');
  await expect(page.getByTestId('spend-report-row')).toHaveCount(2);
  await expect(page.getByTestId('spend-report')).toContainText('launch');
  await expect(page.getByTestId('spend-log-row').first()).toContainText(END_USER);
  await expect(page.getByTestId('spend-log-row').first()).toContainText('e2e, launch');
  await page.getByTestId('spend-group-by').selectOption('model');
  await expect(page.getByTestId('spend-report')).toContainText(MODEL);

  // ─── Clean up the shared backend ──────────────────────────────────────────
  await page.goto('/gateway');
  await page.getByTestId(`gateway-model-delete-${MODEL}`).click();
  await expect(page.getByTestId(`gateway-model-${MODEL}`)).toHaveCount(0);
  await request.delete(`${API_BASE}/v1/keys/${keyId}`, { headers: auth });
  await request.delete(`${API_BASE}/v1/end-users/${END_USER}`, { headers: auth });
});

test('Logs, the trace and Generations show a charge with every digit it has', async ({ page, request }) => {
  await feature('Costs — a sub-cent charge shown whole in Logs, the trace and Generations');
  // Every mock model costs $0 or at least half a cent, so the server's real
  // answers come back with a vendor's sub-cent charge put in (fal flux-2-klein
  // at 256x256 is $0.00033), which four fixed decimals showed as "$0.0003".
  const PRICE = 0.00033;
  const made = await request.post(`${API_BASE}/v1/videos/generations`, {
    headers: auth,
    data: { model: 'mock/video-gen', prompt: 'a sub-cent clip' },
  });
  expect(made.status(), await made.text()).toBe(200);
  const { id } = (await made.json()) as { id: string };
  const priced = (rows: Array<{ cost_usd?: number | null }>) => rows.forEach(r => { r.cost_usd = PRICE; });
  await page.route(/\/v1\/(logs|generations)\?/, async route => {
    const response = await route.fetch();
    const body = (await response.json()) as { data?: Array<{ cost_usd?: number | null }> };
    if (response.ok() && Array.isArray(body.data)) priced(body.data);
    await route.fulfill({ response, json: body });
  });

  await openAsMaster(page, '/logs');
  const row = page.locator('[data-testid^="logs-row-"]').first();
  await expect(row).toContainText('$0.00033', { timeout: 10_000 });
  await row.click();
  await page.getByTestId('trace-tab-params').click();
  await expect(page.getByTestId('trace-panel')).toContainText('$0.00033');
  await page.getByTestId('trace-panel-close').click();

  await page.goto('/generations');
  await expect(page.getByTestId(`gen-row-${id}`)).toContainText('$0.00033', { timeout: 10_000 });
});

test('the Overview shows its total and its per-provider cost with every digit they have', async ({ page, request }) => {
  await feature('Overview — a sub-cent total and provider cost shown whole');
  // As above: the server's real answers with a vendor's sub-cent charge put in.
  const PRICE = 0.00033;
  expect((await request.post(`${API_BASE}/v1/images/generations`, {
    headers: auth,
    data: { model: 'mock/image-gen', prompt: 'a sub-cent picture' },
  })).status()).toBe(200);
  await page.route(/\/v1\/stats(\?.*)?$/, async route => {
    const response = await route.fetch();
    const body = (await response.json()) as { total_cost_usd: number; providers_used: Array<{ cost_usd: number }> };
    if (response.ok()) {
      body.total_cost_usd = PRICE;
      body.providers_used.forEach(p => { p.cost_usd = PRICE; });
    }
    await route.fulfill({ response, json: body });
  });

  await openAsMaster(page, '/');
  await expect(page.locator('.stat-card').filter({ hasText: 'Total Cost' }).locator('.value')).toHaveText('$0.00033', { timeout: 10_000 });
  await expect(page.locator('.chart-container').filter({ hasText: 'Cost by Provider' })).toContainText(': $0.00033');
});

test("clearing a key's quota, RPM limit and webhook on the Keys page clears them", async ({ page, request }) => {
  await feature('Keys — a quota, an RPM limit and a webhook cleared in the edit form');
  const created = await request.post(`${API_BASE}/v1/keys`, {
    headers: auth,
    data: { name: `e2e-clear-${RUN}`, token_quota: 100, rpm_limit: 60, webhook_url: 'https://hooks.example.com/e2e-clear' },
  });
  expect(created.ok()).toBeTruthy();
  const { id } = (await created.json()) as { id: string };
  try {
    await openAsMaster(page, '/keys');
    // The quota is a USD budget cap (`token_quota`), what the key spent USD too.
    await expect(page.getByTestId(`key-quota-${id}`)).toHaveText('$0.00 / $100.00', { timeout: 10_000 });
    await expect(page.getByTestId(`key-rpm-${id}`)).toHaveText('60');
    await expect(page.getByTestId(`key-test-webhook-${id}`)).toBeVisible();

    await page.getByTestId(`edit-key-${id}`).click();
    await page.getByTestId('edit-token-quota').fill('');
    await page.getByTestId('edit-rpm-limit').fill('');
    await page.getByTestId('edit-webhook-url').fill('');
    const saved = nextCall(page, 'PATCH', new RegExp(`^/v1/keys/${id}$`));
    await page.getByTestId('save-edit-btn').click();
    const answer = await saved;
    expect(answer.request().postDataJSON()).toMatchObject({ token_quota: null, rpm_limit: null, webhook_url: null });
    expect(answer.status()).toBe(200);
    await expect(page.getByTestId(`edit-row-${id}`)).toHaveCount(0);

    // The row shows what the server now holds: no quota, no limit, no hook.
    await expect(page.getByTestId(`key-quota-${id}`)).toHaveText('—', { timeout: 10_000 });
    await expect(page.getByTestId(`key-rpm-${id}`)).toHaveText('—');
    await expect(page.getByTestId(`key-test-webhook-${id}`)).toHaveCount(0);
    const stored = (await (await request.get(`${API_BASE}/v1/keys/${id}`, { headers: auth })).json()) as Record<string, unknown>;
    expect(stored.token_quota ?? null, 'the quota is gone on the server').toBeNull();
    expect(stored.rpm_limit ?? null, 'the RPM limit too').toBeNull();
    expect(stored.webhook_url ?? null, 'and the webhook').toBeNull();
  } finally {
    await request.delete(`${API_BASE}/v1/keys/${id}`, { headers: auth });
  }
});

test("opening the Users page keeps the master key's login", async ({ page }) => {
  // The master key holds no session, so /v1/auth/me answers it 401, which
  // signs the dashboard out: no page asks it in the API-key flow. What
  // /v1/users answers the master key is the server's to say.
  await page.goto('/');
  await page.evaluate(key => localStorage.setItem('litegen_api_key', key), MASTER_KEY);
  const asked: string[] = [];
  page.on('request', r => asked.push(new URL(r.url()).pathname));
  const listed = nextCall(page, 'GET', /^\/v1\/users$/);
  await page.goto('/users');
  await listed;
  await page.waitForLoadState('networkidle');
  await expect(page).toHaveURL(/\/users$/);
  await expect(page.getByTestId('users-invite-btn')).toBeVisible();
  expect(await page.evaluate(() => localStorage.getItem('litegen_api_key')), 'the key is still the login').toBe(MASTER_KEY);
  expect(asked.filter(p => p === '/v1/auth/me'), 'nothing asks /v1/auth/me with the master key').toEqual([]);
});
