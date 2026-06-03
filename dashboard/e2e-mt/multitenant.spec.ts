import { test, expect } from '@playwright/test';

// Backend (litegen binary, hosted mode) is served directly on :5199 by the
// playwright.multitenant.config.ts webServer. The dashboard (vite on :5274)
// proxies /v1 etc. to it so session cookies are same-origin.
const BACKEND = 'http://127.0.0.1:5199';
const PW = 'super-secret-password-123';

// Unique suffix so reruns against a (theoretically) shared DB never collide.
// The DB is recreated each run via `rm -f` in the webServer command, but unique
// emails make local debugging reruns painless too.
const rand = () => Math.random().toString(36).slice(2, 10);

test('hosted multi-tenant: signup, apps, keys, BYO creds, invites, isolation, real API auth', async ({
  page,
  browser,
}) => {
  // Backend lowercases emails on signup/invite, so use lowercase to match the
  // rendered values exactly.
  const ownerAEmail = `ownera+${rand()}@litegen.test`;
  const memberAEmail = `membera+${rand()}@litegen.test`;
  const ownerBEmail = `ownerb+${rand()}@litegen.test`;

  // ─── 1. Sign up org A owner ──────────────────────────────────────────────────
  await page.goto('/signup');
  await page.locator('[data-testid="signup-org-name"]').fill('Acme A');
  await page.locator('[data-testid="signup-email"]').fill(ownerAEmail);
  await page.locator('[data-testid="signup-password"]').fill(PW);
  await page.locator('[data-testid="signup-confirm-password"]').fill(PW);
  await page.locator('[data-testid="signup-submit"]').click();
  await page.waitForURL('**/');

  // Full reload so TenantProvider re-runs /auth/me and populates orgs/apps
  // (SPA navigate('/') after signup does not remount the provider).
  await page.goto('/');
  await expect(page.locator('[data-testid="user-menu-email"]')).toContainText(ownerAEmail, {
    timeout: 15_000,
  });
  const orgSwitcher = page.locator('[data-testid="org-switcher"]');
  await expect(orgSwitcher).toBeVisible({ timeout: 15_000 });
  // Selected option label is "Acme A".
  await expect(orgSwitcher.locator('option:checked')).toHaveText('Acme A');

  // ─── 2. Create a 2nd app ─────────────────────────────────────────────────────
  await page.goto('/organization');
  await page.locator('[data-testid="app-create-name"]').fill('staging');
  await page.locator('[data-testid="app-create-submit"]').click();
  // An app-row for "staging" appears (rows are keyed by app id; match by text).
  await expect(
    page.locator('[data-testid^="app-row-"]').filter({ hasText: 'staging' }),
  ).toBeVisible({ timeout: 15_000 });

  // ─── 3. Create an API key ────────────────────────────────────────────────────
  await page.goto('/keys');
  await expect(page.locator('table')).toBeVisible({ timeout: 15_000 });
  await page.locator('[data-testid="new-key-name"]').fill('prod-key');
  await page.locator('[data-testid="new-key-scopes"]').fill('generate,read');
  await page.locator('[data-testid="create-key-btn"]').click();

  const secretLocator = page.locator('[data-testid="key-secret"]');
  await expect(secretLocator).toBeVisible({ timeout: 15_000 });
  const secret = (await secretLocator.textContent())!.trim();
  expect(secret.startsWith('sk_live_')).toBe(true);

  const publicIdLocator = page.locator('[data-testid="key-public-id"]');
  const publicId = (await publicIdLocator.textContent())!.trim();
  expect(publicId.startsWith('pk_live_')).toBe(true);

  // ─── 4. Add a provider credential (BYO) ──────────────────────────────────────
  const rawCredSecret = 'sk-mock-abcd1234';
  await page.goto('/organization');
  await page.locator('[data-testid="provider-cred-provider"]').fill('mock');
  await page.locator('[data-testid="provider-cred-secret"]').fill(rawCredSecret);
  await page.locator('[data-testid="provider-cred-add"]').click();
  await expect(page.locator('[data-testid="provider-cred-row-mock"]')).toBeVisible({
    timeout: 15_000,
  });
  // The raw secret must never be rendered back to the page.
  await expect(page.locator('body')).not.toContainText(rawCredSecret);

  // ─── 5. Invite a member ──────────────────────────────────────────────────────
  await page.goto('/members');
  await page.locator('[data-testid="members-invite-btn"]').click();
  await page.locator('[data-testid="invite-email"]').fill(memberAEmail);
  await page.locator('[data-testid="invite-role"]').selectOption('member');
  await page.locator('[data-testid="invite-send"]').click();
  const inviteTokenLocator = page.locator('[data-testid="invite-dev-token"]');
  await expect(inviteTokenLocator).toBeVisible({ timeout: 15_000 });
  const inviteToken = (await inviteTokenLocator.textContent())!.trim();
  expect(inviteToken.length).toBeGreaterThan(20);
  await page.locator('[data-testid="invite-dev-token-close"]').click();
  await expect(inviteTokenLocator).not.toBeVisible({ timeout: 5_000 });

  // ─── 6. Accept invite as member in a fresh context ───────────────────────────
  const memberCtx = await browser.newContext();
  const mp = await memberCtx.newPage();
  try {
    await mp.goto(`http://127.0.0.1:5274/invite/${inviteToken}`);
    await expect(mp.locator('[data-testid="accept-email"]')).toContainText(memberAEmail, {
      timeout: 15_000,
    });
    await mp.locator('[data-testid="accept-password"]').fill(PW);
    await mp.locator('[data-testid="accept-confirm-password"]').fill(PW);
    await mp.locator('[data-testid="accept-submit"]').click();
    await mp.waitForURL('**/');
    await mp.goto('http://127.0.0.1:5274/');
    await expect(mp.locator('[data-testid="user-menu-email"]')).toContainText(memberAEmail, {
      timeout: 15_000,
    });

    // As the member, /members must NOT show the invite button (role-gated).
    await mp.goto('http://127.0.0.1:5274/members');
    await expect(mp.locator('[data-testid="members-table"]')).toBeVisible({ timeout: 15_000 });
    await expect(mp.locator('[data-testid="members-invite-btn"]')).toHaveCount(0);
  } finally {
    await memberCtx.close();
  }

  // ─── 7. Cross-tenant isolation: org B owner ──────────────────────────────────
  const ctxB = await browser.newContext();
  const pb = await ctxB.newPage();
  try {
    await pb.goto('http://127.0.0.1:5274/signup');
    await pb.locator('[data-testid="signup-org-name"]').fill('Acme B');
    await pb.locator('[data-testid="signup-email"]').fill(ownerBEmail);
    await pb.locator('[data-testid="signup-password"]').fill(PW);
    await pb.locator('[data-testid="signup-confirm-password"]').fill(PW);
    await pb.locator('[data-testid="signup-submit"]').click();
    await pb.waitForURL('**/');
    await pb.goto('http://127.0.0.1:5274/');
    await expect(pb.locator('[data-testid="user-menu-email"]')).toContainText(ownerBEmail, {
      timeout: 15_000,
    });

    const orgSwitcherB = pb.locator('[data-testid="org-switcher"]');
    await expect(orgSwitcherB).toBeVisible({ timeout: 15_000 });
    const optionTexts = await orgSwitcherB.locator('option').allTextContents();
    expect(optionTexts).toContain('Acme B');
    expect(optionTexts).not.toContain('Acme A');

    // Org B has no keys.
    await pb.goto('http://127.0.0.1:5274/keys');
    await expect(pb.locator('table')).toBeVisible({ timeout: 15_000 });
    await expect(pb.locator('[data-testid^="key-row-"]')).toHaveCount(0);
  } finally {
    await ctxB.close();
  }

  // ─── 8. Use the captured key via the real API ────────────────────────────────
  // Call the backend directly on :5199 with the sk_live_ secret — proves the
  // minted key authenticates over real HTTP, scoped to org A's tenant.
  const r = await page.request.post(`${BACKEND}/v1/images/generations`, {
    headers: { Authorization: `Bearer ${secret}` },
    data: { model: 'mock/image-gen', prompt: 'a cat' },
  });
  expect(r.status()).toBe(200);
});
