import { test, expect } from '@playwright/test';
import crypto from 'node:crypto';

// The dashboard's Connect AI tools page on the hosted stack: the deployment's
// MCP endpoints, per-client setup, a key made for MCP, and the apps the person
// has connected through OAuth, which they can revoke.

const BACKEND = 'http://127.0.0.1:5199';
const PUBLIC = 'http://127.0.0.1:5274/api';
const PW = 'connect-e2e-password-123';
const REDIRECT = 'http://127.0.0.1:53111/callback';

test('Connect shows the endpoints and client setup, makes an MCP key, and lists and revokes connected apps', async ({ page, request }) => {
  const email = `connect-${Date.now()}@litegen.test`;
  await page.goto('/signup');
  await page.getByTestId('signup-org-name').fill('Connect org');
  await page.getByTestId('signup-email').fill(email);
  await page.getByTestId('signup-password').fill(PW);
  await page.getByTestId('signup-confirm-password').fill(PW);
  await page.getByTestId('signup-submit').click();
  await page.waitForURL('**/');

  await page.getByTestId('nav-connect').click();
  await expect(page.getByTestId('connect-ai-url')).toHaveText(`${PUBLIC}/mcp`);
  await expect(page.getByTestId('connect-admin-url')).toHaveText(`${PUBLIC}/admin/mcp`);

  // Each client's own setup, with this deployment's URL.
  await page.getByTestId('connect-tab-codex').click();
  await expect(page.getByTestId('connect-snippets')).toContainText('bearer_token_env_var = "LITEGEN_API_KEY"');
  await expect(page.getByTestId('connect-snippets')).toContainText(`url = "${PUBLIC}/mcp"`);
  await page.getByTestId('connect-tab-cursor').click();
  await expect(page.getByTestId('connect-install-cursor')).toHaveAttribute('href', /^cursor:\/\/anysphere\.cursor-deeplink\/mcp\/install\?name=litegen&config=/);
  await page.getByTestId('connect-tab-chatgpt').click();
  await expect(page.getByTestId('connect-snippets')).toContainText('OAuth');

  // A key for the AI endpoint, shown once.
  await page.getByTestId('connect-preset-ai').check();
  await page.getByTestId('connect-create-key').click();
  await expect(page.getByTestId('key-secret')).toHaveText(/^sk_live_/);

  // Connect an app through OAuth in this session, then see and revoke it here.
  const reg = await request.post(`${BACKEND}/oauth/register`, {
    data: { redirect_uris: ['http://127.0.0.1/callback'], client_name: 'E2E connect client' },
  });
  expect(reg.status()).toBe(201);
  const clientId = (await reg.json()).client_id;
  const verifier = crypto.randomBytes(32).toString('base64url');
  const params = new URLSearchParams({
    response_type: 'code',
    client_id: clientId,
    redirect_uri: REDIRECT,
    code_challenge: crypto.createHash('sha256').update(verifier).digest('base64url'),
    code_challenge_method: 'S256',
    scope: 'ai:read',
  });
  const backToClient = page.waitForRequest(r => r.url().startsWith(REDIRECT));
  await page.goto(`/api/oauth/authorize?${params}`);
  await page.getByTestId('consent-approve').click();
  const code = new URL((await backToClient).url()).searchParams.get('code')!;
  const tokens = await request.post(`${BACKEND}/oauth/token`, {
    form: { grant_type: 'authorization_code', code, redirect_uri: REDIRECT, client_id: clientId, code_verifier: verifier },
  });
  expect(tokens.status()).toBe(200);

  await page.goto('/connect');
  const row = page.getByTestId('connected-app-row');
  await expect(row).toHaveCount(1);
  await expect(row).toContainText('E2E connect client');
  await expect(row).toContainText('ai:read');
  await row.getByRole('button', { name: 'Revoke' }).click();
  await expect(page.getByTestId('connected-apps-empty')).toBeVisible();
});
