import { test, expect } from '@playwright/test';

// Drives the hosted multi-tenant flow through the REAL deployed dashboard on the
// droplet (baseURL from playwright.live.config.ts). Runs against the shared prod
// DB, so every run uses unique emails and never assumes a clean database. No
// dev-only flags required (no invite-dev-token / mock-generation dependency).
const PW = 'super-secret-password-123';
const rand = () => Math.random().toString(36).slice(2, 10);

test('LIVE hosted multi-tenant: signup -> org -> app -> id/secret key -> BYO cred -> tenant isolation', async ({
  page,
  browser,
}) => {
  const ownerAEmail = `livea+${rand()}@litegen.test`;
  const ownerBEmail = `liveb+${rand()}@litegen.test`;
  const orgAName = `LiveOrgA-${rand()}`;
  const orgBName = `LiveOrgB-${rand()}`;

  // ─── 1. Sign up org A owner against the live URL ─────────────────────────────
  await page.goto('/signup');
  await page.locator('[data-testid="signup-org-name"]').fill(orgAName);
  await page.locator('[data-testid="signup-email"]').fill(ownerAEmail);
  await page.locator('[data-testid="signup-password"]').fill(PW);
  await page.locator('[data-testid="signup-confirm-password"]').fill(PW);
  await page.locator('[data-testid="signup-submit"]').click();
  await page.waitForURL('**/');

  await page.goto('/'); // full reload so TenantProvider hydrates orgs/apps
  await expect(page.locator('[data-testid="user-menu-email"]')).toContainText(ownerAEmail, {
    timeout: 20_000,
  });
  const orgSwitcher = page.locator('[data-testid="org-switcher"]');
  await expect(orgSwitcher).toBeVisible({ timeout: 20_000 });
  await expect(orgSwitcher.locator('option:checked')).toHaveText(orgAName);

  // ─── 2. Create a second application ──────────────────────────────────────────
  await page.goto('/organization');
  await page.locator('[data-testid="app-create-name"]').fill('staging');
  await page.locator('[data-testid="app-create-submit"]').click();
  await expect(
    page.locator('[data-testid^="app-row-"]').filter({ hasText: 'staging' }),
  ).toBeVisible({ timeout: 20_000 });

  // ─── 3. Mint an id/secret API key ────────────────────────────────────────────
  await page.goto('/keys');
  await expect(page.locator('table')).toBeVisible({ timeout: 20_000 });
  await page.locator('[data-testid="new-key-name"]').fill('prod-key');
  await page.locator('[data-testid="new-key-scopes"]').fill('generate,read');
  await page.locator('[data-testid="create-key-btn"]').click();

  const secretLocator = page.locator('[data-testid="key-secret"]');
  await expect(secretLocator).toBeVisible({ timeout: 20_000 });
  const secret = (await secretLocator.textContent())!.trim();
  expect(secret.startsWith('sk_live_')).toBe(true);
  const publicId = (await page.locator('[data-testid="key-public-id"]').textContent())!.trim();
  expect(publicId.startsWith('pk_live_')).toBe(true);

  // The proxied API is mounted under /api (Caddy strips the prefix before
  // reverse-proxying to litegen-core); a bare `/v1/...` path would instead be
  // served the SPA's index.html (200) and never reach the API. page.request
  // shares the signup session cookie, so /api/v1/auth/me must return 200 and
  // identify *this* owner.
  const me = await page.request.get('/api/v1/auth/me');
  expect(me.status()).toBe(200);
  const meBody = await me.json();
  expect(meBody.user.email).toBe(ownerAEmail);

  // ─── 4. Store a BYO provider credential; assert the secret never renders back ──
  // The form is catalog-driven: pick a provider from the (async-loaded) select,
  // fill its credential field(s), save, and confirm the row appears.
  const rawCredSecret = `sk-byo-${rand()}`;
  await page.goto('/organization');
  const providerSelect = page.locator('[data-testid="provider-cred-provider"]');
  await expect(providerSelect).toBeEnabled({ timeout: 20_000 }); // waits for the catalog fetch
  const provider = (await providerSelect.locator('option').first().getAttribute('value'))!;
  await providerSelect.selectOption(provider);
  const credFields = page.locator('[data-testid^="provider-cred-field-"][data-testid$="-0"]');
  await expect(credFields.first()).toBeVisible({ timeout: 10_000 });
  const fieldCount = await credFields.count();
  for (let i = 0; i < fieldCount; i++) await credFields.nth(i).fill(rawCredSecret);
  await page.locator('[data-testid="provider-cred-add"]').click();
  await expect(page.locator(`[data-testid="provider-cred-row-${provider}"]`)).toBeVisible({
    timeout: 20_000,
  });
  await expect(page.locator('body')).not.toContainText(rawCredSecret);

  // ─── 5. Cross-tenant isolation: a second org sees none of org A ──────────────
  const ctxB = await browser.newContext();
  const pb = await ctxB.newPage();
  try {
    await pb.goto('/signup');
    await pb.locator('[data-testid="signup-org-name"]').fill(orgBName);
    await pb.locator('[data-testid="signup-email"]').fill(ownerBEmail);
    await pb.locator('[data-testid="signup-password"]').fill(PW);
    await pb.locator('[data-testid="signup-confirm-password"]').fill(PW);
    await pb.locator('[data-testid="signup-submit"]').click();
    await pb.waitForURL('**/');
    await pb.goto('/');
    await expect(pb.locator('[data-testid="user-menu-email"]')).toContainText(ownerBEmail, {
      timeout: 20_000,
    });

    const orgSwitcherB = pb.locator('[data-testid="org-switcher"]');
    await expect(orgSwitcherB).toBeVisible({ timeout: 20_000 });
    const optionTexts = await orgSwitcherB.locator('option').allTextContents();
    expect(optionTexts).toContain(orgBName);
    expect(optionTexts).not.toContain(orgAName);

    await pb.goto('/keys');
    await expect(pb.locator('table')).toBeVisible({ timeout: 20_000 });
    await expect(pb.locator('[data-testid^="key-row-"]')).toHaveCount(0);
  } finally {
    await ctxB.close();
  }
});
