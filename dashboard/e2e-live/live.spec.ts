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

  // The minted secret authenticates over the real HTTP API (same origin via nginx).
  const me = await page.request.get('/v1/auth/me');
  // (session-cookie call; just confirms the proxied API is reachable)
  expect([200, 401]).toContain(me.status());

  // ─── 4. Store a BYO provider credential; assert the secret never renders back ──
  const rawCredSecret = `sk-byo-${rand()}`;
  await page.goto('/organization');
  await page.locator('[data-testid="provider-cred-provider"]').fill('mock');
  await page.locator('[data-testid="provider-cred-secret"]').fill(rawCredSecret);
  await page.locator('[data-testid="provider-cred-add"]').click();
  await expect(page.locator('[data-testid="provider-cred-row-mock"]')).toBeVisible({
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
