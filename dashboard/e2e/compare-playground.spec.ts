import { test, expect } from '@playwright/test';

const MASTER_KEY = process.env.PLAYWRIGHT_MASTER_KEY ?? 'test-master-key-please-rotate';

async function authenticate(page: import('@playwright/test').Page) {
  // Master API-key auth. Seed the key into localStorage before any app code runs;
  // TenantContext short-circuits to authenticated when a key is present, so the
  // app shell renders and the SDK sends `Authorization: Bearer <key>` on every call.
  // (Unauthenticated /v1/auth/me returns 401, so the session path would redirect to
  // /login, and signup is closed after the first bootstrap user — this avoids both.)
  await page.addInitScript((key) => {
    localStorage.setItem('litegen_api_key', key);
  }, MASTER_KEY);
}

test('compare mode: unified params from diverse schemas + multi-model generation', async ({ page }) => {
  await authenticate(page);

  await page.goto('/playground');
  await page.getByTestId('pg-mode-compare').click();
  await expect(page.getByTestId('pg-model-picker')).toBeVisible();

  // (A) Unified panel from diverse REAL catalog schemas:
  //   openai/dall-e-3 → params {quality, size, style}
  //   bfl/flux-pro    → params {seed, size}
  // Their providers aren't configured here, but GET /v1/models/{id} still serves
  // the schema, so the unified panel builds from them.
  await page.getByTestId('pg-model-openai/dall-e-3').locator('input').check();
  await page.getByTestId('pg-model-bfl/flux-pro').locator('input').check();
  await expect(page.getByTestId('pg-selected-count')).toContainText('2 selected');

  // size is shared by both → "all"; style only dall-e-3.
  await expect(page.getByTestId('pg-param-size-applies')).toContainText('all');
  await expect(page.getByTestId('pg-param-style-applies')).toContainText('dall-e-3');
  // seed has a dedicated control, not a merged row.
  await expect(page.getByTestId('pg-param-seed')).toHaveCount(0);

  // Per-model Request JSON proves buildRequest drops params a model doesn't declare.
  await page.getByTestId('pg-prompt').fill('a red fox');
  await page.getByTestId('pg-view-requests').click();
  const reqs = JSON.parse(await page.getByTestId('pg-requests-json').innerText());
  const dalle = reqs.find((r: { modelId: string }) => r.modelId === 'openai/dall-e-3').request;
  const flux = reqs.find((r: { modelId: string }) => r.modelId === 'bfl/flux-pro').request;
  expect(dalle.prompt).toBe('a red fox');
  expect('style' in dalle).toBe(true);    // dall-e-3 declares style
  expect('style' in flux).toBe(false);     // flux does NOT → dropped
  expect('size' in dalle).toBe(true);
  expect('size' in flux).toBe(true);

  // (B) Generation with MOCK models (runnable here) → one tile per model with an image.
  await page.getByTestId('pg-model-openai/dall-e-3').locator('input').uncheck();
  await page.getByTestId('pg-model-bfl/flux-pro').locator('input').uncheck();
  await page.getByTestId('pg-model-filter').fill('mock');
  await page.getByTestId('pg-model-mock/visual-image-gen').locator('input').check();
  await page.getByTestId('pg-model-mock/image-gen').locator('input').check();
  await page.getByTestId('pg-view-results').click();
  await page.getByTestId('pg-generate').click();

  await expect(page.getByTestId('pg-tile-img-mock/visual-image-gen')).toBeVisible({ timeout: 30000 });
  await expect(page.getByTestId('pg-tile-img-mock/image-gen')).toBeVisible({ timeout: 30000 });
});

test('single mode still works (regression)', async ({ page }) => {
  await authenticate(page);
  await page.goto('/playground');
  // Single mode is the default; its original testids must still be present.
  await expect(page.getByTestId('playground-model')).toBeVisible();
  await page.getByTestId('playground-prompt').fill('a blue cat');
  await page.getByTestId('playground-generate').click();
  await expect(page.getByTestId('playground-image')).toBeVisible({ timeout: 30000 });
});
