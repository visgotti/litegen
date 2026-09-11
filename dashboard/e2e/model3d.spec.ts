import { test, expect } from '@playwright/test';
import type { APIRequestContext, Locator, Page, Response } from '@playwright/test';

// End-to-end coverage for 3D model generation against the real gateway binary
// (playwright.config.ts webServer) and the mock 3D providers in models/mock.yaml:
//   mock/mesh-3d        — real glTF 2.0 cube, progress over three polls
//   mock/all-params-3d  — every 3D param kind, labelled
//   mock/fail-3d        — always terminates `failed` with an error string
//
// This is the only gate on the TypeScript `MediaType` copies (the dashboard's
// string comparisons compile fine with a missed spot), and the only proof the
// mock GLB actually parses in three.js: <model-viewer> sets `loaded` only once
// GLTFLoader has parsed the file.
//
// Timeouts are deliberately generous: the machine this runs on is often under
// heavy load, and a mesh load involves a lazy ~1MB bundle, a WebGL context and
// a cross-origin fetch. Every wait is a web-first assertion or expect.poll —
// no fixed sleeps.

const MASTER_KEY = process.env.PLAYWRIGHT_MASTER_KEY ?? 'test-master-key-please-rotate';
// Direct-to-backend base for raw API calls (bypasses the dashboard proxy). Matches
// the webServer port in playwright.config.ts; override with LITEGEN_API_BASE.
const API_BASE = process.env.LITEGEN_API_BASE ?? 'http://127.0.0.1:5099';

const MESH_MODEL = 'mock/mesh-3d';
const ALL_PARAMS_MODEL = 'mock/all-params-3d';
const FAIL_MODEL = 'mock/fail-3d';
// The mock GLB is a 12-triangle cube (litegen-core mock provider).
const MOCK_POLYCOUNT = '12';

const MESH_LOAD_TIMEOUT = 60_000;
const JOB_TIMEOUT = 60_000;

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

const authHeaders = { Authorization: `Bearer ${MASTER_KEY}` };

/** Submit a mock/mesh-3d job straight to the gateway and wait for it to
 *  complete, so the Generations and Logs tests don't depend on another test
 *  having run first. Returns the generation id (which is also the log id). */
async function createCompletedMesh(request: APIRequestContext, prompt: string): Promise<string> {
  const res = await request.post(`${API_BASE}/v1/models3d/generations`, {
    headers: authHeaders,
    data: { model: MESH_MODEL, prompt },
  });
  expect(res.ok(), `submit failed: ${res.status()} ${await res.text()}`).toBeTruthy();
  const { id } = (await res.json()) as { id: string };
  await expect
    .poll(async () => {
      const r = await request.get(`${API_BASE}/v1/models3d/${encodeURIComponent(id)}`, { headers: authHeaders });
      return ((await r.json()) as { status?: string }).status;
    }, { timeout: JOB_TIMEOUT, intervals: [500, 1000, 1500] })
    .toBe('completed');
  return id;
}

async function getJson<T>(request: APIRequestContext, path: string): Promise<T> {
  const res = await request.get(`${API_BASE}${path}`, { headers: authHeaders });
  expect(res.ok(), `GET ${path} → ${res.status()}`).toBeTruthy();
  return (await res.json()) as T;
}

type ModelRow = { id: string; media_type: string; pricing?: { base_cost_usd: number } | null };

async function listModels(request: APIRequestContext): Promise<ModelRow[]> {
  const body = await getJson<ModelRow[] | { data: ModelRow[] }>(request, '/v1/models');
  return Array.isArray(body) ? body : body.data;
}

type ParamSpecJson = { label?: string | null; description?: string | null };

async function paramSpecs(request: APIRequestContext, modelId: string): Promise<Record<string, ParamSpecJson>> {
  const schema = await getJson<{ params?: Record<string, ParamSpecJson> }>(
    request, `/v1/models/${modelId}`,
  );
  return schema.params ?? {};
}

/**
 * The `<model-viewer>` element must be a real model-viewer and reach
 * `loaded === true` — model-viewer only sets it after GLTFLoader has parsed the
 * file, so this is the proof the mesh is valid rather than well-formed bytes.
 * Scrolled into view first: model-viewer's default `loading="auto"` defers the
 * fetch until the element is near the viewport.
 */
