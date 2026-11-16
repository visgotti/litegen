import { test, expect, type Page } from '@playwright/test';
import { feature } from '../e2e/support';

// Password resets in the dashboard (playwright.multitenant.config.ts). litegen
// sends no email: a person who forgot their password asks an admin, who hands
// over a reset link to this dashboard's `/reset-password/:token` page. The
// password's rules are the server's, shown in its words.

const BACKEND = 'http://127.0.0.1:5199';
// LITEGEN__MASTER_KEY in playwright.multitenant.config.ts.
const AS_MASTER_KEY = { Authorization: 'Bearer mt-master-key' };
const PW = 'super-secret-password-123';
const NEW_PW = 'a-brand-new-password-456';
const rand = () => Math.random().toString(36).slice(2, 10);

async function signUp(page: Page): Promise<string> {
  const email = `reset+${rand()}@litegen.test`;
  await page.goto('/signup');
  await page.getByTestId('signup-org-name').fill('Reset Co');
  await page.getByTestId('signup-email').fill(email);
  await page.getByTestId('signup-password').fill(PW);
  await page.getByTestId('signup-confirm-password').fill(PW);
  await page.getByTestId('signup-submit').click();
  await page.waitForURL('**/');
  await page.goto('/');
  await expect(page.getByTestId('user-menu-email')).toContainText(email, { timeout: 15_000 });
  return email;
}

async function signOut(page: Page) {
  await page.getByTestId('user-menu-toggle').click();
  await page.getByTestId('user-menu-signout').click();
  await page.waitForURL('**/login');
}

/** What the confirm route answered the page's next submit. */
function confirmAnswer(page: Page) {
  return page.waitForResponse(r => r.request().method() === 'POST' && new URL(r.url()).pathname === '/v1/auth/password-reset/confirm');
}

test('"Forgot password?" says how to get a reset link: litegen sends no email', async ({ page }) => {
  await feature('Forgot password — a page that says an admin issues reset links');
  await page.goto('/login');
  await page.getByTestId('login-forgot-link').click();
  await expect(page).toHaveURL(/\/forgot-password$/);
  await expect(page.getByTestId('forgot-password')).toContainText('No email is sent');
  await page.getByTestId('forgot-back').click();
  await expect(page).toHaveURL(/\/login$/);
});

test('a reset link sets a new password once, and the person signs in with it', async ({ page, request }) => {
  await feature('Reset password — a new password from the link, then sign in with it');
  const email = await signUp(page);
  await signOut(page);
  // The link's token, issued as an admin issues one: here by the master key,
  // through POST /v1/users/{id}/password-reset.
  const users = (await (await request.get(`${BACKEND}/v1/users`, { headers: AS_MASTER_KEY })).json()) as { id: string; email: string }[];
  const account = users.find(u => u.email === email);
  expect(account, 'the master key lists the new account').toBeTruthy();
  const issued = await request.post(`${BACKEND}/v1/users/${account!.id}/password-reset`, { headers: AS_MASTER_KEY });
  expect(issued.status(), await issued.text()).toBe(200);
  const { token } = (await issued.json()) as { token: string };
  expect(token, 'the answer carries the token').toBeTruthy();

  await page.goto(`/reset-password/${token}`);
  // The password's rules are the server's: a short one is sent, and refused in its words.
  await page.getByTestId('reset-password').fill('too-short');
  await page.getByTestId('reset-confirm-password').fill('too-short');
  let answered = confirmAnswer(page);
  await page.getByTestId('reset-submit').click();
  const short = await answered;
  expect(short.status()).toBe(400);
  await expect(page.getByTestId('reset-error')).toContainText(((await short.json()) as { error: { message: string } }).error.message);

  await page.getByTestId('reset-password').fill(NEW_PW);
  await page.getByTestId('reset-confirm-password').fill(NEW_PW);
  answered = confirmAnswer(page);
  await page.getByTestId('reset-submit').click();
  expect((await answered).status()).toBe(204);
  await expect(page).toHaveURL(/\/login\?reset=done$/);
  await expect(page.getByTestId('login-notice')).toContainText('Password changed');

  await page.getByTestId('login-email').fill(email);
  await page.getByTestId('login-password').fill(NEW_PW);
  await page.getByTestId('login-submit').click();
  await expect(page.getByTestId('user-menu-email')).toContainText(email, { timeout: 15_000 });

  await feature('Reset password — the link opens once');
  await signOut(page);
  await page.goto(`/reset-password/${token}`);
  await page.getByTestId('reset-password').fill('yet-another-password-789');
  await page.getByTestId('reset-confirm-password').fill('yet-another-password-789');
  answered = confirmAnswer(page);
  await page.getByTestId('reset-submit').click();
  const used = await answered;
  expect(used.status()).toBe(400);
  await expect(page.getByTestId('reset-error')).toContainText(((await used.json()) as { error: { message: string } }).error.message);
});

test("an Owner hands a member a reset link from the Members page, and the member's new password works", async ({ page, browser }) => {
  await feature('Members — a reset link for a member, made by the Owner and used by the member');
  await signUp(page);
  // A member, invited by link.
  const memberEmail = `member+${rand()}@litegen.test`;
  await page.goto('/members');
  await page.getByTestId('members-invite-btn').click();
  await page.getByTestId('invite-email').fill(memberEmail);
  await page.getByTestId('invite-role').selectOption('member');
  await page.getByTestId('invite-send').click();
  const inviteLink = await page.getByTestId('invite-link').inputValue({ timeout: 15_000 });
  await page.getByTestId('invite-close').click();
  const memberCtx = await browser.newContext();
  try {
    const member = await memberCtx.newPage();
    await member.goto(inviteLink);
    await member.getByTestId('accept-password').fill(PW);
    await member.getByTestId('accept-confirm-password').fill(PW);
    await member.getByTestId('accept-submit').click();
    await member.waitForURL('**/');

    // The Owner makes the link: shown once, to this dashboard's reset page.
    await page.goto('/members');
    const issued = page.waitForResponse(r => r.request().method() === 'POST' && /^\/v1\/users\/[^/]+\/password-reset$/.test(new URL(r.url()).pathname));
    await page.getByTestId(`member-reset-password-${memberEmail}`).click();
    const answer = await issued;
    expect(answer.status(), await answer.text()).toBe(200);
    const { token } = (await answer.json()) as { token: string };
    const resetLink = page.getByTestId('reset-link');
    await expect(resetLink).toHaveValue(`http://127.0.0.1:5274/reset-password/${token}`);
    const link = await resetLink.inputValue();
    await page.getByTestId('reset-close').click();
    await expect(resetLink).toHaveCount(0);

    // The member, signed out by the reset, sets the new password and signs in with it.
    await member.goto(link);
    await member.getByTestId('reset-password').fill(NEW_PW);
    await member.getByTestId('reset-confirm-password').fill(NEW_PW);
    await member.getByTestId('reset-submit').click();
    await expect(member).toHaveURL(/\/login\?reset=done$/);
    await member.getByTestId('login-email').fill(memberEmail);
    await member.getByTestId('login-password').fill(NEW_PW);
    await member.getByTestId('login-submit').click();
    await expect(member.getByTestId('user-menu-email')).toContainText(memberEmail, { timeout: 15_000 });
  } finally {
    await memberCtx.close();
  }
});
