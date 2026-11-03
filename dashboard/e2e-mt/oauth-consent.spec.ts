import { test, expect, type APIRequestContext, type Page } from '@playwright/test';
import crypto from 'node:crypto';

// An MCP client signing in to LiteGen with OAuth, as ChatGPT or claude.ai do:
// it registers itself, sends the person's browser to /oauth/authorize, the
// person approves on the dashboard's consent page, and the browser comes back
// to the client with a code it exchanges for tokens.
//
// The backend runs hosted on :5199 with public_base_url http://127.0.0.1:5274/api
// (see playwright.multitenant.config.ts): like production, the issuer is the
// dashboard's origin + /api and the consent page is on the dashboard.

const BACKEND = 'http://127.0.0.1:5199';
const ISSUER = 'http://127.0.0.1:5274/api';
const PW = 'oauth-e2e-password-123';
/** Where the client wants the browser back: its registered loopback URI, on its own port. */
const REDIRECT = 'http://127.0.0.1:53111/callback';
const VERIFIER = crypto.randomBytes(32).toString('base64url');
const CHALLENGE = crypto.createHash('sha256').update(VERIFIER).digest('base64url');

async function registerClient(request: APIRequestContext): Promise<string> {
  const reg = await request.post(`${BACKEND}/oauth/register`, {
    data: { redirect_uris: ['http://127.0.0.1/callback'], client_name: 'E2E MCP client' },
  });
  expect(reg.status()).toBe(201);
  return (await reg.json()).client_id;
}

/** A new person with their own org, created through the API; returns their email. */
async function newPerson(request: APIRequestContext, tag: string): Promise<string> {
  const email = `oauth-${tag}-${Date.now()}@litegen.test`;
  const res = await request.post(`${BACKEND}/v1/auth/signup`, { data: { email, password: PW, org_name: `OAuth ${tag}` } });
  expect(res.status()).toBe(200);
  return email;
}

function authorizeUrl(clientId: string, scope: string): string {
  const params = new URLSearchParams({
    response_type: 'code',
    client_id: clientId,
    redirect_uri: REDIRECT,
    code_challenge: CHALLENGE,
    code_challenge_method: 'S256',
    state: 'e2e-state',
    scope,
  });
  // Through the dashboard's /api proxy, as the hosted service routes it.
  return `/api/oauth/authorize?${params}`;
}

/** Start the sign-in signed out, log in, and land on the consent page. */
async function reachConsent(page: Page, clientId: string, email: string, scope: string) {
  await page.goto(authorizeUrl(clientId, scope));
  await page.waitForURL(/\/login\?next=/);
  await page.getByTestId('login-email').fill(email);
  await page.getByTestId('login-password').fill(PW);
  await page.getByTestId('login-submit').click();
  await page.waitForURL(/\/oauth\/consent\?request=/);
}

test('an MCP client signs in through the consent page', async ({ page, request }) => {
  const clientId = await registerClient(request);
  const email = await newPerson(request, 'approve');
  await reachConsent(page, clientId, email, 'ai:generate ai:read');

  await expect(page.getByTestId('consent-title')).toHaveText('E2E MCP client wants to use LiteGen');
  await expect(page.getByTestId('consent-redirect-host')).toContainText('127.0.0.1');
  await expect(page.getByTestId('consent-scope-ai:generate')).toBeChecked();
  await expect(page.getByTestId('consent-scope-ai:read')).toBeChecked();
  await expect(page.getByText("Generate images, video and 3D models (spends this org's budget)")).toBeVisible();

  // Untick one scope; the grant carries only what stays ticked.
  await page.getByTestId('consent-scope-ai:generate').uncheck();
  const backToClient = page.waitForRequest(r => r.url().startsWith(REDIRECT));
  await page.getByTestId('consent-approve').click();
  const back = new URL((await backToClient).url());
  expect(back.searchParams.get('state')).toBe('e2e-state');
  expect(back.searchParams.get('iss')).toBe(ISSUER);
  const code = back.searchParams.get('code');
  expect(code).toBeTruthy();

  const tokens = await request.post(`${BACKEND}/oauth/token`, {
    form: { grant_type: 'authorization_code', code: code!, redirect_uri: REDIRECT, client_id: clientId, code_verifier: VERIFIER },
  });
  expect(tokens.status()).toBe(200);
  const body = await tokens.json();
  expect(body.access_token).toMatch(/^lgat_/);
  expect(body.scope).toBe('ai:read');
});

test('denying sends the person back with access_denied, and a spent request says so', async ({ page, request }) => {
  const clientId = await registerClient(request);
  const email = await newPerson(request, 'deny');
  await reachConsent(page, clientId, email, 'admin:read admin:write');
  await expect(page.getByTestId('consent-title')).toHaveText('E2E MCP client wants to use LiteGen admin (management API)');
  const consentUrl = page.url();

  const backToClient = page.waitForRequest(r => r.url().startsWith(REDIRECT));
  await page.getByTestId('consent-deny').click();
  const back = new URL((await backToClient).url());
  expect(back.searchParams.get('error')).toBe('access_denied');
  expect(back.searchParams.get('iss')).toBe(ISSUER);

  // The same request again: decided, so it cannot be approved now.
  await page.goto(consentUrl);
  await expect(page.getByTestId('consent-expired')).toHaveText(
    'This sign-in request has expired. Go back to the app and connect again.',
  );
  await expect(page.getByTestId('consent-approve')).toHaveCount(0);
});
