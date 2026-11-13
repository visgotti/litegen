import { test, expect, type Browser, type Page } from '@playwright/test';

// What a hosted tenant sees by role (playwright.multitenant.config.ts):
// - /health is the operator's (registry P1: 403 for a tenant), so the Health
//   page keeps the probes and names provider health as the operator's;
// - a Member keeps keys and generations but not an app's settings, limits,
//   model access, storage and access, or the org's vendor keys (security,
//   README roles table), so those forms are not offered;
// - a Viewer reads only: no generation or quote UI.
// The server enforces every one of these; the page only stops offering them.

const PW = 'super-secret-password-123';
const rand = () => Math.random().toString(36).slice(2, 10);

/** A new org's owner, signed in on `page`. */
async function signUpOwner(page: Page, org: string): Promise<string> {
  const email = `owner+${rand()}@litegen.test`;
  await page.goto('/signup');
  await page.getByTestId('signup-org-name').fill(org);
  await page.getByTestId('signup-email').fill(email);
  await page.getByTestId('signup-password').fill(PW);
  await page.getByTestId('signup-confirm-password').fill(PW);
  await page.getByTestId('signup-submit').click();
  await page.waitForURL('**/');
  await page.goto('/');
  await expect(page.getByTestId('user-menu-email')).toContainText(email, { timeout: 15_000 });
  return email;
}

/** Invite someone to the owner's org with `role`, and sign them in on a page of their own. */
async function invitee(owner: Page, browser: Browser, role: 'member' | 'viewer'): Promise<Page> {
  return (await invited(owner, browser, role)).page;
}

/** As `invitee`, with the invitee's email. */
async function invited(owner: Page, browser: Browser, role: 'member' | 'viewer'): Promise<{ page: Page; email: string }> {
  const email = `${role}+${rand()}@litegen.test`;
  await owner.goto('/members');
  await owner.getByTestId('members-invite-btn').click();
  await owner.getByTestId('invite-email').fill(email);
  await owner.getByTestId('invite-role').selectOption(role);
  await owner.getByTestId('invite-send').click();
  const token = (await owner.getByTestId('invite-dev-token').textContent({ timeout: 15_000 }))!.trim();
  await owner.getByTestId('invite-dev-token-close').click();
  const page = await (await browser.newContext()).newPage();
  await page.goto(`http://127.0.0.1:5274/invite/${token}`);
  await expect(page.getByTestId('accept-email')).toContainText(email, { timeout: 15_000 });
  await page.getByTestId('accept-password').fill(PW);
  await page.getByTestId('accept-confirm-password').fill(PW);
  await page.getByTestId('accept-submit').click();
  await page.waitForURL('**/');
  await page.goto('http://127.0.0.1:5274/');
  await expect(page.getByTestId('user-menu-email')).toContainText(email, { timeout: 15_000 });
  return { page, email };
}

test("a tenant's Health page keeps the probes and names provider health as the operator's", async ({ page }) => {
  await signUpOwner(page, 'Health Co');
  const health = page.waitForResponse(r => new URL(r.url()).pathname === '/health');
  // By the nav link: /health typed into the address bar is the backend's own route (the dev proxy's).
  await page.getByRole('link', { name: 'Health' }).click();
  expect((await health).status(), 'provider health is the operator\'s (registry P1)').toBe(403);
  await expect(page.getByTestId('health-provider-note')).toContainText("operator's");
  await expect(page.getByTestId('health-liveness-badge')).toContainText('alive');
  await expect(page.getByTestId('health-readiness-badge')).toContainText('ready');
  await expect(page.locator('.alert-error')).toHaveCount(0);
});

