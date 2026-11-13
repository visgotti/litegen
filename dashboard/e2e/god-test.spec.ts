import { test, expect } from '@playwright/test';
import fs from 'fs';
import { createServer, Server, type IncomingHttpHeaders } from 'http';
import { feature, nextCall } from './support';

const MASTER_KEY = process.env.PLAYWRIGHT_MASTER_KEY ?? 'test-master-key-please-rotate';
// Direct-to-backend base for raw API calls (bypasses the dashboard proxy). Matches
// the webServer port in playwright.config.ts; override with LITEGEN_API_BASE.
const API_BASE = process.env.LITEGEN_API_BASE ?? 'http://127.0.0.1:5099';

/** One POST the receiver got: its JSON body, or the raw text under `raw`. */
type Delivery = { url?: string; headers: IncomingHttpHeaders; body: Record<string, unknown> };

function startWebhookReceiver(port: number): Promise<{ server: Server; received: Delivery[] }> {
  return new Promise((resolve) => {
    const received: Delivery[] = [];
    const server = createServer((req, res) => {
      let body = '';
      req.on('data', (chunk) => (body += chunk));
      req.on('end', () => {
        try {
          received.push({ url: req.url, headers: req.headers, body: JSON.parse(body || '{}') as Record<string, unknown> });
        } catch {
          received.push({ url: req.url, headers: req.headers, body: { raw: body } });
        }
        res.writeHead(200, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ ok: true }));
      });
    });
    server.listen(port, '127.0.0.1', () => resolve({ server, received }));
  });
}

