import { test, expect, type Page, type APIRequestContext } from '@playwright/test';

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
  await page.getByTestId('gateway-model-create').click();
  await expect(page.getByTestId('gateway-models-error')).toBeVisible();

  // Disable, then enable again.
  await page.getByTestId(`gateway-model-toggle-${MODEL}`).click();
  await expect(modelRow).toContainText('disabled');
  await page.getByTestId(`gateway-model-toggle-${MODEL}`).click();
  await expect(modelRow).toContainText('enabled');

  // The runtime model is served to callers immediately.
  const served = await request.get(`${API_BASE}/v1/models/${encodeURIComponent(MODEL)}`, { headers: auth });
  expect(served.status()).toBe(200);

  // ─── Gateway: provider config (api_base only, no secret) then reset ───────
  await page.getByTestId('gateway-tab-providers').click();
  await page.getByTestId('gateway-provider-edit-openai').click();
  await page.getByTestId('gateway-provider-base').fill('https://openai-proxy.e2e.invalid/v1');
  await page.getByTestId('gateway-provider-save').click();
  const openai = page.getByTestId('gateway-provider-openai');
  await expect(openai).toContainText('openai-proxy.e2e.invalid');
  await expect(openai).toContainText('runtime');
  await page.getByTestId('gateway-provider-reset-openai').click();
  await expect(openai).not.toContainText('openai-proxy.e2e.invalid');

  // ─── Gateway: routes ──────────────────────────────────────────────────────
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

  // ─── Budgets: a key budget set through the page is enforced ───────────────
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

  // ─── Spend: grouped and itemised ──────────────────────────────────────────
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
