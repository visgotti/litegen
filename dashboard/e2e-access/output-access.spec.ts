import { test, expect, type APIRequestContext, type Locator, type Page } from '@playwright/test';
import { feature, generateAndCapture } from '../e2e/support';

// Who may open stored outputs (docs/storage-access.md), driven from the
// dashboard against a real hosted backend (playwright.access.config.ts).

const BACKEND = 'http://127.0.0.1:5399';
const MASTER_KEY = 'access-e2e-master';
const PW = 'Password123!';
const RUN = Date.now().toString(36);

type Asset = { kind: string; format: string; url: string; expires_at?: string };

/** Submit a mock 3D model with `key` and poll it to `completed`. */
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

/**
 * The shared viewer (@litegen/3d-viewer) reports `data-state="loaded"` only
 * once three.js's loader has parsed the file and drawn it, and the drawing
 * must cover part of the frame (as e2e/model3d.spec.ts checks).
 */
async function expectMeshLoaded(viewer: Locator) {
  await expect(viewer).toBeAttached({ timeout: 60_000 });
  await viewer.scrollIntoViewIfNeeded();
  await expect(viewer, 'three.js never drew the mesh').toHaveAttribute('data-state', 'loaded', { timeout: 60_000 });
  expect(Number(await viewer.getAttribute('data-coverage')), 'the mesh covers part of the frame').toBeGreaterThanOrEqual(0.01);
}

async function signUpOwner(page: Page) {
  await page.goto('/signup');
  await page.getByTestId('signup-org-name').fill('Files Co');
  await page.getByTestId('signup-email').fill(`owner-${RUN}@files.example`);
  await page.getByTestId('signup-password').fill(PW);
  await page.getByTestId('signup-confirm-password').fill(PW);
  await page.getByTestId('signup-submit').click();
  await expect(page.getByTestId('org-switcher')).toBeVisible({ timeout: 15_000 });
}