test('clicks every UI feature with real backend, full CRUD round-trip', async ({ page, context }) => {

  // ─── Session-based auth flow ──────────────────────────────────────────────────
  const OWNER_EMAIL = 'owner@litegen.test';
  const OWNER_PW = 'super-secret-password-123';
  const MEMBER_EMAIL = 'member@litegen.test';
  const MEMBER_PW = 'another-strong-pw-456';

  // 1. Sign up as Owner
  await feature('Sign-up — the first user is the owner');
  await page.goto('/signup');
  await page.locator('[data-testid="signup-email"]').fill(OWNER_EMAIL);
  await page.locator('[data-testid="signup-password"]').fill(OWNER_PW);
  await page.locator('[data-testid="signup-confirm-password"]').fill(OWNER_PW);
  await page.locator('[data-testid="signup-submit"]').click();
  await page.waitForURL('**/');
  await expect(page.locator('[data-testid="user-menu-email"]')).toContainText(OWNER_EMAIL, { timeout: 10_000 });
  await expect(page.locator('[data-testid="user-menu-role"]')).toContainText(/owner/i);

  // 2. Visit /users — see only the owner
  await page.goto('/users');
  await expect(page.locator('[data-testid^="user-row-"]')).toHaveCount(1, { timeout: 10_000 });

  // 3. Invite member
  await feature('Invite a member; accept the invitation');
  await page.locator('[data-testid="users-invite-btn"]').click();
  await page.locator('[data-testid="invite-email"]').fill(MEMBER_EMAIL);
  await page.locator('[data-testid="invite-role"]').selectOption('member');
  await page.locator('[data-testid="invite-send"]').click();
  const tokenLocator = page.locator('[data-testid="invite-dev-token"]');
  await expect(tokenLocator).toBeVisible({ timeout: 10_000 });
  const inviteToken = (await tokenLocator.textContent())!.trim();
  expect(inviteToken.length).toBeGreaterThan(20);

  // Close the invite modal (it's a fixed overlay; must close before interacting with UserMenu)
  await page.locator('[data-testid="invite-dev-token-close"]').click();
  await expect(tokenLocator).not.toBeVisible({ timeout: 5_000 });

  // 4. Sign out owner
  await page.locator('[data-testid="user-menu-toggle"]').click();
  await page.locator('[data-testid="user-menu-signout"]').click();
  await page.waitForURL('**/login');

  // 5. Accept invitation
  await page.goto(`/invite/${inviteToken}`);
  await expect(page.locator('[data-testid="accept-email"]')).toContainText(MEMBER_EMAIL, { timeout: 10_000 });
  await expect(page.locator('[data-testid="accept-role"]')).toContainText(/member/i);
  await page.locator('[data-testid="accept-password"]').fill(MEMBER_PW);
  await page.locator('[data-testid="accept-confirm-password"]').fill(MEMBER_PW);
  await page.locator('[data-testid="accept-submit"]').click();
  await page.waitForURL('**/');
  // Wait for session to be established (UserMenu shows the member's account)
  await expect(page.locator('[data-testid="user-menu-email"]')).toContainText(MEMBER_EMAIL, { timeout: 10_000 });

  // 6. Member visits /users → forbidden
  await feature('Role-based access — a member is forbidden /users');
  await page.goto('/users');
  await expect(page.locator('[data-testid="forbidden-403"]')).toBeVisible({ timeout: 10_000 });

  // 7. Sign out member, sign in as owner
  await page.locator('[data-testid="user-menu-toggle"]').click();
  await page.locator('[data-testid="user-menu-signout"]').click();
  await page.waitForURL('**/login');
  await page.locator('[data-testid="login-email"]').fill(OWNER_EMAIL);
  await page.locator('[data-testid="login-password"]').fill(OWNER_PW);
  await page.locator('[data-testid="login-submit"]').click();
  await page.waitForURL('**/');

  // 8. Promote member to admin
  await feature('Promote to admin; transfer ownership');
  await page.goto('/users');
  await page.locator(`[data-testid="user-edit-${MEMBER_EMAIL}"]`).click();
  await page.locator('[data-testid="user-edit-role"]').selectOption('admin');
  await page.locator('[data-testid="user-edit-save"]').click();
  await expect(page.locator(`[data-testid="user-role-${MEMBER_EMAIL}"]`)).toContainText(/admin/i, { timeout: 10_000 });

  // 9. Transfer ownership to the now-admin member
  await page.locator(`[data-testid="user-transfer-${MEMBER_EMAIL}"]`).click();
  await page.locator('[data-testid="confirm-transfer"]').click();
  await expect(page.locator(`[data-testid="user-role-${MEMBER_EMAIL}"]`)).toContainText(/owner/i, { timeout: 10_000 });
  await expect(page.locator(`[data-testid="user-role-${OWNER_EMAIL}"]`)).toContainText(/admin/i, { timeout: 10_000 });
  // Read back from the server, not the page's own state.
  await page.reload();
  await expect(page.locator(`[data-testid="user-role-${MEMBER_EMAIL}"]`)).toContainText(/owner/i, { timeout: 10_000 });
  await expect(page.locator(`[data-testid="user-role-${OWNER_EMAIL}"]`)).toContainText(/admin/i, { timeout: 10_000 });

  // 9b. Providers: the org's model pool narrows what keys may use, never what
  // the page lists, or a model left out of the pool could not be added back.
  // In an org of its own: the specs running beside this one share the
  // default org, and a pool there would refuse their models.
  await feature('Providers — the org pool, edited on the page; a provider with no models says so');
  const csrf = ((await (await page.request.get(`${API_BASE}/v1/auth/csrf`)).json()) as { csrf_token: string }).csrf_token;
  const created = await page.request.post(`${API_BASE}/v1/orgs`, {
    headers: { 'x-csrf-token': csrf },
    data: { name: `Providers e2e ${Date.now()}` },
  });
  expect([200, 201], await created.text()).toContain(created.status());
  const poolOrg = ((await created.json()) as { id: string }).id;
  await page.goto('/providers');
  const switcher = page.getByTestId('org-switcher');
  const homeOrg = await switcher.inputValue();
  await switcher.selectOption(poolOrg);
  await page.getByTestId('provider-toggle-mock').click();
  const pooled = page.getByTestId('provider-model-enable-mock/image-gen');
  await expect(page.getByTestId('provider-model-enable-mock/video-gen')).toBeVisible({ timeout: 10_000 });
  const poolSaved = nextCall(page, 'PUT', /^\/v1\/orgs\/[^/]+\/allowed-models$/);
  await pooled.check();
  expect((await poolSaved).status(), 'the pool is now just mock/image-gen').toBe(200);
  await page.reload();
  await page.getByTestId('provider-toggle-mock').click();
  await expect(page.getByTestId('provider-model-enable-mock/image-gen')).toBeChecked({ timeout: 10_000 });
  await expect(page.getByTestId('provider-model-enable-mock/video-gen'), 'a model outside the pool is still listed').not.toBeChecked();
  // Another provider's models are outside this org's pool: listed, none enabled.
  await page.getByTestId('provider-toggle-bedrock').click();
  await expect(page.getByTestId('provider-toggle-bedrock')).toContainText(/(?<!\d)0\/[1-9]\d* enabled/);

  // 9c. What a session is offered is the permission list /v1/auth/me serves for
  // the org it acts in (FU-ME), not its account role: an Admin of the
  // deployment who owns an org of its own holds none of the deployment's own
  // permissions while acting there.
  await feature("Acting in an org of its own: the deployment's users and settings are not offered");
  const served = (await (await page.request.get(`${API_BASE}/v1/auth/me`)).json()) as {
    user: { role: string };
    orgs: Array<{ id: string; role: string; permissions: string[] }>;
  };
  const sideOrg = served.orgs.find(o => o.id === poolOrg)!;
  const deploymentOrg = served.orgs.find(o => o.id === homeOrg)!;
  expect(served.user.role, 'the account role still says admin').toBe('admin');
  expect(sideOrg.role).toBe('owner');
  expect(sideOrg.permissions).not.toContain('system:config');
  expect(sideOrg.permissions).not.toContain('user:read:any');
  expect(deploymentOrg.permissions).toEqual(expect.arrayContaining(['system:config', 'user:read:any']));
  await expect(page.getByRole('link', { name: 'Users', exact: true })).toHaveCount(0);
  await page.getByRole('link', { name: 'Gateway', exact: true }).click();
  await expect(page.getByTestId('forbidden-403')).toContainText('system:config');
  // Back in the deployment's own org, both are offered again.
  await page.getByTestId('org-switcher').selectOption(homeOrg);
  await expect(page.getByRole('link', { name: 'Users', exact: true })).toBeVisible();
  await expect(page.getByTestId('forbidden-403')).toHaveCount(0);
  await expect(page.getByTestId('gateway-tab-settings')).toBeVisible({ timeout: 10_000 });

  // 10. Sign out (owner is now admin)
  await page.locator('[data-testid="user-menu-toggle"]').click();
  await page.locator('[data-testid="user-menu-signout"]').click();
  await page.waitForURL('**/login');

  // ─── Existing master-key path begins ─────────────────────────────────────────
  // After signout, unauthenticated visitors are redirected to /login, so the
  // master-key AuthBar no longer auto-shows on a bare '/'. Seed the key into
  // localStorage (exactly what the AuthBar's "Save key" persists) so the app runs
  // in API-key mode; RequireAuth then treats us as authenticated and the AuthBar
  // renders its authenticated state. This matches how the other e2e specs auth.

  // ─── Step 1: Auth setup ──────────────────────────────────────────────────────
  await feature('Master-key authentication');
  // Seed once (not via addInitScript) so the later AuthBar sign-out can actually
  // clear it — we're on /login here, so localStorage is writable for this origin.
  await page.evaluate((key) => localStorage.setItem('litegen_api_key', key), MASTER_KEY);
  await page.goto('/');

  // Assert authenticated state appears
  await expect(page.getByTestId('auth-status')).toBeVisible({ timeout: 5_000 });
  await expect(page.getByTestId('auth-status')).toContainText('Authenticated ✓');

  // ─── Step 2: Overview page ───────────────────────────────────────────────────
  await feature('Overview — the figures are the server\'s /v1/stats');
  // Navigate away and back to reload data with auth now set
  await page.getByRole('link', { name: 'Logs' }).click();
  const statsCall = nextCall(page, 'GET', /^\/v1\/stats$/);
  await page.getByRole('link', { name: 'Overview' }).click();
  const shownStats = (await (await statsCall).json()) as { total_requests: number; failed_requests: number };
  await expect(page.getByText('Total Requests')).toBeVisible({ timeout: 10_000 });
  await expect(page.locator('.stat-card').filter({ hasText: 'Total Requests' }).locator('.value'))
    .toHaveText(shownStats.total_requests.toLocaleString('en-US'));
  await expect(page.locator('.stat-card').filter({ hasText: 'Failed Requests' }).locator('.value'))
    .toHaveText(shownStats.failed_requests.toLocaleString('en-US'));
  await expect(page.getByText('Success Rate')).toBeVisible();
  await expect(page.getByText('Failed Requests')).toBeVisible();
  await expect(page.getByText('Total Cost')).toBeVisible();
  await expect(page.getByText('Avg Latency')).toBeVisible();
  await expect(page.getByText('RPM')).toBeVisible();

  // P50/P95/P99 latency cards (cluster 4)
  await expect(page.getByTestId('overview-p50')).toBeVisible();
  await expect(page.getByTestId('overview-p95')).toBeVisible();
  await expect(page.getByTestId('overview-p99')).toBeVisible();

  // ─── Step 3: Logs page ───────────────────────────────────────────────────────
  await feature('Logs — every log the server returned is a row');
  const logsCall = nextCall(page, 'GET', /^\/v1\/logs$/);
  await page.getByRole('link', { name: 'Logs' }).click();
  const logsPage = (await (await logsCall).json()) as { data: Array<{ id: string }> };
  await expect(page.locator('table')).toBeVisible({ timeout: 10_000 });
  for (const entry of logsPage.data.slice(0, 10)) {
    await expect(page.getByTestId(`logs-row-${entry.id}`)).toBeVisible();
  }

  // ─── Step 4: Models page ─────────────────────────────────────────────────────
  await feature('Models page and its detail drawer');
  await page.getByRole('link', { name: 'Models' }).click();

  // Wait for at least 10 model rows to appear (may be more)
  await expect(async () => {
    const count = await page.locator('tbody tr').count();
    expect(count).toBeGreaterThanOrEqual(10);
  }).toPass({ timeout: 15_000 });

  const modelRows = page.locator('tbody tr');
  const count = await modelRows.count();
  expect(count).toBeGreaterThanOrEqual(10);

  // Click on the mock/image-gen row
  const mockImageRow = page.getByTestId('model-row-mock/image-gen');
  await expect(mockImageRow).toBeVisible({ timeout: 5_000 });
  await mockImageRow.click();

  // Assert detail panel shows
  const detailPanel = page.getByTestId('model-detail-panel');
  await expect(detailPanel).toBeVisible({ timeout: 5_000 });

  // Assert the panel includes pricing content, for the model the server describes.
  await expect(page.getByTestId('model-detail-panel')).toContainText('Pricing', { timeout: 10_000 });
  const imageGen = await (await page.request.get(`${API_BASE}/v1/models/mock/image-gen`, { headers: { Authorization: `Bearer ${MASTER_KEY}` } })).json();
  await expect(page.getByTestId('model-detail-panel')).toContainText(imageGen.display_name);

  // Close the panel
  await page.getByTestId('model-detail-close').click();
  await expect(detailPanel).not.toBeVisible({ timeout: 3_000 });

  // ─── Step 5: Health page ─────────────────────────────────────────────────────
  await feature('Health — every provider /health reports, and a cache clear');
  const healthCall = nextCall(page, 'GET', /^\/health$/);
  await page.getByRole('link', { name: 'Health' }).click();
  const health = (await (await healthCall).json()) as { providers: Array<{ provider: string }> };
  await expect(page.locator('table')).toBeVisible({ timeout: 10_000 });
  expect(health.providers.map(p => p.provider)).toContain('mock');
  for (const p of health.providers) await expect(page.locator('table')).toContainText(p.provider);

  // Click Refresh
  const refreshBtn = page.getByRole('button', { name: 'Refresh' });
  await expect(refreshBtn).toBeVisible();
  await refreshBtn.click();
  await expect(page.locator('table')).toBeVisible({ timeout: 5_000 });

  // Click Clear Cache — should not show error alert
  const clearCacheBtn = page.getByRole('button', { name: 'Clear Cache' });
  await expect(clearCacheBtn).toBeVisible();
  const cleared = nextCall(page, 'DELETE', /^\/v1\/cache$/);
  await clearCacheBtn.click();
  expect((await cleared).status(), 'the server cleared its cache').toBe(200);
  // Ensure no error appeared
  await expect(page.locator('.alert-error')).not.toBeVisible({ timeout: 3_000 });

  // ─── Step 6: Keys page — full CRUD ──────────────────────────────────────────
  await feature('API keys — create, use, edit, persist, revoke');
  await page.getByRole('link', { name: 'API Keys' }).click();
  await expect(page.locator('table')).toBeVisible({ timeout: 10_000 });

  // May or may not be empty initially; that's fine

  // Create key
  await page.getByTestId('new-key-name').fill('playwright-test-key');
  await page.getByTestId('new-key-token-quota').fill('10');
  await page.getByTestId('new-key-rpm-limit').fill('60');

  // Clear scopes field and fill with generate,read,admin
  const scopesInput = page.getByTestId('new-key-scopes');
  await scopesInput.clear();
  await scopesInput.fill('generate,read,admin');

  await page.getByTestId('create-key-btn').click();

  // Assert success banner with created key (id/secret pair UI from Task 12)
  const banner = page.getByTestId('key-created-banner');
  await expect(banner).toBeVisible({ timeout: 10_000 });
  // Public id (pk_live_…) is shown alongside the one-time secret.
  await expect(page.getByTestId('key-public-id')).toContainText('pk_live_');

  // Get the created secret value from the banner's secret <code> (sk_live_…)
  const keyText = await page.getByTestId('key-secret').textContent();
  expect(keyText).toBeTruthy();
  expect(keyText!.startsWith('sk_live_')).toBeTruthy();

  // The secret works: the gateway serves a generation for it.
  const withKey = (secret: string) => page.request.post(`${API_BASE}/v1/images/generations`, {
    headers: { Authorization: `Bearer ${secret}`, 'Content-Type': 'application/json' },
    data: { model: 'mock/image-gen', prompt: 'a key at work' },
  });
  expect((await withKey(keyText!)).status(), 'a new key generates').toBe(200);

  // Wait for table row to appear — find the row with name "playwright-test-key"
  await expect(page.getByText('playwright-test-key')).toBeVisible({ timeout: 10_000 });

  // Verify columns in the new row
  // Find the row containing our key name
  const allRows = page.locator('tbody tr');
  let keyRowId: string | null = null;

  // Find the row with playwright-test-key
  for (let i = 0; i < await allRows.count(); i++) {
    const row = allRows.nth(i);
    const rowText = await row.textContent();
    if (rowText?.includes('playwright-test-key')) {
      // Get the row's data-testid to extract the ID
      const testId = await row.getAttribute('data-testid');
      if (testId?.startsWith('key-row-')) {
        keyRowId = testId.replace('key-row-', '');
      }
      break;
    }
  }

  expect(keyRowId).toBeTruthy();

  // Check the scopes cell
  const scopesCell = page.getByTestId(`key-scopes-${keyRowId}`);
  await expect(scopesCell).toContainText('admin');

  // Check quota cell
  const quotaCell = page.getByTestId(`key-quota-${keyRowId}`);
  await expect(quotaCell).toContainText('10');

  // Check rpm cell
  const rpmCell = page.getByTestId(`key-rpm-${keyRowId}`);
  await expect(rpmCell).toContainText('60');

  // ─── Persistence check 1: reload ────────────────────────────────────────────
  await page.reload();
  // Re-auth (localStorage persists through reload)
  await expect(page.getByTestId('auth-status')).toBeVisible({ timeout: 10_000 });

  // Navigate back to keys page
  await page.getByRole('link', { name: 'API Keys' }).click();
  await expect(page.locator('table')).toBeVisible({ timeout: 10_000 });
  await expect(page.getByText('playwright-test-key')).toBeVisible({ timeout: 10_000 });

  // ─── PATCH: Edit quota and RPM ───────────────────────────────────────────────
  const editBtn = page.getByTestId(`edit-key-${keyRowId}`);
  await expect(editBtn).toBeVisible({ timeout: 5_000 });
  await editBtn.click();

  // Edit form should appear
  const editRow = page.getByTestId(`edit-row-${keyRowId}`);
  await expect(editRow).toBeVisible({ timeout: 5_000 });

  // Clear and fill token_quota = 50
  const editQuota = page.getByTestId('edit-token-quota');
  await editQuota.clear();
  await editQuota.fill('50');

  // Clear and fill rpm = 120
  const editRpm = page.getByTestId('edit-rpm-limit');
  await editRpm.clear();
  await editRpm.fill('120');

  // Save
  await page.getByTestId('save-edit-btn').click();

  // Edit row should close
  await expect(editRow).not.toBeVisible({ timeout: 5_000 });

  // Table should refresh: check new values
  await expect(page.getByTestId(`key-quota-${keyRowId}`)).toContainText('50', { timeout: 10_000 });
  await expect(page.getByTestId(`key-rpm-${keyRowId}`)).toContainText('120', { timeout: 10_000 });

  // ─── Persistence check 2: reload ────────────────────────────────────────────
  await page.reload();
  await expect(page.getByTestId('auth-status')).toBeVisible({ timeout: 10_000 });
  await page.getByRole('link', { name: 'API Keys' }).click();
  await expect(page.locator('table')).toBeVisible({ timeout: 10_000 });

  // Check values persisted
  await expect(page.getByTestId(`key-quota-${keyRowId}`)).toContainText('50', { timeout: 10_000 });
  await expect(page.getByTestId(`key-rpm-${keyRowId}`)).toContainText('120', { timeout: 10_000 });

  // ─── Revoke ──────────────────────────────────────────────────────────────────
  const revokeBtn = page.getByTestId(`revoke-key-${keyRowId}`);
  await expect(revokeBtn).toBeVisible({ timeout: 5_000 });
  await revokeBtn.click();

  // Status should become revoked
  await expect(page.getByTestId(`key-status-${keyRowId}`)).toContainText('revoked', { timeout: 10_000 });

  // ─── Persistence check 3: reload ────────────────────────────────────────────
  await page.reload();
  await expect(page.getByTestId('auth-status')).toBeVisible({ timeout: 10_000 });
  await page.getByRole('link', { name: 'API Keys' }).click();
  await expect(page.locator('table')).toBeVisible({ timeout: 10_000 });

  // Key still shows as revoked
  await expect(page.getByTestId(`key-status-${keyRowId}`)).toContainText('revoked', { timeout: 10_000 });
  // …and the gateway refuses it.
  expect((await withKey(keyText!)).status(), 'a revoked key is refused').toBe(401);

  // ─── NEW: Playground ──────────────────────────────────────────────────────────
  await feature('Playground — generate, the Request/Response tabs, history and re-submit');
  await page.click('a[href="/playground"]');
  await page.waitForURL('**/playground');

  // Wait for model options to load (models are fetched async on mount)
  await expect(async () => {
    const optCount = await page.locator('[data-testid="playground-model"] option').count();
    expect(optCount).toBeGreaterThan(0);
  }).toPass({ timeout: 10_000 });

  // Select mock/visual-image-gen so the response is a REAL visible PNG
  await page.locator('[data-testid="playground-model"]').selectOption({ value: 'mock/visual-image-gen' });

  await page.locator('[data-testid="playground-prompt"]').fill('a playwright cat');

  // Lax mode: the run still goes, whatever the model cannot take
  const strictCheckbox = page.locator('[data-testid="playground-strict"]');
  await strictCheckbox.uncheck();

  // mock/visual-image-gen takes no negative prompt, so the field is off.
  await expect(page.locator('[data-testid="playground-negative-prompt"]')).toBeDisabled();

  const firstRun = nextCall(page, 'POST', /^\/v1\/images\/generations$/);
  await page.locator('[data-testid="playground-generate"]').click();
  const firstResponse = await firstRun;
  expect(firstResponse.status()).toBe(200);
  const firstWire = firstResponse.request().postDataJSON() as Record<string, unknown>;
  const firstBody = (await firstResponse.json()) as { id: string; model: string; data: Array<{ b64_json: string }> };
  expect(firstWire).toMatchObject({ model: 'mock/visual-image-gen', prompt: 'a playwright cat' });
  expect(firstBody.model).toBe('mock/visual-image-gen');
  const playgroundGenId = firstBody.id;

  // The image shown is the one the server returned, and it decodes.
  const imgLocator = page.locator('[data-testid="playground-image"]');
  await expect(imgLocator).toBeVisible({ timeout: 15_000 });
  await expect(imgLocator).toHaveAttribute('src', `data:image/png;base64,${firstBody.data[0].b64_json}`);
  expect(await imgLocator.evaluate((el: HTMLImageElement) => el.naturalWidth), 'the PNG decodes').toBeGreaterThan(0);

  // Switch to Request JSON tab: exactly the body that went over the wire.
  await page.locator('[data-testid="playground-tab-request"]').click();
  const reqText = await page.locator('[data-testid="playground-request-json"]').textContent();
  expect(JSON.parse(reqText!)).toEqual(firstWire);

  // Switch to Response JSON tab: the server's answer.
  await page.locator('[data-testid="playground-tab-response"]').click();
  const resText = await page.locator('[data-testid="playground-response-json"]').textContent();
  expect(JSON.parse(resText!).id).toBe(playgroundGenId);

  // History row appears
  const histRow0 = page.locator('[data-testid="playground-history-row-0"]');
  await expect(histRow0).toBeVisible();
  await histRow0.click();
  // Form should be repopulated — assert prompt input value
  await expect(page.locator('[data-testid="playground-prompt"]')).toHaveValue('a playwright cat');

  // Strict toggle — check it and verify it's checked (it was unchecked for the generate above)
  const strictToggle = page.locator('[data-testid="playground-strict"]');
  await strictToggle.check();
  await expect(strictToggle).toBeChecked();
  // Also verify uncheck works
  await strictToggle.uncheck();
  await expect(strictToggle).not.toBeChecked();
  await strictToggle.check(); // restore

  // Re-submit history row 0: the same body goes again, and a new generation comes back.
  const rerun = nextCall(page, 'POST', /^\/v1\/images\/generations$/);
  await page.locator('[data-testid="playground-history-resubmit-0"]').click();
  const rerunResponse = await rerun;
  expect(rerunResponse.status()).toBe(200);
  expect(rerunResponse.request().postDataJSON()).toEqual(firstWire);
  expect(((await rerunResponse.json()) as { id: string }).id).not.toBe(playgroundGenId);
  await expect(page.locator('[data-testid="playground-history-row-1"]')).toBeVisible({ timeout: 10_000 });

  // Delete history row 1
  await page.locator('[data-testid="playground-history-delete-1"]').click();
  // Now row 1 should no longer exist; only row 0
  await expect(page.locator('[data-testid="playground-history-row-1"]')).toHaveCount(0);
  await expect(histRow0).toBeVisible();

  // ─── NEW: Generations ─────────────────────────────────────────────────────────
  await feature('Generations — cancel a running job; the vendor takes it and the charge is refunded');
  // First trigger a video generation via API directly. mock/held-video stays
  // processing until it is cancelled, so the cancel is never a race.
  const masterKey = process.env.PLAYWRIGHT_MASTER_KEY ?? 'test-master-key-please-rotate';
  const videoResp = await page.request.post(`${API_BASE}/v1/videos/generations`, {
    headers: {
      Authorization: `Bearer ${masterKey}`,
      'Content-Type': 'application/json',
    },
    data: { model: 'mock/held-video', prompt: 'a playwright cat dancing' },
  });
  expect(videoResp.status()).toBe(200);
  const videoBody = await videoResp.json();
  const videoId = videoBody.id as string;
  expect(videoId).toBeTruthy();
  expect(videoBody.usage.cost_usd, 'the held clip is billed at submit').toBe(0.25);

  // Navigate to Generations
  await page.click('a[href="/generations"]');
  await page.waitForURL('**/generations');

  // Wait for the row to appear (auto-refresh runs every 3s on pending rows)
  const rowLocator = page.locator(`[data-testid="gen-row-${videoId}"]`);
  await expect(rowLocator).toBeVisible({ timeout: 15_000 });

  // Click the id cell to expand
  await page.locator(`[data-testid="gen-expand-${videoId}"]`).click();
  await expect(page.locator(`[data-testid="gen-detail-${videoId}"]`)).toBeVisible();

  // The job is in flight, so the cancel is always available.
  await expect(page.locator(`[data-testid="gen-status-${videoId}"]`)).toContainText(/pending|processing/);
  const cancelResp = page.waitForResponse(
    r => r.request().method() === 'PATCH' && r.url().includes(`/v1/generations/${videoId}`),
  );
  await page.locator(`[data-testid="gen-cancel-${videoId}"]`).click();
  const cancelled = await cancelResp;
  expect(cancelled.status()).toBe(200);
  const cancelBody = (await cancelled.json()) as { status: string; vendor_cancelled: boolean };
  expect(cancelBody.status).toBe('cancelled');
  expect(cancelBody.vendor_cancelled, 'the vendor accepted the cancel').toBe(true);
  await expect(page.locator(`[data-testid="gen-status-${videoId}"]`)).toContainText('cancelled', { timeout: 5_000 });

  // L18: a cancelled job is refunded — its row now costs nothing.
  const afterCancel = await (await page.request.get(`${API_BASE}/v1/generations/${videoId}`, {
    headers: { Authorization: `Bearer ${masterKey}` },
  })).json();
  expect(afterCancel.status).toBe('cancelled');
  expect(afterCancel.cost_usd, 'refunded').toBe(0);

  // Reload and assert the row still exists, still cancelled (persistence)
  await page.reload();
  await expect(rowLocator).toBeVisible();
  await expect(page.locator(`[data-testid="gen-status-${videoId}"]`)).toContainText('cancelled');

  // ─── NEW: Overview auto-refresh / cost chart / quota table ──────────────────
  await feature('Overview auto-refresh re-asks /v1/stats');
  await page.click('a[href="/"]');
  // Home route — match by pathname so this isn't tied to the dev-server host/port.
  await page.waitForURL((url) => url.pathname === '/');

  // Toggle auto-refresh on (testid is on the <label>, so locate the inner checkbox)
  const refreshToggle = page.locator('[data-testid="overview-autorefresh"] input[type="checkbox"]');
  // Auto-refresh polls stats on a 5s setInterval — wait for that fetch to actually
  // fire rather than sleeping a fixed interval. Arm the listener before enabling.
  const autoRefreshFired = page.waitForResponse(
    (r) => r.url().includes('/v1/stats') && r.request().method() === 'GET',
    { timeout: 15_000 },
  );
  await refreshToggle.check();

  // Cost chart and quota table containers are present
  await expect(page.locator('[data-testid="overview-cost-chart"]')).toBeVisible();
  await expect(page.locator('[data-testid="overview-quota-table"]')).toBeVisible();

  // Quota rows may or may not be populated depending on active keys;
  // the playwright-test-key was revoked above so it won't appear.
  // Just verify the table section is rendered (the container is always present).
  // If any active quota keys exist, they should show up.
  // Quota rows may or may not exist — container visibility is the real check.
  // (The key created in step 6 was revoked, so count=0 is expected here.)

  // Wait for auto-refresh to fire at least once (resolves as soon as it does).
  await autoRefreshFired;

  // Toggle off
  await refreshToggle.uncheck();

  // ─── NEW: Logs filters + trace panel drill-down ───────────────────────────────
  await feature('Logs — a model filter, its CSV, and the trace of our own request');
  await page.click('a[href="/logs"]');
  await page.waitForURL('**/logs');

  // Apply a model filter — use mock/visual-image-gen (the model we just generated with)
  await page.locator('[data-testid="logs-filter-model"]').selectOption({ label: 'mock/visual-image-gen' });
  const filtered = nextCall(page, 'GET', /^\/v1\/logs$/);
  await page.locator('[data-testid="logs-filter-apply"]').click();
  await expect(page).toHaveURL(/model=mock%2Fvisual-image-gen/);
  const filteredLogs = (await (await filtered).json()) as { data: Array<{ id: string; model: string }> };
  expect(new Set(filteredLogs.data.map(l => l.model)), 'the server filtered by model').toEqual(new Set(['mock/visual-image-gen']));

  // Export CSV — intercept download, and read it: our request, and that model only.
  const downloadPromise = page.waitForEvent('download', { timeout: 10_000 });
  await page.locator('[data-testid="logs-export-csv"]').click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toMatch(/logs-\d{8}\.csv/);
  const csv = fs.readFileSync((await download.path())!, 'utf8').trim().split('\n');
  expect(csv[0]).toMatch(/model/);
  expect(csv.some(line => line.includes(playgroundGenId)), 'our request is in the export').toBe(true);
  for (const line of csv.slice(1)) expect(line).toContain('mock/visual-image-gen');

  // Open the trace of our own request (not whichever row happens to be first).
  const firstRow = page.getByTestId(`logs-row-${playgroundGenId}`);
  await expect(firstRow).toBeVisible({ timeout: 10_000 });
  await firstRow.click();

  // Trace panel should be visible
  await expect(page.locator('[data-testid="trace-panel"]')).toBeVisible({ timeout: 5_000 });

  // Cycle through every tab
  await page.locator('[data-testid="trace-tab-visual"]').click();
  // mock/visual-image-gen produces a real colored PNG → image should be visible
  const traceImg = page.locator('[data-testid="trace-visual-image"]');
  await expect(traceImg).toBeVisible({ timeout: 10_000 });
  const traceSrc = await traceImg.getAttribute('src');
  expect(traceSrc, 'trace image src must be populated').toBeTruthy();
  expect(traceSrc!.length, 'trace image must be a real PNG (>1000 chars), not a placeholder').toBeGreaterThan(1000);

  await page.locator('[data-testid="trace-tab-prompt"]').click();
  await expect(page.locator('[data-testid="trace-prompt-text"]')).toHaveText('a playwright cat');

  await page.locator('[data-testid="trace-tab-params"]').click();
  await expect(page.locator('[data-testid="trace-params-table"]')).toBeVisible();

  await page.locator('[data-testid="trace-tab-response"]').click();
  await expect(page.locator('[data-testid="trace-response-json"]')).toContainText(playgroundGenId);

  // Close panel via close button
  await page.locator('[data-testid="trace-panel-close"]').click();
  await expect(page.locator('[data-testid="trace-panel"]')).not.toBeVisible({ timeout: 3_000 });

  // Clear filters
  await page.locator('[data-testid="logs-filter-clear"]').click();
  await expect(page).not.toHaveURL(/model=/);

  // ─── NEW: Keys rotate + test-webhook ─────────────────────────────────────────
  await feature('Webhook test delivery; key rotation in place');
  const webhookPort = 5180;
  let rotatedWebhookSecret = '';
  const { server: webhookServer, received: webhookReceived } = await startWebhookReceiver(webhookPort);

  try {
    await page.click('a[href="/keys"]');
    await page.waitForURL('**/keys');

    // Create a fresh key with a webhook_url pointing to the receiver
    await page.locator('[data-testid="new-key-name"]').fill('webhook-test-key');
    await page.locator('[data-testid="new-key-scopes"]').fill('generate,read,admin');
    await page.locator('[data-testid="create-key-btn"]').click();
    await expect(page.locator('[data-testid="key-created-banner"]')).toBeVisible({ timeout: 10_000 });
    const webhookKeySecret = (await page.getByTestId('key-secret').textContent())!.trim();

    // Find the new row by name cell
    const webhookKeyRows = page.locator('tbody tr');
    let webhookKeyId: string | null = null;
    for (let i = 0; i < await webhookKeyRows.count(); i++) {
      const row = webhookKeyRows.nth(i);
      const rowText = await row.textContent();
      if (rowText?.includes('webhook-test-key')) {
        const testId = await row.getAttribute('data-testid');
        if (testId?.startsWith('key-row-')) {
          webhookKeyId = testId.replace('key-row-', '');
        }
        break;
      }
    }
    expect(webhookKeyId).toBeTruthy();

    // Edit to set webhook_url
    await page.locator(`[data-testid="edit-key-${webhookKeyId}"]`).click();
    // The edit row's webhook URL input has testid="edit-webhook-url"
    await page.locator('[data-testid="edit-webhook-url"]').fill(`http://127.0.0.1:${webhookPort}/hook`);
    await page.locator('[data-testid="save-edit-btn"]').click();

    // Wait for edit row to close
    await expect(page.locator(`[data-testid="edit-row-${webhookKeyId}"]`)).not.toBeVisible({ timeout: 5_000 });

    // Test webhook button should now be visible
    await page.locator(`[data-testid="key-test-webhook-${webhookKeyId}"]`).click();
    // Inline result shows "200" or "OK"
    await expect(page.locator(`[data-testid="key-test-webhook-result-${webhookKeyId}"]`)).toContainText(/200|OK/, { timeout: 10_000 });

    // Receiver should have got one POST
    expect(webhookReceived.length).toBeGreaterThan(0);
    expect(webhookReceived[0].body.status).toBe('completed');

    // Capture the public id BEFORE rotation so we can prove it changed in place.
    const publicIdBefore = (await page
      .locator(`[data-testid="key-public-id-${webhookKeyId}"]`)
      .textContent())!.trim();
    expect(publicIdBefore.startsWith('pk_live_')).toBeTruthy();

    // Rotate the key. Task 12's id/secret rewrite makes rotation IN PLACE: the
    // same row id keeps its name/scopes/settings and stays ACTIVE, but gets a
    // brand-new pk_live_ public id and a one-time sk_live_ secret. (Previously
    // rotation revoked the old key and minted a separate new row.)
    await page.locator(`[data-testid="key-rotate-${webhookKeyId}"]`).click();
    const rotateBanner = page.locator('[data-testid="key-rotate-result-banner"]');
    await expect(rotateBanner).toBeVisible({ timeout: 10_000 });
    // The rotate banner reveals the new secret exactly once.
    const rotatedSecret = (await rotateBanner.locator('code').textContent())!.trim();
    expect(rotatedSecret.startsWith('sk_live_')).toBeTruthy();

    // Reload to see the updated row
    await page.reload();
    await expect(page.getByTestId('auth-status')).toBeVisible({ timeout: 10_000 });
    await page.click('a[href="/keys"]');
    await page.waitForURL('**/keys');

    const oldRow = page.locator(`[data-testid="key-row-${webhookKeyId}"]`);
    // Same row id persists and remains active after an in-place rotation.
    await expect(oldRow.locator(`[data-testid="key-status-${webhookKeyId}"]`)).toContainText('active', { timeout: 10_000 });
    // …but its public id was replaced with a fresh pk_live_ value.
    const publicIdAfter = (await page
      .locator(`[data-testid="key-public-id-${webhookKeyId}"]`)
      .textContent())!.trim();
    expect(publicIdAfter.startsWith('pk_live_')).toBeTruthy();
    expect(publicIdAfter).not.toBe(publicIdBefore);
    // The old secret no longer works; the new one does.
    expect((await withKey(webhookKeySecret)).status(), 'the rotated-away secret is refused').toBe(401);
    expect((await withKey(rotatedSecret)).status(), 'the new secret generates').toBe(200);
    rotatedWebhookSecret = rotatedSecret;

    // Copy prefix check — grant clipboard permissions for the context first
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    // Find any active row
    const anyActiveRow = page.locator('[data-testid^="key-row-"]').filter({ has: page.locator('.badge.healthy') }).first();
    const activeTestId = await anyActiveRow.getAttribute('data-testid');
    const activeId = activeTestId!.replace('key-row-', '');
    await page.locator(`[data-testid="key-copy-prefix-${activeId}"]`).click();
    // Toast should appear, and the clipboard holds that key's prefix.
    await expect(page.locator('[data-testid^="toast-"]')).toBeVisible({ timeout: 3000 });
    const keys = (await (await page.request.get(`${API_BASE}/v1/keys`, { headers: { Authorization: `Bearer ${MASTER_KEY}` } })).json()) as {
      data: Array<{ id: string; prefix: string }>;
    };
    const copied = keys.data.find(k => k.id === activeId)!;
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(copied.prefix);
  } finally {
    await new Promise<void>((resolve) => webhookServer.close(() => resolve()));
  }

  // ─── NEW: Models filters + copy curl ─────────────────────────────────────────
  await feature('Models — provider, media-type and capability filters; copy as curl');
  await page.click('a[href="/models"]');
  await page.waitForURL('**/models');

  // Filter to mock provider
  await page.locator('[data-testid="models-filter-provider"]').selectOption({ label: 'mock' });

  // Assert at least 14 mock model rows (4 original + 12 new config-demo models).
  // The provider filter is client-side, so poll the row count (auto-retries until
  // the re-render settles) instead of sleeping a fixed interval.
  await expect.poll(
    () => page.locator('[data-testid^="model-row-"]').count(),
    { timeout: 5_000 },
  ).toBeGreaterThanOrEqual(14);

  // The catalog the filters must agree with.
  const catalog = ((await (await page.request.get(`${API_BASE}/v1/models`, { headers: { Authorization: `Bearer ${MASTER_KEY}` } })).json()) as {
    data: Array<{ id: string; provider: string; media_type: string; capabilities: Record<string, unknown> }>;
  }).data.filter(m => m.provider === 'mock' && !m.id.startsWith('mock/e2e-runtime-'));
  // The management spec adds and removes a runtime mock model as it runs, in
  // parallel: neither side counts it.
  const shownIds = async () => (await page.locator('[data-testid^="model-row-"]').evaluateAll(rows =>
    rows.map(r => r.getAttribute('data-testid')!.replace('model-row-', ''))))
    .filter(id => !id.startsWith('mock/e2e-runtime-'))
    .sort();

  // Media type: each radio shows exactly the mock models of that type.
  for (const media of ['image', 'video']) {
    await page.locator(`[data-testid="models-filter-media-type"] input[value="${media}"]`).click();
    await expect.poll(shownIds).toEqual(catalog.filter(m => m.media_type === media).map(m => m.id).sort());
  }
  await page.locator('[data-testid="models-filter-media-type"] input[value=""]').click(); // reset to All

  // Capability: rigging shows exactly the models that rig.
  await page.getByTestId('models-filter-capability-supports_rig').check();
  await expect.poll(shownIds).toEqual(catalog.filter(m => m.capabilities.supports_rig === true).map(m => m.id).sort());
  await page.getByTestId('models-filter-capability-supports_rig').uncheck();
  await expect.poll(shownIds).toEqual(catalog.map(m => m.id).sort());

  // Click on the mock/image-gen row to open detail
  await page.locator('[data-testid="model-row-mock/image-gen"]').click();

  // Click Copy as curl (clipboard permissions already granted)
  await page.locator('[data-testid="model-detail-copy-curl"]').click();

  // Read the clipboard
  const clipboard = await page.evaluate(() => navigator.clipboard.readText());
  expect(clipboard).toContain('mock/image-gen');
  expect(clipboard).toContain('curl -X POST');

  // Close the model detail panel
  await page.getByTestId('model-detail-close').click();
  await expect(page.getByTestId('model-detail-panel')).not.toBeVisible({ timeout: 3_000 });

  // ─── Cluster 3a: Health probes ───────────────────────────────────────────────
  await feature('Liveness and readiness probes');
  await page.getByRole('link', { name: 'Health' }).click();
  await expect(page.locator('table')).toBeVisible({ timeout: 10_000 });

  // Liveness badge — wait for probe to resolve (starts as "…")
  await expect(page.getByTestId('health-liveness-badge')).toBeVisible({ timeout: 10_000 });
  await expect(page.getByTestId('health-liveness-badge')).toContainText('alive', { timeout: 10_000 });

  // Readiness badge — mock provider + sqlite DB = ready
  await expect(page.getByTestId('health-readiness-badge')).toBeVisible({ timeout: 10_000 });
  await expect(page.getByTestId('health-readiness-badge')).toContainText('ready', { timeout: 10_000 });

  // ─── Cluster 3b: X-RateLimit headers ─────────────────────────────────────────
  await feature('Rate limits — X-RateLimit headers on a 60 rpm key');
  // Navigate to keys and create a fresh key with rpm_limit=60
  await page.getByRole('link', { name: 'API Keys' }).click();
  await expect(page.locator('table')).toBeVisible({ timeout: 10_000 });

  await page.getByTestId('new-key-name').fill('ratelimit-test-key');
  await page.getByTestId('new-key-rpm-limit').fill('60');
  const newRpmScopesInput = page.getByTestId('new-key-scopes');
  await newRpmScopesInput.clear();
  await newRpmScopesInput.fill('generate,read');
  await page.getByTestId('create-key-btn').click();

  const rpmBanner = page.getByTestId('key-created-banner');
  await expect(rpmBanner).toBeVisible({ timeout: 10_000 });
  // Use the secret <code> (sk_live_…) as the bearer token, not the public id.
  const rpmKeyValue = await page.getByTestId('key-secret').textContent();
  expect(rpmKeyValue).toBeTruthy();
  expect(rpmKeyValue!.startsWith('sk_live_')).toBeTruthy();

  // Fire a request with the rate-limited key
  const rpmResp = await page.request.post(`${API_BASE}/v1/images/generations`, {
    headers: {
      Authorization: `Bearer ${rpmKeyValue}`,
      'Content-Type': 'application/json',
    },
    data: { model: 'mock/image-gen', prompt: 'ratelimit test' },
  });
  // Should succeed (first out of 60 RPM)
  expect(rpmResp.status()).toBe(200);
  // X-RateLimit-Limit is the key's 60, and one of them is spent.
  expect(rpmResp.headers()['x-ratelimit-limit']).toBe('60');
  expect(rpmResp.headers()['x-ratelimit-remaining']).toBe('59');

  // ─── Cluster 3c: Audit page ───────────────────────────────────────────────────
  await feature('Audit log — an action filter, a row\'s detail, the CSV');
  await page.goto('/audit');
  await page.waitForURL('**/audit');
  await expect(page.locator('[data-testid="audit-table"]')).toBeVisible({ timeout: 10_000 });

  // At least one row should exist (keys created/modified earlier produced audit entries)
  await expect(async () => {
    const count = await page.locator('[data-testid^="audit-row-"]').count();
    expect(count).toBeGreaterThan(0);
  }).toPass({ timeout: 10_000 });

  // Apply action=key.create filter
  await page.locator('[data-testid="audit-filter-action"]').selectOption('key.create');
  await page.locator('[data-testid="audit-filter-apply"]').click();
  await expect(page).toHaveURL(/action=key\.create/, { timeout: 5_000 });
  // Every row left is a key.create.
  await expect.poll(async () => [...new Set(await page.locator('[data-testid^="audit-row-"] td:nth-child(3) code').allTextContents())])
    .toEqual(['key.create']);

  // Click a row to expand detail
  const firstAuditRow = page.locator('[data-testid^="audit-row-"]').first();
  await expect(firstAuditRow).toBeVisible({ timeout: 10_000 });
  const firstAuditId = (await firstAuditRow.getAttribute('data-testid'))!.replace('audit-row-', '');
  await firstAuditRow.click();
  await expect(page.locator(`[data-testid="audit-detail-${firstAuditId}"]`)).toBeVisible({ timeout: 3_000 });
  // Click again to collapse
  await firstAuditRow.click();
  await expect(page.locator(`[data-testid="audit-detail-${firstAuditId}"]`)).not.toBeVisible({ timeout: 3_000 });

  // Export Audit CSV
  const auditDownloadPromise = page.waitForEvent('download', { timeout: 10_000 });
  await page.locator('[data-testid="audit-export-csv"]').click();
  const auditDownload = await auditDownloadPromise;
  expect(auditDownload.suggestedFilename()).toMatch(/audit-\d{8}\.csv/);
  const auditCsv = fs.readFileSync((await auditDownload.path())!, 'utf8').trim().split('\n');
  expect(auditCsv.length, 'a header and at least the keys this test created').toBeGreaterThan(3);
  for (const line of auditCsv.slice(1)) expect(line).toContain('key.create');

  // Clear filter
  await page.locator('[data-testid="audit-filter-clear"]').click();
  await expect(page).not.toHaveURL(/action=key\.create/, { timeout: 5_000 });

  // ─── Cluster 3d: Webhook deliveries panel ────────────────────────────────────
  await feature('Webhook on job completion — received, and listed in the deliveries panel');
  // A job made with the webhook key: its completion is POSTed to the key's
  // webhook URL, and recorded as a delivery.
  const hook = await startWebhookReceiver(webhookPort);
  let hookedId = '';
  try {
    const hooked = await page.request.post(`${API_BASE}/v1/videos/generations`, {
      headers: { Authorization: `Bearer ${rotatedWebhookSecret}`, 'Content-Type': 'application/json' },
      data: { model: 'mock/video-gen', prompt: 'a clip that reports back' },
    });
    expect(hooked.status()).toBe(200);
    hookedId = ((await hooked.json()) as { id: string }).id;
    await expect.poll(async () => ((await (await page.request.get(`${API_BASE}/v1/videos/${hookedId}`, {
      headers: { Authorization: `Bearer ${rotatedWebhookSecret}` },
    })).json()) as { status: string }).status, { timeout: 30_000 }).toBe('completed');
    await expect.poll(() => hook.received.find(r => r.body?.id === hookedId)?.body?.status, { timeout: 30_000 }).toBe('completed');
  } finally {
    await new Promise<void>((resolve) => hook.server.close(() => resolve()));
  }
  await page.getByRole('link', { name: 'API Keys' }).click();
  await page.waitForURL('**/keys');
  await expect(page.locator('table')).toBeVisible({ timeout: 10_000 });
  await expect(page.getByText('webhook-test-key').first()).toBeVisible({ timeout: 10_000 });

  // Find the webhook-test-key row. After Task 12's in-place rotation it stays a
  // single ACTIVE row (same id, new secret) rather than splitting into a revoked
  // old row + a new active row, so target the active row that still owns the
  // earlier test-webhook deliveries.
  const webhookRow = page.locator('tr[data-testid^="key-row-"]')
    .filter({ hasText: 'webhook-test-key' })
    .filter({ has: page.locator('.badge.healthy') })
    .first();
  await expect(webhookRow).toBeVisible({ timeout: 5_000 });
  const webhookRowTestId = await webhookRow.getAttribute('data-testid');
  const deliveryKeyId = webhookRowTestId?.replace('key-row-', '') ?? null;
  expect(deliveryKeyId).toBeTruthy();

  // Click the Deliveries button for that key
  await page.locator(`[data-testid="key-deliveries-${deliveryKeyId}"]`).click();

  // Panel should appear and show deliveries (or "No deliveries" if none stored yet)
  await expect(page.locator(`[data-testid="key-deliveries-panel-${deliveryKeyId}"]`)).toBeVisible({ timeout: 10_000 });

  // The completion delivery above is listed, answered 200 by the receiver.
  const deliveries = page.locator(`[data-testid="key-deliveries-panel-${deliveryKeyId}"] [data-testid^="delivery-row-"]`);
  await expect(deliveries.first()).toBeVisible({ timeout: 10_000 });
  await expect(deliveries.first()).toContainText('200');

  // ─── Cluster 3e: Backpressure sanity ping ────────────────────────────────────
  await feature('The gateway still serves after everything above');
  // Lib tests cover backpressure logic; just verify the route is still healthy.
  const sanityResp = await page.request.post(`${API_BASE}/v1/images/generations`, {
    headers: {
      Authorization: `Bearer ${MASTER_KEY}`,
      'Content-Type': 'application/json',
    },
    data: { model: 'mock/image-gen', prompt: 'sanity ping' },
  });
  expect(sanityResp.status()).toBe(200);

  // ─── Step 7: Sign out / re-auth round trip ───────────────────────────────────
  await feature('Sign out, and back in');
  // AuthBar sign-out clears the key and reloads; with no key and no session the app
  // redirects to the clean /login page (the app shell / AuthBar is gone).
  await page.getByRole('button', { name: 'Sign out' }).click();
  // RequireAuth redirects to /login?next=… — match with a regex, not a glob.
  await page.waitForURL(/\/login/);
  await expect(page.getByTestId('auth-page')).toBeVisible({ timeout: 5_000 });
  await expect(page.getByTestId('auth-status')).toHaveCount(0);

  // Re-authenticate by re-seeding the key (the "Save key" equivalent) and returning
  // to the app; the authenticated AuthBar state reappears.
  await page.evaluate((key) => localStorage.setItem('litegen_api_key', key), MASTER_KEY);
  await page.goto('/');
  await expect(page.getByTestId('auth-status')).toBeVisible({ timeout: 5_000 });
  await expect(page.getByTestId('auth-status')).toContainText('Authenticated ✓');
});