test("a Member is not offered an app's settings, limits, storage, access or vendor keys", async ({ page, browser }) => {
  await signUpOwner(page, 'Member Co');
  const member = await invitee(page, browser, 'member');
  try {
    await member.goto('http://127.0.0.1:5274/organization');
    // The page has drawn (the access card is there for every role); now what it offers.
    await expect(member.getByTestId('access-card')).toBeVisible({ timeout: 15_000 });
    for (const id of ['org-name-save', 'app-create-submit', 'storage-save', 'access-save', 'access-revoke']) {
      await expect(member.getByTestId(id), `${id} is not offered`).toHaveCount(0);
    }
    await expect(member.locator('[data-testid^="app-delete-"]')).toHaveCount(0);
    await expect(member.getByTestId('org-readonly-note')).toBeVisible();

    await member.goto('http://127.0.0.1:5274/budgets');
    await expect(member.getByTestId('app-limits')).toBeVisible({ timeout: 15_000 });
    for (const id of ['org-limits-save', 'app-limits-save', 'member-limits-save']) {
      await expect(member.getByTestId(id), `${id} is not offered`).toHaveCount(0);
    }

    await member.goto('http://127.0.0.1:5274/providers');
    await expect(member.locator('[data-testid^="provider-key-save-"]')).toHaveCount(0);

    // A Member still makes generations.
    await member.goto('http://127.0.0.1:5274/playground');
    await expect(member.getByTestId('playground-generate')).toBeVisible({ timeout: 15_000 });
  } finally {
    await member.context().close();
  }
});

test('a Viewer reads, with no generation or quote UI', async ({ page, browser }) => {
  await signUpOwner(page, 'Viewer Co');
  const viewer = await invitee(page, browser, 'viewer');
  try {
    await expect(viewer.getByRole('link', { name: 'Playground' })).toHaveCount(0);
    await viewer.goto('http://127.0.0.1:5274/playground');
    await expect(viewer.getByTestId('playground-viewer-note')).toBeVisible({ timeout: 15_000 });
    await expect(viewer.getByTestId('playground-generate')).toHaveCount(0);
    await expect(viewer.getByTestId('pg-mode-compare')).toHaveCount(0);
    // Reading stays: the Generations page.
    await viewer.goto('http://127.0.0.1:5274/generations');
    await expect(viewer.locator('.alert-error')).toHaveCount(0);
  } finally {
    await viewer.context().close();
  }
});

test("the key form says, in place, why a Viewer's key cannot hold generate", async ({ page, browser }) => {
  await signUpOwner(page, 'Keys Co');
  const { page: member, email } = await invited(page, browser, 'member');
  try {
    // A Member makes a read key of their own.
    await member.goto('http://127.0.0.1:5274/keys');
    await member.getByTestId('new-key-name').fill('member-key');
    await member.getByTestId('new-key-scopes').fill('read');
    await member.getByTestId('create-key-btn').click();
    await expect(member.getByTestId('key-secret')).toBeVisible({ timeout: 15_000 });
  } finally {
    await member.context().close();
  }

  // The owner makes the Member a Viewer, then asks for generate on their key (late-a: 403).
  await page.goto('/members');
  await page.getByTestId(`member-edit-${email}`).click();
  await page.getByTestId(`member-edit-role-${email}`).selectOption('viewer');
  await page.getByTestId(`member-edit-save-${email}`).click();
  await expect(page.getByTestId(`member-role-${email}`)).toContainText(/viewer/i);
  await page.goto('/keys');
  const row = page.locator('[data-testid^="key-row-"]').filter({ hasText: 'member-key' });
  await expect(row).toBeVisible({ timeout: 15_000 });
  const keyId = (await row.getAttribute('data-testid'))!.replace('key-row-', '');
  await page.getByTestId(`edit-key-${keyId}`).click();
  await page.getByTestId('edit-scopes').fill('generate,read');
  const patched = page.waitForResponse(r => r.request().method() === 'PATCH' && new URL(r.url()).pathname.startsWith('/v1/keys/'));
  await page.getByTestId('save-edit-btn').click();
  expect((await patched).status()).toBe(403);
  await expect(page.getByTestId('edit-key-error')).toContainText("can't hold scope 'generate'");
  await expect(page.getByTestId(`edit-row-${keyId}`), 'the form stays open to change').toBeVisible();
  await expect(page.getByTestId(`key-scopes-${keyId}`)).toContainText('read');
  await expect(page.getByTestId(`key-scopes-${keyId}`)).not.toContainText('generate');
});
