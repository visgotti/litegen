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

  // (A) Unified panel from diverse catalog schemas:
  //   mock/all-params-image → params {quality, size (enum), seed, …}
  //   bfl/flux-pro-1.1      → params {seed, size (freeform)}
  // (was openai/gpt-image-1, and before it openai/dall-e-3 + bfl/flux-pro. OpenAI
  // shut gpt-image-1 down on 2026-10-23 and no carried OpenAI model has an enum
  // `size` beside `quality`, so the mock stands in for that shape.)
  // flux's provider isn't configured here, but GET /v1/models/{id} still serves
  // the schema, so the unified panel builds from it.
  await page.getByTestId('pg-model-mock/all-params-image').locator('input').check();
  await page.getByTestId('pg-model-bfl/flux-pro-1.1').locator('input').check();
  await expect(page.getByTestId('pg-selected-count')).toContainText('2 selected');

  // size is shared by both → "all"; quality only all-params-image.
  await expect(page.getByTestId('pg-param-size-applies')).toContainText('all');
  await expect(page.getByTestId('pg-param-quality-applies')).toContainText('all-params-image');
  // seed has a dedicated control, not a merged row: flux-pro-1.1 declares `seed`, but
  // it must NOT surface as a merged param row (no `pg-param-seed` input nor
  // `pg-param-seed-applies` row) — instead the dedicated `pg-seed` control renders.
  await expect(page.getByTestId('pg-param-seed')).toHaveCount(0);
  await expect(page.getByTestId('pg-param-seed-applies')).toHaveCount(0);
  await expect(page.getByTestId('pg-seed')).toBeVisible();

  // Per-model Request JSON proves buildRequest drops params a model doesn't declare.
  await page.getByTestId('pg-prompt').fill('a red fox');
  const requestsFor = async () => {
    await page.getByTestId('pg-view-requests').click();
    const reqs = JSON.parse(await page.getByTestId('pg-requests-json').innerText());
    return {
      gptImage: reqs.find((r: { modelId: string }) => r.modelId === 'mock/all-params-image').request,
      flux: reqs.find((r: { modelId: string }) => r.modelId === 'bfl/flux-pro-1.1').request,
    };
  };
  // Untouched params are never sent — each model applies its own default
  // rather than one the panel invented.
  let { gptImage, flux } = await requestsFor();
  expect(gptImage.prompt).toBe('a red fox');
  expect('quality' in gptImage).toBe(false);
  expect('size' in gptImage).toBe(false);
  expect('size' in flux).toBe(false);

  // Once set, each param goes only to the models that declare it.
  await page.getByTestId('pg-param-quality').selectOption('hd');
  await page.getByTestId('pg-param-size').selectOption('1024x1024');
  ({ gptImage, flux } = await requestsFor());
  expect(gptImage.quality).toBe('hd'); // all-params-image declares quality
  expect('quality' in flux).toBe(false);  // flux does NOT → dropped
  expect(gptImage.size).toBe('1024x1024');
  expect(flux.size).toBe('1024x1024');

  // "reset" forgets the value: back to the model's default, off the wire.
  await page.getByTestId('pg-param-quality-reset').click();
  ({ gptImage } = await requestsFor());
  expect('quality' in gptImage).toBe(false);

  // (B) Generation with MOCK models (runnable here) → one tile per model with an image.
  await page.getByTestId('pg-model-mock/all-params-image').locator('input').uncheck();
  await page.getByTestId('pg-model-bfl/flux-pro-1.1').locator('input').uncheck();
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