test('an app set to signed, then private links; the gateway default; private previews', async ({ page, request, browser }) => {
  await feature('Sign up an owner, turn on the mock models, mint a key');
  await signUpOwner(page);
  await page.goto('/providers');
  await page.getByTestId('provider-toggle-mock').click();
  await page.getByTestId('provider-key-field-mock-key-0').fill('sk-mock-files');
  await page.getByTestId('provider-key-save-mock').click();
  await expect(page.getByTestId('provider-section-mock')).toContainText('configured', { timeout: 15_000 });
  for (const model of ['mock/mesh-3d', 'mock/image-gen']) {
    const enable = page.getByTestId(`provider-model-enable-${model}`);
    await enable.check();
    await expect(enable).toBeChecked();
  }
  await page.goto('/keys');
  await page.getByTestId('new-key-name').fill('files-key');
  await page.getByTestId('new-key-scopes').fill('generate,read');
  await page.getByTestId('create-key-btn').click();
  const secret = (await page.getByTestId('key-secret').textContent())!.trim();
  expect(secret.startsWith('sk_live_')).toBe(true);

  await feature('The app\'s files: signed links for 10 minutes, set on the Organization page');
  await page.goto('/organization');
  await expect(page.getByTestId('access-effective')).toContainText('public links');
  await page.getByTestId('access-mode').selectOption('signed');
  await page.getByTestId('access-ttl').fill('600');
  await page.getByTestId('access-save').click();
  await expect(page.getByTestId('access-effective')).toContainText('signed links for 10 min');
  await expect(page.getByTestId('access-bucket-note')).toBeVisible();

  const signed = await finished3d(request, secret);
  const link = signed.assets[0].url;
  expect(link).toContain('signature=');
  const left = (Date.parse(signed.assets[0].expires_at!) - Date.now()) / 1000;
  expect(left).toBeGreaterThan(580);
  expect(left).toBeLessThanOrEqual(601);
  expect((await request.get(link)).status(), 'a signed link needs no credentials').toBe(200);
  expect((await request.get(link.split('?')[0])).status(), 'the stored URL alone opens nothing').toBe(404);

  await feature('Private: the file route only, which wants a key');
  await page.getByTestId('access-mode').selectOption('private');
  await page.getByTestId('access-save').click();
  await expect(page.getByTestId('access-effective')).toContainText('private: the file route only');
  const priv = await finished3d(request, secret);
  const route = priv.assets[0].url;
  expect(route).toBe(`${BACKEND}/v1/generations/${priv.id}/assets/0`);
  expect((await request.get(route)).status()).toBe(401);
  expect((await request.get(route, { headers: { authorization: `Bearer ${secret}` } })).status()).toBe(200);
  expect((await request.get(link)).status(), 'the earlier signed link closes with the app').toBe(404);

  await feature('The Playground previews a private app\'s mesh with the dashboard\'s own session');
  await page.goto('/playground');
  const select = page.getByTestId('playground-model');
  await expect(select).toBeVisible({ timeout: 30_000 });
  await select.selectOption('mock/mesh-3d');
  await page.getByTestId('playground-prompt').fill('a low-poly chair');
  const run = await generateAndCapture(page, /^\/v1\/models3d\/generations$/, /^\/v1\/models3d\/[^/]+$/);
  const mesh = (run.done!.assets as Asset[]).find(a => a.kind === 'mesh' && a.format === 'glb')!;
  expect(mesh.url).toMatch(/\/v1\/generations\/[^/]+\/assets\/\d+$/);
  const viewer = page.getByTestId('playground-mesh-viewer');
  await expectMeshLoaded(viewer);
  // The viewer fetched the private route itself, through the dashboard's
  // credentialed fetch (fetchStoredFile): the route refuses a request without
  // credentials, and the mesh above was drawn from it.
  await expect(viewer).toHaveAttribute('data-src', mesh.url);
  expect((await request.get(mesh.url)).status(), 'the route without credentials').toBe(401);

  await feature('The gateway default, set by the gateway admin on the Gateway page');
  const admin = await browser.newContext({ baseURL: 'http://127.0.0.1:5374/' });
  const adminPage = await admin.newPage();
  await adminPage.addInitScript(key => localStorage.setItem('litegen_api_key', key), MASTER_KEY);
  await adminPage.goto('/gateway');
  await adminPage.getByTestId('gateway-tab-settings').click();
  await expect(adminPage.getByTestId('gateway-effective-access')).toContainText('public links');
  await adminPage.getByTestId('gateway-access-mode').selectOption('signed');
  await adminPage.getByTestId('gateway-access-ttl').fill('59');
  await adminPage.getByTestId('gateway-settings-save').click();
  await expect(adminPage.getByTestId('gateway-settings-error')).toContainText('between 60 and 604800');
  await adminPage.getByTestId('gateway-access-ttl').fill('900');
  await adminPage.getByTestId('gateway-settings-save').click();
  await expect(adminPage.getByTestId('gateway-effective-access')).toContainText('signed links for 15 min');
  const gateway = await (await request.get(`${BACKEND}/v1/admin/settings`, { headers: { authorization: `Bearer ${MASTER_KEY}` } })).json();
  expect(gateway.effective_storage_access).toEqual({ mode: 'signed', owner_scope: 'key', signed_url_ttl_seconds: 900, keep_outputs: true, retention_days: 0, signed_links: 'store' });
  await admin.close();

  // The app's own setting still wins; cleared, the app follows the gateway.
  await page.goto('/organization');
  await expect(page.getByTestId('access-effective')).toContainText('private: the file route only');
  await page.getByTestId('access-mode').selectOption('');
  await page.getByTestId('access-ttl').fill('');
  await page.getByTestId('access-save').click();
  await expect(page.getByTestId('access-effective')).toContainText('signed links for 15 min');
  expect((await finished3d(request, secret)).assets[0].url).toContain('signature=');
});
