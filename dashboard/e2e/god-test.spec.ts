import { test, expect } from '@playwright/test';
import { createServer, Server } from 'http';

const MASTER_KEY = process.env.PLAYWRIGHT_MASTER_KEY ?? 'test-master-key-please-rotate';

function startWebhookReceiver(port: number): Promise<{ server: Server; received: any[] }> {
  return new Promise((resolve) => {
    const received: any[] = [];
    const server = createServer((req, res) => {
      let body = '';
      req.on('data', (chunk) => (body += chunk));
      req.on('end', () => {
        try {
          received.push({ url: req.url, headers: req.headers, body: JSON.parse(body || '{}') });
        } catch {
          received.push({ url: req.url, headers: req.headers, body });
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

  // 10. Sign out (owner is now admin)
  await page.locator('[data-testid="user-menu-toggle"]').click();
  await page.locator('[data-testid="user-menu-signout"]').click();
  await page.waitForURL('**/login');

  // ─── Existing master-key path begins ─────────────────────────────────────────
  // Navigate to / — no session exists, so AuthBar auto-shows (no localStorage pref).

  // ─── Step 1: Auth setup ──────────────────────────────────────────────────────
  await page.goto('/');

  // Fill the AuthBar's key input and save
  const keyInput = page.getByTestId('api-key-input');
  await expect(keyInput).toBeVisible({ timeout: 10_000 });
  await keyInput.fill(MASTER_KEY);
  await page.getByTestId('save-key-btn').click();

  // Assert authenticated state appears
  await expect(page.getByTestId('auth-status')).toBeVisible({ timeout: 5_000 });
  await expect(page.getByTestId('auth-status')).toContainText('Authenticated ✓');

  // ─── Step 2: Overview page ───────────────────────────────────────────────────
  // Navigate away and back to reload data with auth now set
  await page.getByRole('link', { name: 'Logs' }).click();
  await page.getByRole('link', { name: 'Overview' }).click();
  await expect(page.getByText('Total Requests')).toBeVisible({ timeout: 10_000 });
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
  await page.getByRole('link', { name: 'Logs' }).click();
  // Wait for logs table to load (may show "No logs yet")
  await expect(page.locator('table')).toBeVisible({ timeout: 10_000 });

  // Try pagination if multiple pages exist
  const nextBtn = page.getByRole('button', { name: 'Next' });
  const prevBtn = page.getByRole('button', { name: 'Previous' });
  if (await nextBtn.isVisible()) {
    const isNextEnabled = !await nextBtn.isDisabled();
    if (isNextEnabled) {
      await nextBtn.click();
      await expect(page.locator('table')).toBeVisible({ timeout: 5_000 });
      await prevBtn.click();
      await expect(page.locator('table')).toBeVisible({ timeout: 5_000 });
    }
  }

  // ─── Step 4: Models page ─────────────────────────────────────────────────────
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

  // Assert the panel includes schema content (params with "seed" or "size")
  const schemaJson = page.getByTestId('model-schema-json');
  await expect(schemaJson).toBeVisible({ timeout: 10_000 });
  const schemaText = await schemaJson.textContent();
  expect(schemaText).toMatch(/seed|size|params/i);

  // Close the panel
  await page.getByTestId('close-model-panel').click();
  await expect(detailPanel).not.toBeVisible({ timeout: 3_000 });

  // ─── Step 5: Health page ─────────────────────────────────────────────────────
  await page.getByRole('link', { name: 'Health' }).click();
  await expect(page.locator('table')).toBeVisible({ timeout: 10_000 });

  // Click Refresh
  const refreshBtn = page.getByRole('button', { name: 'Refresh' });
  await expect(refreshBtn).toBeVisible();
  await refreshBtn.click();
  await expect(page.locator('table')).toBeVisible({ timeout: 5_000 });

  // Click Clear Cache — should not show error alert
  const clearCacheBtn = page.getByRole('button', { name: 'Clear Cache' });
  await expect(clearCacheBtn).toBeVisible();
  await clearCacheBtn.click();
  // Ensure no error appeared
  await expect(page.locator('.alert-error')).not.toBeVisible({ timeout: 3_000 });

  // ─── Step 6: Keys page — full CRUD ──────────────────────────────────────────
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

  // ─── NEW: Playground ──────────────────────────────────────────────────────────
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

  // Uncheck strict so negative_prompt is accepted
  const strictCheckbox = page.locator('[data-testid="playground-strict"]');
  await strictCheckbox.uncheck();

  // Fill negative prompt (exercising the field per spec)
  await page.locator('[data-testid="playground-negative-prompt"]').fill('blurry, low quality');

  await page.locator('[data-testid="playground-generate"]').click();

  // Wait for the image to render
  const imgLocator = page.locator('[data-testid="playground-image"]');
  await expect(imgLocator).toBeVisible({ timeout: 15_000 });
  const src = await imgLocator.getAttribute('src');
  expect(src, 'image src must be populated').toBeTruthy();
  // Real PNG base64 must be substantially longer than a 4-byte placeholder
  expect(src!.length, 'image src must be a real PNG, not a placeholder').toBeGreaterThan(1000);

  // Switch to Request JSON tab
  await page.locator('[data-testid="playground-tab-request"]').click();
  const reqText = await page.locator('[data-testid="playground-request-json"]').textContent();
  expect(reqText).toContain('"model": "mock/visual-image-gen"');
  expect(reqText).toContain('"prompt": "a playwright cat"');

  // Switch to Response JSON tab
  await page.locator('[data-testid="playground-tab-response"]').click();
  const resText = await page.locator('[data-testid="playground-response-json"]').textContent();
  expect(resText).toMatch(/"id":/);

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

  // Re-submit history row 0
  await page.locator('[data-testid="playground-history-resubmit-0"]').click();
  // Wait for second response
  await expect(page.locator('[data-testid="playground-history-row-1"]')).toBeVisible({ timeout: 10_000 });

  // Delete history row 1
  await page.locator('[data-testid="playground-history-delete-1"]').click();
  // Now row 1 should no longer exist; only row 0
  await expect(page.locator('[data-testid="playground-history-row-1"]')).toHaveCount(0);
  await expect(histRow0).toBeVisible();

  // ─── NEW: Generations ─────────────────────────────────────────────────────────
  // First trigger a video generation via API directly
  const masterKey = process.env.PLAYWRIGHT_MASTER_KEY ?? 'test-master-key-please-rotate';
  const videoResp = await page.request.post('http://127.0.0.1:5099/v1/videos/generations', {
    headers: {
      Authorization: `Bearer ${masterKey}`,
      'Content-Type': 'application/json',
    },
    data: { model: 'mock/video-gen', prompt: 'a playwright cat dancing' },
  });
  expect(videoResp.status()).toBe(200);
  const videoBody = await videoResp.json();
  const videoId = videoBody.id as string;
  expect(videoId).toBeTruthy();

  // Navigate to Generations
  await page.click('a[href="/generations"]');
  await page.waitForURL('**/generations');

  // Wait for the row to appear (auto-refresh runs every 3s on pending rows)
  const rowLocator = page.locator(`[data-testid="gen-row-${videoId}"]`);
  await expect(rowLocator).toBeVisible({ timeout: 15_000 });

  // Click the id cell to expand
  await page.locator(`[data-testid="gen-expand-${videoId}"]`).click();
  await expect(page.locator(`[data-testid="gen-detail-${videoId}"]`)).toBeVisible();

  // Wait for status to be 'pending' or 'processing' so cancel is available
  // (mock provider could already complete by now — be tolerant)
  const genStatus = await page.locator(`[data-testid="gen-status-${videoId}"]`).textContent();
  if (genStatus && (genStatus.includes('pending') || genStatus.includes('processing'))) {
    await page.locator(`[data-testid="gen-cancel-${videoId}"]`).click();
    // After cancel, status should change to 'cancelled'
    await expect(page.locator(`[data-testid="gen-status-${videoId}"]`)).toContainText('cancelled', { timeout: 5_000 });
  }

  // Pagination buttons — verify they render and are in correct enabled/disabled state
  const genPrevBtn = page.getByRole('button', { name: 'Previous' });
  const genNextBtn = page.getByRole('button', { name: 'Next' });
  if (await genPrevBtn.isVisible()) {
    // Just click to exercise the button (may be disabled, which is fine)
    const isPrevDisabled = await genPrevBtn.isDisabled();
    if (!isPrevDisabled) await genPrevBtn.click();
  }
  if (await genNextBtn.isVisible()) {
    const isNextDisabled = await genNextBtn.isDisabled();
    if (!isNextDisabled) await genNextBtn.click();
  }

  // Reload and assert the row still exists (persistence)
  await page.reload();
  await expect(rowLocator).toBeVisible();

  // ─── NEW: Overview auto-refresh / cost chart / quota table ──────────────────
  await page.click('a[href="/"]');
  await page.waitForURL('http://127.0.0.1:5174/');

  // Toggle auto-refresh on (testid is on the <label>, so locate the inner checkbox)
  const refreshToggle = page.locator('[data-testid="overview-autorefresh"] input[type="checkbox"]');
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

  // Wait 6 seconds for auto-refresh to fire at least once
  await page.waitForTimeout(6000);

  // Toggle off
  await refreshToggle.uncheck();

  // ─── NEW: Logs filters + trace panel drill-down ───────────────────────────────
  await page.click('a[href="/logs"]');
  await page.waitForURL('**/logs');

  // Apply a model filter — use mock/visual-image-gen (the model we just generated with)
  await page.locator('[data-testid="logs-filter-model"]').selectOption({ label: 'mock/visual-image-gen' });
  await page.locator('[data-testid="logs-filter-apply"]').click();
  await expect(page).toHaveURL(/model=mock%2Fvisual-image-gen/);

  // Export CSV — intercept download
  const downloadPromise = page.waitForEvent('download', { timeout: 10_000 });
  await page.locator('[data-testid="logs-export-csv"]').click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toMatch(/logs-\d{8}\.csv/);

  // Click the first row to open trace panel
  const firstRow = page.locator('[data-testid^="logs-row-"]').first();
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
  await expect(page.locator('[data-testid="trace-prompt-text"]')).toBeVisible();

  await page.locator('[data-testid="trace-tab-params"]').click();
  await expect(page.locator('[data-testid="trace-params-table"]')).toBeVisible();

  await page.locator('[data-testid="trace-tab-response"]').click();
  await expect(page.locator('[data-testid="trace-response-json"]')).toBeVisible();

  // Close panel via close button
  await page.locator('[data-testid="trace-panel-close"]').click();
  await expect(page.locator('[data-testid="trace-panel"]')).not.toBeVisible({ timeout: 3_000 });

  // Clear filters
  await page.locator('[data-testid="logs-filter-clear"]').click();
  await expect(page).not.toHaveURL(/model=/);

  // ─── NEW: Keys rotate + test-webhook ─────────────────────────────────────────
  const webhookPort = 5180;
  const { server: webhookServer, received: webhookReceived } = await startWebhookReceiver(webhookPort);

  try {
    await page.click('a[href="/keys"]');
    await page.waitForURL('**/keys');

    // Create a fresh key with a webhook_url pointing to the receiver
    await page.locator('[data-testid="new-key-name"]').fill('webhook-test-key');
    await page.locator('[data-testid="new-key-scopes"]').fill('generate,read,admin');
    await page.locator('[data-testid="create-key-btn"]').click();
    await expect(page.locator('[data-testid="key-created-banner"]')).toBeVisible({ timeout: 10_000 });

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

    // Copy prefix check — grant clipboard permissions for the context first
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    // Find any active row
    const anyActiveRow = page.locator('[data-testid^="key-row-"]').filter({ has: page.locator('.badge.healthy') }).first();
    const activeTestId = await anyActiveRow.getAttribute('data-testid');
    const activeId = activeTestId!.replace('key-row-', '');
    await page.locator(`[data-testid="key-copy-prefix-${activeId}"]`).click();
    // Toast should appear
    await expect(page.locator('[data-testid^="toast-"]')).toBeVisible({ timeout: 3000 });
  } finally {
    await new Promise<void>((resolve) => webhookServer.close(() => resolve()));
  }

  // ─── NEW: Models filters + copy curl ─────────────────────────────────────────
  await page.click('a[href="/models"]');
  await page.waitForURL('**/models');

  // Filter to mock provider
  await page.locator('[data-testid="models-filter-provider"]').selectOption({ label: 'mock' });

  // Assert at least 14 mock model rows (4 original + 12 new config-demo models)
  await page.waitForTimeout(500); // let filter apply
  const mockRowCount = await page.locator('[data-testid^="model-row-"]').count();
  expect(mockRowCount >= 14).toBe(true);

  // Exercise media-type radio buttons
  const mediaTypeAll = page.locator('[data-testid="models-filter-media-type"] input[value=""]');
  const mediaTypeImage = page.locator('[data-testid="models-filter-media-type"] input[value="image"]');
  const mediaTypeVideo = page.locator('[data-testid="models-filter-media-type"] input[value="video"]');
  if (await mediaTypeImage.isVisible()) {
    await mediaTypeImage.click();
    await mediaTypeVideo.click();
    await mediaTypeAll.click(); // reset to All
  }

  // Click at least one capability checkbox (first available one)
  const firstCapCheckbox = page.locator('[data-testid^="models-filter-capability-"]').first();
  if (await firstCapCheckbox.isVisible()) {
    await firstCapCheckbox.click(); // check it
    await firstCapCheckbox.click(); // uncheck it (reset)
  }

  // Click on the mock/image-gen row to open detail
  await page.locator('[data-testid="model-row-mock/image-gen"]').click();

  // Click Copy as curl (clipboard permissions already granted)
  await page.locator('[data-testid="models-copy-curl-mock/image-gen"]').click();

  // Read the clipboard
  const clipboard = await page.evaluate(() => navigator.clipboard.readText());
  expect(clipboard).toContain('mock/image-gen');
  expect(clipboard).toContain('curl -X POST');

  // Close the model detail panel
  await page.getByTestId('close-model-panel').click();

  // ─── Cluster 3a: Health probes ───────────────────────────────────────────────
  await page.getByRole('link', { name: 'Health' }).click();
  await expect(page.locator('table')).toBeVisible({ timeout: 10_000 });

  // Liveness badge — wait for probe to resolve (starts as "…")
  await expect(page.getByTestId('health-liveness-badge')).toBeVisible({ timeout: 10_000 });
  await expect(page.getByTestId('health-liveness-badge')).toContainText('alive', { timeout: 10_000 });

  // Readiness badge — mock provider + sqlite DB = ready
  await expect(page.getByTestId('health-readiness-badge')).toBeVisible({ timeout: 10_000 });
  await expect(page.getByTestId('health-readiness-badge')).toContainText('ready', { timeout: 10_000 });

  // ─── Cluster 3b: X-RateLimit headers ─────────────────────────────────────────
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
  const rpmResp = await page.request.post('http://127.0.0.1:5099/v1/images/generations', {
    headers: {
      Authorization: `Bearer ${rpmKeyValue}`,
      'Content-Type': 'application/json',
    },
    data: { model: 'mock/image-gen', prompt: 'ratelimit test' },
  });
  // Should succeed (first out of 60 RPM)
  expect(rpmResp.status()).toBeLessThan(500);
  // X-RateLimit-Limit should be present and equal to 60
  const rateLimitHeader = rpmResp.headers()['x-ratelimit-limit'];
  expect(rateLimitHeader).toBe('60');
  const rateLimitRemaining = rpmResp.headers()['x-ratelimit-remaining'];
  expect(rateLimitRemaining).toBeTruthy();
  expect(Number(rateLimitRemaining)).toBeGreaterThanOrEqual(0);

  // ─── Cluster 3c: Audit page ───────────────────────────────────────────────────
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

  // Clear filter
  await page.locator('[data-testid="audit-filter-clear"]').click();
  await expect(page).not.toHaveURL(/action=key\.create/, { timeout: 5_000 });

  // ─── Cluster 3d: Webhook deliveries panel ────────────────────────────────────
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

  // The panel must be visible — delivery rows may be 0 if no generation-completion
  // webhook fires were triggered (test-webhook does not log to webhook_deliveries).
  // For a fuller assertion, trigger a completed video gen and check delivery rows.
  // For now, verify the panel renders without error (the API call to /webhook-deliveries succeeded).
  const panelText = await page.locator(`[data-testid="key-deliveries-panel-${deliveryKeyId}"]`).textContent();
  expect(panelText).toBeTruthy();
  // Panel should show either deliveries or an empty-state message, but NOT an error
  expect(panelText).not.toContain('Network error');
  expect(panelText).not.toContain('Unauthorized');

  // ─── Cluster 3e: Backpressure sanity ping ────────────────────────────────────
  // Lib tests cover backpressure logic; just verify the route is still healthy.
  const sanityResp = await page.request.post('http://127.0.0.1:5099/v1/images/generations', {
    headers: {
      Authorization: `Bearer ${MASTER_KEY}`,
      'Content-Type': 'application/json',
    },
    data: { model: 'mock/image-gen', prompt: 'sanity ping' },
  });
  expect(sanityResp.status()).toBeLessThan(500);

  // ─── Step 7: Sign out / re-auth round trip ───────────────────────────────────
  await page.getByRole('button', { name: 'Sign out' }).click();

  // Input field reappears
  await expect(page.getByTestId('api-key-input')).toBeVisible({ timeout: 5_000 });
  await expect(page.getByTestId('auth-status')).not.toBeVisible();

  // Re-authenticate
  await page.getByTestId('api-key-input').fill(MASTER_KEY);
  await page.getByTestId('save-key-btn').click();
  await expect(page.getByTestId('auth-status')).toBeVisible({ timeout: 5_000 });
  await expect(page.getByTestId('auth-status')).toContainText('Authenticated ✓');
});
