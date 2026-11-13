import { test, expect, type Browser, type Page } from '@playwright/test';

// What a hosted tenant sees by role (playwright.multitenant.config.ts):
// - /health is the operator's (registry P1: 403 for a tenant), so the Health
//   page keeps the probes and names provider health as the operator's;
// - a Member keeps keys and generations but not an app's settings, limits,
//   model access, storage and access, or the org's vendor keys (security,
//   README roles table), so those forms are not offered;
// - a Viewer reads only: no generation or quote UI;
// - what a session is offered is the permission list `/v1/auth/me` serves for
//   its active org (FU-ME), so a hosted Owner is not offered the deployment's
//   own settings, which no org's list holds in hosted mode;
// - an account that belongs to no org (removed, or its org deleted; F4) gets a
//   screen that lets it make an org or join one by an invitation, and a person
//   removed from the org its dashboard acts in carries on in another.
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

/** What `/v1/auth/me` serves the session signed in on `page`. */
async function servedMe(page: Page): Promise<{ orgs: { id: string; name: string; role: string; permissions: string[] }[] }> {
  const res = await page.request.get('http://127.0.0.1:5274/v1/auth/me');
  expect(res.status(), await res.text()).toBe(200);
  return res.json();
}

/** The Owner on `owner` invites `email` (an account that exists) to the org; the invitation's token. */
async function inviteAgain(owner: Page, email: string): Promise<string> {
  await owner.goto('/members');
  await owner.getByTestId('members-invite-btn').click();
  await owner.getByTestId('invite-email').fill(email);
  await owner.getByTestId('invite-role').selectOption('member');
  await owner.getByTestId('invite-send').click();
  const token = (await owner.getByTestId('invite-dev-token').textContent({ timeout: 15_000 }))!.trim();
  await owner.getByTestId('invite-dev-token-close').click();
  return token;
}

test("a hosted Owner is not offered the deployment's own settings: its org's served permissions hold none", async ({ page }) => {
  await signUpOwner(page, 'Gateway Co');
  // The server's own list for the org (F2): the Owner's, less the deployment's own permissions.
  const [org] = (await servedMe(page)).orgs;
  expect(org.role).toBe('owner');
  expect(org.permissions).toContain('org:write');
  expect(org.permissions).not.toContain('system:config');

  const asked: string[] = [];
  page.on('request', r => { if (new URL(r.url()).pathname.startsWith('/v1/admin/')) asked.push(new URL(r.url()).pathname); });
  await page.getByRole('link', { name: 'Gateway', exact: true }).click();
  await expect(page.getByTestId('forbidden-403')).toContainText('system:config');
  expect(asked, 'the page asks nothing of the gateway admin API').toEqual([]);
});

test('an account in no organization can make one, or join one by an invitation', async ({ page, browser }) => {
  await signUpOwner(page, 'Leavers Co');
  const maker = await invited(page, browser, 'member');
  const joiner = await invited(page, browser, 'member');
  try {
    // The Owner removes both; a hosted session outlives its membership (F4).
    await page.goto('/members');
    for (const { email } of [maker, joiner]) {
      await page.getByTestId(`member-remove-${email}`).click();
      await expect(page.getByTestId(`member-row-${email}`)).toHaveCount(0, { timeout: 15_000 });
    }

    for (const { page: p } of [maker, joiner]) {
      expect((await servedMe(p)).orgs, 'the server lists no org for it').toEqual([]);
      const refused: string[] = [];
      p.on('response', r => { if (r.status() === 403) refused.push(new URL(r.url()).pathname); });
      await p.goto('http://127.0.0.1:5274/');
      await expect(p.getByTestId('no-org')).toBeVisible({ timeout: 15_000 });
      await expect(p.getByTestId('no-org-create')).toBeVisible();
      await expect(p.locator('.alert-error')).toHaveCount(0);
      // Its account role (Member) grants nothing without an org, so no role is shown.
      await expect(p.getByTestId('user-menu-role')).toHaveCount(0);
      expect(refused, 'the page asks nothing it would be refused').toEqual([]);
    }

    // One makes an org of its own: it owns it and the dashboard opens in it.
    await maker.page.getByTestId('no-org-name').fill('Fresh Co');
    await maker.page.getByTestId('no-org-create').click();
    await expect(maker.page.getByTestId('org-switcher').locator('option:checked')).toHaveText('Fresh Co', { timeout: 15_000 });
    await expect(maker.page.getByTestId('user-menu-role')).toContainText(/owner/i);
    await expect(maker.page.getByTestId('no-org')).toHaveCount(0);
    expect((await servedMe(maker.page)).orgs.map(o => [o.name, o.role])).toEqual([['Fresh Co', 'owner']]);

    // Its own account still works from there: the Account page and its sign-ins.
    await joiner.page.getByRole('link', { name: 'Account', exact: true }).click();
    await expect(joiner.page.getByTestId('account-email')).toContainText(joiner.email, { timeout: 15_000 });
    await expect(joiner.page.getByTestId('account-sessions-table')).toBeVisible();

    // The other is invited again, opens the link and proves its password: it is back in the org.
    const token = await inviteAgain(page, joiner.email);
    await joiner.page.goto(`http://127.0.0.1:5274/invite/${token}`);
    await expect(joiner.page.getByTestId('accept-email')).toContainText(joiner.email, { timeout: 15_000 });
    await joiner.page.getByTestId('accept-password').fill(PW);
    await joiner.page.getByTestId('accept-confirm-password').fill(PW);
    await joiner.page.getByTestId('accept-submit').click();
    await joiner.page.waitForURL('**/');
    await expect(joiner.page.getByTestId('org-switcher').locator('option:checked')).toHaveText('Leavers Co', { timeout: 15_000 });
    await expect(joiner.page.getByTestId('no-org')).toHaveCount(0);
  } finally {
    await maker.page.context().close();
    await joiner.page.context().close();
  }
});

test('a person removed from the org it acts in carries on in its other org', async ({ page, browser }) => {
  await signUpOwner(page, 'Former Co');
  const { page: person, email } = await invited(page, browser, 'member');
  try {
    // It makes an org of its own on the side (the API; the dashboard keeps acting in Former Co).
    const csrf = ((await (await person.request.get('http://127.0.0.1:5274/v1/auth/csrf')).json()) as { csrf_token: string }).csrf_token;
    const made = await person.request.post('http://127.0.0.1:5274/v1/orgs', { headers: { 'x-csrf-token': csrf }, data: { name: 'Own Co' } });
    expect(made.status(), await made.text()).toBe(200);
    await person.reload();
    await expect(person.getByTestId('org-switcher').locator('option:checked')).toHaveText('Former Co', { timeout: 15_000 });

    // The Owner removes it from Former Co, the org its dashboard acts in.
    await page.goto('/members');
    await page.getByTestId(`member-remove-${email}`).click();
    await expect(page.getByTestId(`member-row-${email}`)).toHaveCount(0, { timeout: 15_000 });

    // Its dashboard opens in the org it still belongs to, and nothing is refused.
    const refused: string[] = [];
    person.on('response', r => { if (r.status() === 403) refused.push(new URL(r.url()).pathname); });
    await person.reload();
    await expect(person.getByTestId('org-switcher').locator('option:checked')).toHaveText('Own Co', { timeout: 15_000 });
    await expect(person.getByTestId('user-menu-role')).toContainText(/owner/i);
    await expect(person.locator('.alert-error')).toHaveCount(0);
    expect(refused, 'no request names the org it left').toEqual([]);
  } finally {
    await person.context().close();
  }
});

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
