import { test, expect, type Page } from '@playwright/test';
import { feature } from '../e2e/support';

// A password's rules are the server's (playwright.multitenant.config.ts):
// GET /v1/auth/config serves the minimum length, which the sign-up, invitation
// and account forms show and hold to; served lower than the server holds to,
// a form sends the short password and shows the server's refusal, in its words.

const BACKEND = 'http://127.0.0.1:5199';
const PW = 'super-secret-password-123';
const SHORT = 'short-pw';
const rand = () => Math.random().toString(36).slice(2, 10);

/** The page's GET /v1/auth/config, answered as the server does but with a lower minimum. */
async function servedLower(page: Page) {
  await page.route(/\/v1\/auth\/config$/, async route => {
    const response = await route.fetch();
    const body = (await response.json()) as Record<string, unknown>;
    await route.fulfill({ response, json: { ...body, password_min_length: 4 } });
  });
}

/** The server's refusal of the page's next POST/PATCH to `path`, in its words. */
async function refusalOf(page: Page, method: string, path: RegExp, act: () => Promise<void>): Promise<string> {
  const answered = page.waitForResponse(r => r.request().method() === method && path.test(new URL(r.url()).pathname), { timeout: 15_000 });
  await act();
  const answer = await answered;
  expect(answer.status(), await answer.text()).toBe(400);
  return ((await answer.json()) as { error: { message: string } }).error.message;
}

test("the sign-up, invitation and account forms take the password's length from the server, and show its refusal", async ({ page, browser, request }) => {
  const served = ((await (await request.get(`${BACKEND}/v1/auth/config`)).json()) as { password_min_length: number }).password_min_length;
  expect(served, 'the server serves its minimum').toBeGreaterThan(SHORT.length);

  await feature('Sign-up — a short password sent, and refused in the server\'s words; the served minimum shown');
  await servedLower(page);
  await page.goto('/signup');
  const email = `rules+${rand()}@litegen.test`;
  await page.getByTestId('signup-org-name').fill('Rules Co');
  await page.getByTestId('signup-email').fill(email);
  await page.getByTestId('signup-password').fill(SHORT);
  await page.getByTestId('signup-confirm-password').fill(SHORT);
  let said = await refusalOf(page, 'POST', /^\/v1\/auth\/signup$/, () => page.getByTestId('signup-submit').click());
  await expect(page.getByTestId('signup-error')).toContainText(said);
  await page.unroute(/\/v1\/auth\/config$/);
  await page.reload();
  await expect(page.getByTestId('signup-password')).toHaveAttribute('minlength', String(served));
  await expect(page.getByTestId('signup-password-hint')).toContainText(`at least ${served} characters`);
  await page.getByTestId('signup-org-name').fill('Rules Co');
  await page.getByTestId('signup-email').fill(email);
  await page.getByTestId('signup-password').fill(PW);
  await page.getByTestId('signup-confirm-password').fill(PW);
  await page.getByTestId('signup-submit').click();
  await page.waitForURL('**/');
  await page.goto('/');
  await expect(page.getByTestId('user-menu-email')).toContainText(email, { timeout: 15_000 });

  await feature('Account — a new password held to the server, its refusal shown');
  await servedLower(page);
  await page.goto('/account');
  await expect(page.getByTestId('account-password-hint')).toContainText('at least 4 characters');
  await page.getByTestId('account-current-password').fill(PW);
  await page.getByTestId('account-new-password').fill(SHORT);
  await page.getByTestId('account-confirm-password').fill(SHORT);
  said = await refusalOf(page, 'PATCH', /^\/v1\/account$/, () => page.getByTestId('account-change-password-submit').click());
  await expect(page.getByTestId('account-password-error')).toContainText(said);

  await feature('Invitation — the invitee\'s password held to the server, its refusal shown');
  await page.goto('/members');
  await page.getByTestId('members-invite-btn').click();
  const invitee = `invitee+${rand()}@litegen.test`;
  await page.getByTestId('invite-email').fill(invitee);
  await page.getByTestId('invite-role').selectOption('member');
  await page.getByTestId('invite-send').click();
  const link = await page.getByTestId('invite-link').inputValue({ timeout: 15_000 });
  await page.getByTestId('invite-close').click();
  const ctx = await browser.newContext();
  try {
    const p = await ctx.newPage();
    await servedLower(p);
    await p.goto(link);
    await p.getByTestId('accept-password').fill(SHORT);
    await p.getByTestId('accept-confirm-password').fill(SHORT);
    said = await refusalOf(p, 'POST', /^\/v1\/auth\/invitations\/[^/]+\/accept$/, () => p.getByTestId('accept-submit').click());
    await expect(p.getByTestId('accept-error')).toContainText(said);
    await p.unroute(/\/v1\/auth\/config$/);
    await p.reload();
    await expect(p.getByTestId('accept-password')).toHaveAttribute('minlength', String(served));
    await expect(p.getByTestId('accept-password-hint')).toContainText(`at least ${served} characters`);
  } finally {
    await ctx.close();
  }
});
