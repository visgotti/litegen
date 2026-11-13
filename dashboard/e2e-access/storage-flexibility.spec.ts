import { test, expect, type APIRequestContext, type Page } from '@playwright/test';
import { feature, generateAndCapture } from '../e2e/support';

// Keeping nothing, revoking links, deleting files and the IAM-role storage
// form (docs/storage-access.md), driven from the dashboard against a real
// hosted backend (playwright.access.config.ts).

const BACKEND = 'http://127.0.0.1:5399';
const MASTER_KEY = 'access-e2e-master';
const PW = 'Password123!';
const RUN = Date.now().toString(36);

type Asset = { kind: string; format: string; url: string; expires_at?: string };

async function finished3d(request: APIRequestContext, key: string): Promise<{ id: string; assets: Asset[] }> {
  const headers = { authorization: `Bearer ${key}` };
  const submitted = await request.post(`${BACKEND}/v1/models3d/generations`, { headers, data: { model: 'mock/mesh-3d', prompt: 'a chair' } });
  expect(submitted.status(), await submitted.text()).toBe(200);
  const { id } = await submitted.json();
  for (let i = 0; i < 40; i++) {
    const v = await (await request.get(`${BACKEND}/v1/models3d/${id}`, { headers })).json();
    if (v.status === 'completed') return v;
    expect(v.status).not.toBe('failed');
    await new Promise(r => setTimeout(r, 250));
  }
  throw new Error(`3D job ${id} never finished`);
}

async function signUpOwner(page: Page) {
  await page.goto('/signup');
  await page.getByTestId('signup-org-name').fill('Keeps Co');
  await page.getByTestId('signup-email').fill(`keeper-${RUN}@keeps.example`);
  await page.getByTestId('signup-password').fill(PW);
  await page.getByTestId('signup-confirm-password').fill(PW);
  await page.getByTestId('signup-submit').click();
  await expect(page.getByTestId('org-switcher')).toBeVisible({ timeout: 15_000 });
}

async function saveAccess(page: Page) {
  await page.getByTestId('access-save').click();
  await expect(page.getByTestId('access-error')).toHaveCount(0);
}