async function expectMeshLoaded(viewer: Locator) {
  await expect(viewer).toBeAttached({ timeout: MESH_LOAD_TIMEOUT });
  expect(await viewer.evaluate(el => el.tagName.toLowerCase())).toBe('model-viewer');
  await viewer.scrollIntoViewIfNeeded();
  await expect
    .poll(async () => viewer.evaluate((el: HTMLElement & { loaded?: boolean }) => el.loaded === true), {
      timeout: MESH_LOAD_TIMEOUT,
      message: 'model-viewer never reached loaded === true (GLTFLoader did not parse the mesh)',
    })
    .toBe(true);
}

/** First-attempt src: absolute (a relative mesh URL is a contract violation)
 *  and a .glb. A retry would append `#retry-N`, so this also proves no retry
 *  was needed. Read as the element PROPERTY: React 19 assigns props that exist
 *  on an upgraded custom element as properties, and model-viewer does not
 *  reflect `src` back to an attribute, so getAttribute('src') is null. */
async function expectFirstAttemptGlbSrc(viewer: Locator) {
  const src = await viewer.evaluate((el: HTMLElement & { src?: string }) => el.src ?? el.getAttribute('src'));
  expect(src, 'mesh src must be absolute — a relative URL is a contract violation').toMatch(/^https?:\/\//);
  expect(src, 'first-attempt src must be the raw .glb URL (no #retry-N fragment)').toMatch(/\.glb$/);
}

async function openCompareMode(page: Page) {
  await page.goto('/playground');
  await page.getByTestId('pg-mode-compare').click();
  await expect(page.getByTestId('pg-model-picker')).toBeVisible();
  await page.getByTestId('pg-model-filter').fill('mock');
}

function pickModel(page: Page, modelId: string): Locator {
  return page.getByTestId(`pg-model-${modelId}`).locator('input');
}

test.describe('3D model generation', () => {
  test.beforeEach(async ({ page }) => {
    await authenticate(page);
  });

  test('Single Mode: mock/mesh-3d goes in-flight, then lands on the ModelPreview inspector', async ({ page }) => {
    await page.goto('/playground');
    const modelSelect = page.getByTestId('playground-model');
    // Let the default model's schema settle before switching: SingleMode's
    // schema effect has no cancellation, so switching while the default
    // model's /v1/models/{id} is still in flight could let that stale
    // response re-apply its size to the 3D request.
    await expect(modelSelect).toHaveValue('mock/visual-image-gen', { timeout: 30_000 });
    await expect(page.getByTestId('playground-size')).toHaveValue('512x512', { timeout: 30_000 });

    // The 3D mock must be offerable in the Single Mode list — this guards the
    // model list exposing media_type "model3d" through the whole chain.
    await modelSelect.selectOption(MESH_MODEL);
    // mesh-3d declares no size, so the size control resets to empty.
    await expect(page.getByTestId('playground-size')).toHaveValue('', { timeout: 30_000 });

    await page.getByTestId('playground-prompt').fill('a low-poly fox');
    const generate = page.getByTestId('playground-generate');
    await generate.click();

    // A real in-flight state: the percentage only appears once the job was
    // accepted and a poll reported progress (the mock advances over three polls).
    await expect(generate).toHaveText(/Generating… \d+%/, { timeout: JOB_TIMEOUT });
    await expect(page.locator('.playground-image-area')).toContainText(/generating… \d+%/);

    const inspector = page.getByTestId('playground-mesh');
    await expect(inspector).toBeVisible({ timeout: JOB_TIMEOUT });
    const viewer = page.getByTestId('playground-mesh-viewer');
    await expectMeshLoaded(viewer);
    await expectFirstAttemptGlbSrc(viewer);
    await expect(inspector).toHaveAttribute('data-state', 'loaded', { timeout: MESH_LOAD_TIMEOUT });

    await expect(page.getByTestId('playground-mesh-stat-polycount').locator('dd')).toHaveText(MOCK_POLYCOUNT);
    await expect(page.getByTestId('playground-mesh-stat-format').locator('dd')).toHaveText('GLB');
    // Once loaded, dimensions come from getDimensions() — no longer "measuring…".
    await expect(page.getByTestId('playground-mesh-stat-dimensions')).not.toContainText('measuring');
    await expect(page.getByTestId('playground-mesh-error')).toHaveCount(0);
    await expect(page.getByTestId('playground-mesh-viewer-fallback')).toHaveCount(0);
    await expect(generate).toHaveText('Generate');
  });

  test('Compare Mode: 3D tile polls to a rendered mesh, clamps to one tile at N>1, estimates its real price', async ({ page, request }) => {
    const models = await listModels(request);
    const mesh = models.find(m => m.id === MESH_MODEL);
    const expensive = models.find(m => m.id === 'mock/expensive-image');
    expect(mesh?.media_type).toBe('model3d');
    expect(mesh?.pricing, 'mock/mesh-3d must carry pricing').toBeTruthy();
    expect(expensive?.pricing, 'mock/expensive-image must carry pricing').toBeTruthy();
    const meshPrice = mesh!.pricing!.base_cost_usd;
    const expensivePrice = expensive!.pricing!.base_cost_usd;

    // Every cost-estimate response, with the model it was for. The 3D estimate
    // must go to the 3D endpoint and succeed: the cost preview swallows a
    // failed estimate as 0, so a mis-routed or failing 3D estimate would
    // otherwise pass as a silent "$0.000".
    const estimates: Array<{ path: string; model: string; status: number; body: Record<string, unknown> }> = [];
    page.on('response', async (res: Response) => {
      const url = new URL(res.url());
      if (res.request().method() !== 'POST' || !url.pathname.endsWith('/cost')) return;
      let model = '';
      try { model = (JSON.parse(res.request().postData() ?? '{}') as { model?: string }).model ?? ''; } catch { /* keep '' */ }
      let body: Record<string, unknown> = {};
      try { body = (await res.json()) as Record<string, unknown>; } catch { /* non-JSON error body */ }
      estimates.push({ path: url.pathname, model, status: res.status(), body });
    });

    await openCompareMode(page);

    // ── Cost preview: the 3D model is estimated at its real price ──────────
    // Paired with a non-zero image model so the summed display is meaningful.
    await pickModel(page, MESH_MODEL).check();
    await pickModel(page, 'mock/expensive-image').check();
    await expect(page.getByTestId('pg-selected-count')).toContainText('2 selected');
    await page.getByTestId('pg-prompt').fill('a low-poly fox');

    const meshEstimate = () => estimates.find(e => e.path === '/v1/models3d/cost' && e.model === MESH_MODEL);
    await expect.poll(() => meshEstimate()?.status, { timeout: 30_000 }).toBe(200);
    expect(Number(meshEstimate()!.body.base_cost_usd), '3D estimate must be the model\'s catalog price')
      .toBe(meshPrice);
    // Never routed through the image estimator.
    expect(estimates.filter(e => e.path === '/v1/images/cost' && e.model === MESH_MODEL)).toHaveLength(0);
    const meshTotal = Number(meshEstimate()!.body.total_cost_usd);
    expect(Number.isFinite(meshTotal)).toBe(true);
    await expect(page.getByTestId('pg-cost'))
      .toHaveText(`Est. cost: $${(meshTotal + expensivePrice).toFixed(3)}`, { timeout: 30_000 });

    // ── Generation: 3D alongside an image model, N = 2 ─────────────────────
    await pickModel(page, 'mock/expensive-image').uncheck();
    await pickModel(page, 'mock/visual-image-gen').check();
    await expect(page.getByTestId('pg-selected-count')).toContainText('2 selected');
    await page.getByTestId('pg-n').fill('2');
    await page.getByTestId('pg-view-results').click();
    await page.getByTestId('pg-generate').click();

    // The 3D tile passes through a real polling state (job accepted, progress
    // reported) before it renders.
    const progress = page.getByTestId(`pg-tile-progress-${MESH_MODEL}`);
    await expect(progress).toBeVisible({ timeout: JOB_TIMEOUT });
    await expect(progress).toContainText(/generating… \d+%/);

    // N=2 fans the image model out to two tiles but the 3D model to exactly
    // one: n is meaningless for a mesh job.
    await expect(page.getByTestId('pg-tile-mock/visual-image-gen')).toHaveCount(2);
    await expect(page.getByTestId(`pg-tile-${MESH_MODEL}`)).toHaveCount(1);

    const viewer = page.getByTestId(`pg-tile-mesh-${MESH_MODEL}`);
    await expectMeshLoaded(viewer);
    await expectFirstAttemptGlbSrc(viewer);
    await expect(progress).toHaveCount(0);
    await expect(page.getByTestId(`pg-tile-error-${MESH_MODEL}`)).toHaveCount(0);
    await expect(page.getByTestId(`pg-tile-${MESH_MODEL}`)).toHaveCount(1);
    await expect(page.getByTestId('pg-tile-img-mock/visual-image-gen').first()).toBeVisible({ timeout: JOB_TIMEOUT });
  });

  test('a failing 3D model surfaces its error on the tile, not a blank tile', async ({ page }) => {
    await openCompareMode(page);
    await pickModel(page, FAIL_MODEL).check();
    await page.getByTestId('pg-prompt').fill('anything');
    await page.getByTestId('pg-view-results').click();

    // The terminal poll response carries the provider's error string; the tile
    // must show exactly that, not a generic placeholder or nothing.
    const failedPoll = page.waitForResponse(async res => {
      if (res.request().method() !== 'GET' || !/\/v1\/models3d\/litegen-3d-/.test(res.url())) return false;
      try { return ((await res.json()) as { status?: string }).status === 'failed'; } catch { return false; }
    }, { timeout: JOB_TIMEOUT });
    await page.getByTestId('pg-generate').click();
    const failedBody = (await (await failedPoll).json()) as { error?: string };
    expect(failedBody.error, 'the failed job must carry an error string').toBeTruthy();

    const err = page.getByTestId(`pg-tile-error-${FAIL_MODEL}`);
    await expect(err).toBeVisible({ timeout: JOB_TIMEOUT });
    await expect(err).toContainText(failedBody.error!);
    await expect(page.getByTestId(`pg-tile-mesh-${FAIL_MODEL}`)).toHaveCount(0);
    await expect(page.getByTestId(`pg-tile-progress-${FAIL_MODEL}`)).toHaveCount(0);
  });

  test('Generations gallery: flat thumb per row, ModelPreview inspector on expand', async ({ page, request }) => {
    const id = await createCompletedMesh(request, 'a gallery teapot');
    await page.goto('/generations');

    const row = page.getByTestId(`gen-row-${id}`);
    await expect(row).toBeVisible({ timeout: 30_000 });
    await expect(page.getByTestId(`gen-status-${id}`)).toHaveText('completed');

    // The row thumbnail is a flat image (the preview asset) — never a GL
    // canvas per row. No model-viewer anywhere until a row is expanded.
    const thumb = page.getByTestId(`gen-thumb-${id}`);
    await expect(thumb).toBeVisible();
    expect(await thumb.evaluate(el => el.tagName.toLowerCase())).toBe('img');
    await expect(row.locator('model-viewer')).toHaveCount(0);
    await expect(page.locator('model-viewer')).toHaveCount(0);

    await page.getByTestId(`gen-expand-${id}`).click();
    const inspector = page.getByTestId(`gen-media-${id}`);
    await expect(inspector).toBeVisible({ timeout: 30_000 });
    await expectMeshLoaded(page.getByTestId(`gen-media-${id}-viewer`));
    await expectFirstAttemptGlbSrc(page.getByTestId(`gen-media-${id}-viewer`));
    await expect(inspector).toHaveAttribute('data-state', 'loaded', { timeout: MESH_LOAD_TIMEOUT });

    await expect(page.getByTestId(`gen-media-${id}-stat-polycount`).locator('dd')).toHaveText(MOCK_POLYCOUNT);

    // Asset list: the mesh and the preview, nothing else.
    const assetRows = page.getByTestId(`gen-media-${id}-assets`).locator('tbody tr');
    await expect(assetRows).toHaveCount(2);
    const meshRow = assetRows.filter({ has: page.locator('.mp-kind--mesh') });
    const previewRow = assetRows.filter({ has: page.locator('.mp-kind--preview') });
    await expect(meshRow).toHaveCount(1);
    await expect(previewRow).toHaveCount(1);
    await expect(meshRow).toContainText('GLB');
    await expect(meshRow).toContainText(`${MOCK_POLYCOUNT} polys`);
    await expect(previewRow).toContainText('PNG');
    await expect(meshRow.locator('a[data-testid$="-open"]')).toHaveAttribute('href', /^https?:\/\/.+\.glb$/);
    await expect(previewRow.locator('a[data-testid$="-open"]')).toHaveAttribute('href', /^https?:\/\/.+\.png$/);
  });

  test('Models page: the 3D filter shows mock/mesh-3d and excludes image models', async ({ page }) => {
    await page.goto('/models');
    await expect(page.getByTestId('model-row-mock/visual-image-gen')).toBeVisible({ timeout: 30_000 });

    await page.getByTestId('models-filter-media-type').getByRole('radio', { name: '3D' }).check();

    await expect(page.getByTestId(`model-row-${MESH_MODEL}`)).toBeVisible();
    await expect(page.getByTestId(`model-row-${ALL_PARAMS_MODEL}`)).toBeVisible();
    await expect(page.getByTestId(`model-row-${FAIL_MODEL}`)).toBeVisible();
    await expect(page.getByTestId('model-row-mock/visual-image-gen')).toHaveCount(0);
    await expect(page.getByTestId('model-row-mock/image-gen')).toHaveCount(0);

    // Every remaining row is a model3d row.
    const rows = page.locator('[data-testid^="model-row-"]');
    const n = await rows.count();
    expect(n).toBeGreaterThan(0);
    for (let i = 0; i < n; i++) {
      await expect(rows.nth(i).locator('.badge').first()).toHaveText('model3d');
    }
  });

  test('Logs trace panel renders a completed 3D request as a loaded mesh', async ({ page, request }) => {
    const id = await createCompletedMesh(request, 'a traced lantern');
    await page.goto(`/logs?model=${encodeURIComponent(MESH_MODEL)}`);

    // The log id is the generation id.
    const row = page.getByTestId(`logs-row-${id}`);
    await expect(row).toBeVisible({ timeout: 30_000 });
    await row.click();

    const panel = page.getByTestId('trace-panel');
    await expect(panel).toBeVisible();
    await page.getByTestId('trace-tab-visual').click();

    // The artifact is written at submit time and never updated; the Visual tab
    // must resolve the result through the generation row.
    const completed = panel.getByTestId('generation-output-completed');
    await expect(completed).toBeVisible({ timeout: JOB_TIMEOUT });
    const viewer = completed.getByTestId('generation-output-3d-viewer');
    await expectMeshLoaded(viewer);
    await expectFirstAttemptGlbSrc(viewer);
    await expect(completed.getByTestId('generation-output-3d')).toHaveAttribute('data-state', 'loaded', {
      timeout: MESH_LOAD_TIMEOUT,
    });
    await expect(panel).not.toContainText('Output URL not yet available');
    await expect(panel.getByTestId('generation-output-3d-stat-polycount').locator('dd')).toHaveText(MOCK_POLYCOUNT);
  });

  test('ParamField rows show the schema label and description, not the raw key', async ({ page, request }) => {
    // Assertions are driven by the served schema, so they track the catalog
    // rather than a copy of it.
    const allParams = await paramSpecs(request, ALL_PARAMS_MODEL);
    const meshParams = await paramSpecs(request, MESH_MODEL);
    expect(allParams.target_polycount?.label, 'fixture: all-params-3d labels target_polycount').toBeTruthy();
    expect(meshParams.target_polycount?.description, 'fixture: mesh-3d describes target_polycount').toBeTruthy();

    await openCompareMode(page);

    // mock/all-params-3d alone: every labelled param shows its label.
    await pickModel(page, ALL_PARAMS_MODEL).check();
    const row = page.locator('.pg-param-row').filter({ has: page.getByTestId('pg-param-target_polycount') });
    await expect(row).toHaveCount(1, { timeout: 30_000 });
    const label = row.locator('.pg-param-label');
    await expect(label).toContainText(allParams.target_polycount!.label!);
    await expect(label).not.toContainText('target_polycount');
    await expect(page.getByTestId('pg-param-target_polycount-applies')).toHaveText('all');
    for (const [name, spec] of Object.entries(allParams)) {
      if (name === 'seed' || !spec.label) continue; // seed has a dedicated control, not a row
      await expect(
        page.locator('.pg-param-row').filter({ has: page.getByTestId(`pg-param-${name}`) }).locator('.pg-param-label'),
      ).toContainText(spec.label);
    }
    const allParamsDesc = allParams.target_polycount!.description;
    if (allParamsDesc) {
      await expect(page.getByTestId('pg-param-target_polycount-description')).toHaveText(allParamsDesc);
    } else {
      await expect(page.getByTestId('pg-param-target_polycount-description')).toHaveCount(0);
    }

    // mock/mesh-3d declares a description for target_polycount: it renders
    // as a visible help line (a tooltip alone never shows on touch) and as
    // the label's title.
    await pickModel(page, ALL_PARAMS_MODEL).uncheck();
    await pickModel(page, MESH_MODEL).check();
    const help = page.getByTestId('pg-param-target_polycount-description');
    await expect(help).toBeVisible({ timeout: 30_000 });
    await expect(help).toHaveText(meshParams.target_polycount!.description!);
    await expect(row.locator('.pg-param-label')).toHaveAttribute('title', meshParams.target_polycount!.description!);
    if (meshParams.target_polycount!.label) {
      await expect(row.locator('.pg-param-label')).toContainText(meshParams.target_polycount!.label);
    }
  });
});