test('keep nothing, revoke links, delete files, and an IAM role for the bucket', async ({ page, request, browser }) => {
  await feature('Sign up an owner, turn on the mock models, mint a key');
  await signUpOwner(page);
  await page.goto('/providers');
  await page.getByTestId('provider-toggle-mock').click();
  await page.getByTestId('provider-key-field-mock-key-0').fill('sk-mock-keeps');
  await page.getByTestId('provider-key-save-mock').click();
  await expect(page.getByTestId('provider-section-mock')).toContainText('configured', { timeout: 15_000 });
  for (const model of ['mock/mesh-3d', 'mock/visual-image-gen', 'mock/visual-video-gen']) {
    const enable = page.getByTestId(`provider-model-enable-${model}`);
    await enable.check();
    await expect(enable).toBeChecked();
  }
  await page.goto('/keys');
  await page.getByTestId('new-key-name').fill('keeps-key');
  await page.getByTestId('new-key-scopes').fill('generate,read');
  await page.getByTestId('create-key-btn').click();
  const secret = (await page.getByTestId('key-secret').textContent())!.trim();

  await feature('The app keeps nothing: the Playground image comes back inline, and the log keeps no copy');
  await page.goto('/organization');
  await page.getByTestId('access-keep').selectOption('false');
  await saveAccess(page);
  await expect(page.getByTestId('access-effective')).toContainText('keeps nothing');
  await page.goto('/playground');
  const select = page.getByTestId('playground-model');
  await expect(select).toBeVisible({ timeout: 30_000 });
  await select.selectOption('mock/visual-image-gen');
  await page.getByTestId('playground-prompt').fill('a lighthouse at dusk');
  const run = await generateAndCapture(page, /^\/v1\/images\/generations$/);
  const answer = run.answer as { id: string; data: Array<{ url?: string | null; b64_json?: string }> };
  expect(answer.data[0].b64_json, 'the image itself').toBeTruthy();
  expect(answer.data[0].url ?? null, 'no link to a copy').toBeNull();
  await expect(page.getByTestId('playground-image')).toBeVisible();
  await page.goto('/logs');
  await page.getByTestId(`logs-row-${answer.id}`).click();
  await page.getByTestId('trace-tab-visual').click();
  await expect(page.getByTestId('trace-visual-not-kept')).toBeVisible();
  await page.getByTestId('trace-panel-close').click();

  await feature('Signed links by litegen: they work without credentials until revoked');
  await page.goto('/organization');
  await page.getByTestId('access-keep').selectOption('true');
  await page.getByTestId('access-mode').selectOption('signed');
  await page.getByTestId('access-signed-links').selectOption('litegen');
  await saveAccess(page);
  await expect(page.getByTestId('access-effective')).toContainText('signed links by litegen');
  const mesh = await finished3d(request, secret);
  const link = mesh.assets[0].url;
  expect(link).toContain('/assets/0/signed?');
  expect((await request.get(link)).status(), 'no credentials needed').toBe(200);
  await page.getByTestId('access-revoke').click();
  await expect(page.getByTestId('access-revoked-at')).toBeVisible();
  const revoked = await request.get(link);
  expect(revoked.status()).toBe(403);
  expect((await revoked.json()).error.code).toBe('link_revoked');

  await feature('Private: a GIF video previews as an image through the file route');
  await page.getByTestId('access-mode').selectOption('private');
  await saveAccess(page);
  await page.goto('/playground');
  await expect(select).toBeVisible({ timeout: 30_000 });
  await select.selectOption('mock/visual-video-gen');
  await page.getByTestId('playground-prompt').fill('a bouncing ball');
  await generateAndCapture(page, /^\/v1\/videos\/generations$/, /^\/v1\/videos\/[^/]+$/);
  const clip = page.getByTestId('playground-video');
  await expect(clip).toBeVisible({ timeout: 30_000 });
  await expect.poll(() => clip.evaluate(el => el.tagName)).toBe('IMG');

  await feature('Delete a generation\'s files from its trace');
  await page.goto('/logs');
  await page.getByTestId(`logs-row-${mesh.id}`).click();
  page.once('dialog', d => void d.accept());
  await page.getByTestId('trace-delete-files').click();
  await expect.poll(async () => {
    const g = await (await request.get(`${BACKEND}/v1/generations/${mesh.id}`, { headers: { authorization: `Bearer ${secret}` } })).json();
    return g.files_deleted_reason ?? null;
  }).toBe('deleted');
  await page.getByTestId('trace-panel-close').click();

  await feature('An IAM role for the app\'s bucket: litegen shows the external ID to trust');
  await page.goto('/organization');
  await page.getByTestId('storage-credential-mode').selectOption('assume_role');
  await expect(page.getByTestId('storage-role-arn')).toBeVisible();
  await expect(page.getByTestId('storage-access-key-id')).toHaveCount(0);
  await expect(page.getByTestId('storage-external-id')).toHaveText(/^litegen-[0-9a-f]{32}$/);
  await expect(page.getByTestId('storage-fallback')).toBeChecked();

  await feature('The gateway admin revokes every litegen link');
  const admin = await browser.newContext({ baseURL: 'http://127.0.0.1:5374/' });
  const adminPage = await admin.newPage();
  await adminPage.addInitScript(key => localStorage.setItem('litegen_api_key', key), MASTER_KEY);
  await adminPage.goto('/gateway');
  await adminPage.getByTestId('gateway-tab-settings').click();
  await expect(adminPage.getByTestId('gateway-access-keep')).toBeVisible();
  await expect(adminPage.getByTestId('gateway-access-retention')).toBeVisible();
  await adminPage.getByTestId('gateway-revoke-links').click();
  await expect(adminPage.getByTestId('gateway-revoked-at')).toBeVisible();
  await admin.close();
});
