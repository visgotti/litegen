import { test, expect } from '@playwright/test';
import type { APIRequestContext, Locator, Page } from '@playwright/test';
import crypto from 'crypto';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';
import { checkOf, checked, feature, generateAndCapture, nextCall } from './support';

// End-to-end coverage for the Playground's 3D INPUT side against the real
// gateway binary (playwright.config.ts webServer): reference images and
// multiview views, the output-format provider-job budget, models that take no
// prompt, provider passthrough, lax-mode dropped params, and the model detail
// drawer.
// model3d.spec.ts covers the display side (inspector, gallery, trace panel).
//
// Only mock models are ever GENERATED here. The fal rows are exercised as
// served schemas and form state only — no request reaches fal, so this suite
// can never spend money, with or without a fal key configured.

const MASTER_KEY = process.env.PLAYWRIGHT_MASTER_KEY ?? 'test-master-key-please-rotate';
const MESH_MODEL = 'mock/mesh-3d';
/** Takes text, an input image, or views (multiview). */
const ALL_PARAMS = 'mock/all-params-3d';
const TRELLIS = 'fal/trellis';
const RODIN = 'fal/hyper3d-rodin';
const JOB_TIMEOUT = 60_000;
const MESH_LOAD_TIMEOUT = 60_000;
const MESHES = /^\/v1\/models3d\/generations$/;
const MESH_POLL = /^\/v1\/models3d\/litegen-3d-[^/]+$/;

const __dirname = path.dirname(fileURLToPath(import.meta.url));
/** A real PNG (the Storybook fixture), uploaded as an inline reference image. */
const IMAGE_FIXTURE = path.resolve(__dirname, '../.storybook/fixtures/image.png');
/** A second, different PNG: a view that is not the front. */
const SIDE_FIXTURE = path.resolve(__dirname, '../.storybook/fixtures/texture.png');

async function authenticate(page: Page) {
  // Master API-key auth, seeded before any app code runs (see model3d.spec.ts).
  await page.addInitScript((key) => {
    localStorage.setItem('litegen_api_key', key);
  }, MASTER_KEY);
}

async function openSingleMode(page: Page, modelId: string) {
  await page.goto('/playground');
  const select = page.getByTestId('playground-model');
  // Let the default model settle first so its schema cannot land after ours.
  await expect(select).toHaveValue('mock/visual-image-gen', { timeout: 30_000 });
  await select.selectOption(modelId);
  await expect(select).toHaveValue(modelId);
}

async function openCompareMode(page: Page) {
  await page.goto('/playground');
  await page.getByTestId('pg-mode-compare').click();
  await expect(page.getByTestId('pg-model-picker')).toBeVisible();
}

function pickModel(page: Page, modelId: string): Locator {
  return page.getByTestId(`pg-model-${modelId}`).locator('input');
}

/** The per-model request bodies Compare mode would send, from its Requests tab. */
async function compareRequests(page: Page): Promise<Record<string, Record<string, unknown>>> {
  await page.getByTestId('pg-view-requests').click();
  const reqs = JSON.parse(await page.getByTestId('pg-requests-json').innerText()) as
    Array<{ modelId: string; request: Record<string, unknown> }>;
  await page.getByTestId('pg-view-results').click();
  return Object.fromEntries(reqs.map(r => [r.modelId, r.request]));
}

type FormatsSpec = { enum_values: string[]; derivable_values?: string[]; max_items?: number | null; default?: string[] };

/**
 * A model's `output_formats` exactly as GET /v1/models/{id} serves it. The
 * format tests derive every expectation from this rather than naming formats:
 * which containers litegen converts locally grows over time (`derivable_values`),
 * and a test pinned to today's set would break the day it does.
 */
async function servedFormats(request: APIRequestContext, modelId: string): Promise<FormatsSpec> {
  const res = await request.get(`http://127.0.0.1:5099/v1/models/${modelId}`, {
    headers: { Authorization: `Bearer ${MASTER_KEY}` },
  });
  expect(res.ok(), `GET /v1/models/${modelId} → ${res.status()}`).toBeTruthy();
  const spec = ((await res.json()) as { params?: { output_formats?: FormatsSpec } }).params?.output_formats;
  expect(spec, `${modelId} serves output_formats`).toBeTruthy();
  return spec!;
}

/** The server's rule (validator, `vendor_jobs_needed`): derivable formats share one job. */
function providerJobs(selected: string[], derivable: string[]): number {
  const shared = new Set(derivable);
  return selected.filter(f => !shared.has(f)).length + (selected.some(f => shared.has(f)) ? 1 : 0);
}

/** Each format box is ticked as `chosen` says, and an unticked one is enabled
 *  exactly when ticking it would still fit the provider-job budget. */
async function expectBoxesFollowTheBudget(page: Page, spec: FormatsSpec, chosen: string[]) {
  const derivable = spec.derivable_values ?? [];
  for (const f of spec.enum_values) {
    const box = page.getByTestId(`pg-param-output_formats-${f}`);
    if (chosen.includes(f)) {
      await expect(box, `${f} is ticked`).toBeChecked();
      continue;
    }
    await expect(box, `${f} is unticked`).not.toBeChecked();
    const fits = spec.max_items == null || providerJobs([...chosen, f], derivable) <= spec.max_items;
    if (fits) await expect(box, `${f} fits the budget`).toBeEnabled();
    else await expect(box, `${f} would need another provider job`).toBeDisabled();
  }
}

/** Whether every format on offer shares the one provider job (no selection can exceed the budget). */
const allShare = (spec: FormatsSpec) =>
  spec.enum_values.length > 0 && spec.enum_values.every(f => (spec.derivable_values ?? []).includes(f));

/** What the picker and the drawer say about the budget for this served spec. */
function budgetText(spec: FormatsSpec): { cap: string; drawer: string } {
  if (allShare(spec)) return { cap: 'any combination is one provider job', drawer: 'any combination is one provider job' };
  const shared = spec.derivable_values ?? [];
  const max = spec.max_items ?? 0;
  const jobs = `${max} provider job${max === 1 ? '' : 's'}`;
  return {
    cap: shared.length > 1 ? `${jobs} · ${shared.join('/')} share one` : `up to ${max}`,
    drawer: jobs + (shared.length > 1 ? ` (${shared.join('/')} share one)` : ''),
  };
}

/** `formats` in the order the model declares them — the order the panel submits. */
const inDeclaredOrder = (spec: FormatsSpec, formats: string[]) => spec.enum_values.filter(f => formats.includes(f));

/** The shared viewer reports `loaded` only once three.js's loader has parsed
 *  the file and drawn it, covering part of the frame. */
async function expectMeshLoaded(viewer: Locator) {
  await expect(viewer).toBeAttached({ timeout: MESH_LOAD_TIMEOUT });
  await viewer.scrollIntoViewIfNeeded();
  await expect(viewer, 'three.js never drew the mesh').toHaveAttribute('data-state', 'loaded', { timeout: MESH_LOAD_TIMEOUT });
  expect(Number(await viewer.getAttribute('data-coverage')), 'the mesh covers part of the frame').toBeGreaterThanOrEqual(0.01);
}

test.describe('3D Playground inputs', () => {
  test.beforeEach(async ({ page }) => {
    await authenticate(page);
  });

  test('output formats follow the provider-job budget the server serves (Rodin)', async ({ page, request }) => {
    await feature('Output formats — the provider-job budget the server serves (Rodin)');
    const spec = await servedFormats(request, RODIN);
    const derivable = spec.derivable_values ?? [];
    // The case this exists for: Rodin's format field is a scalar (max_items 1),
    // yet every container litegen derives comes out of that one job.
    expect(spec.max_items).toBe(1);
    expect(derivable.length, 'Rodin emits glb, so something is derivable').toBeGreaterThan(1);

    await openCompareMode(page);
    await page.getByTestId('pg-model-filter').fill('rodin');
    await pickModel(page, RODIN).check();
    const box = (f: string) => page.getByTestId(`pg-param-output_formats-${f}`);
    // The server serves the list an omitted output_formats resolves to (E11d).
    const defaults = spec.default!;
    expect(defaults.length, 'the served default is never empty').toBeGreaterThan(0);
    await expect(box(defaults[0])).toBeChecked({ timeout: 30_000 });
    await expect(page.getByTestId('pg-param-output_formats-cap')).toHaveText(budgetText(spec).cap);
    await expectBoxesFollowTheBudget(page, spec, defaults);

    // The whole derivable set is still one provider job.
    for (const f of derivable) await box(f).check();
    await expectBoxesFollowTheBudget(page, spec, derivable);
    expect((await compareRequests(page))[RODIN].output_formats).toEqual(inDeclaredOrder(spec, derivable));

    // A container only the vendor can produce is a job of its own, so with one
    // ticked nothing else fits (as long as Rodin still has such a container).
    const ownJob = spec.enum_values.find(f => !derivable.includes(f));
    if (ownJob) {
      for (const f of derivable) await box(f).uncheck();
      await box(ownJob).check();
      await expectBoxesFollowTheBudget(page, spec, [ownJob]);
      expect((await compareRequests(page))[RODIN].output_formats).toEqual([ownJob]);
    }
  });

  test('Single Mode: every derivable container comes back from one mesh-3d job', async ({ page, request }) => {
    await feature('Output formats — every derivable container from one mesh job');
    const spec = await servedFormats(request, MESH_MODEL);
    const derivable = spec.derivable_values ?? [];
    // mock/mesh-3d emits only glb with max_items 1 — before derivable_values
    // the panel could offer nothing else.
    expect(spec.max_items).toBe(1);
    expect(derivable.length).toBeGreaterThan(1);

    await openSingleMode(page, MESH_MODEL);
    const box = (f: string) => page.getByTestId(`pg-param-output_formats-${f}`);
    await expect(box('glb')).toBeChecked({ timeout: 30_000 });
    for (const f of derivable) await box(f).check();

    await page.getByTestId('playground-prompt').fill('a low-poly fox');
    const run = await generateAndCapture(page, MESHES, MESH_POLL);
    // The server delivered one mesh per container asked, each its own file.
    const delivered = (run.done!.assets as Array<{ kind: string; format: string; sha256?: string }>).filter(a => a.kind === 'mesh');
    expect(delivered.map(a => a.format).sort()).toEqual([...derivable].sort());
    expect(new Set(delivered.map(a => a.sha256)).size, 'each container is its own file').toBe(delivered.length);

    const inspector = page.getByTestId('playground-mesh');
    await expect(inspector).toBeVisible({ timeout: JOB_TIMEOUT });
    await expectMeshLoaded(page.getByTestId('playground-mesh-viewer'));
    // The viewer renders the glb; the asset list carries every container asked for.
    await expect(page.getByTestId('playground-mesh-stat-format').locator('dd')).toHaveText('GLB');
    const meshRows = page.getByTestId('playground-mesh-assets').locator('tbody tr')
      .filter({ has: page.locator('.mp-kind--mesh') });
    await expect(meshRows).toHaveCount(derivable.length);
    for (const f of derivable) {
      await expect(meshRows.filter({ hasText: f.toUpperCase() }), `a ${f} mesh`).toHaveCount(1);
    }

    // The request carried the formats and nothing the user left untouched —
    // in particular no client-invented target_polycount.
    const body = run.wire;
    expect(body.output_formats).toEqual(inDeclaredOrder(spec, derivable));
    expect('target_polycount' in body).toBe(false);
    expect('texture' in body).toBe(false);
  });

  test('Single Mode: image-to-3D from an uploaded reference image', async ({ page, request }) => {
    await feature('Image-to-3D — an uploaded image reaches the server byte for byte');
    await openSingleMode(page, MESH_MODEL);
    await expect(page.getByTestId('pg-ref-init')).toBeVisible({ timeout: 30_000 });
    await page.getByTestId('pg-ref-init-file').setInputFiles(IMAGE_FIXTURE);
    await expect(page.getByTestId('pg-ref-init-item-0')).toContainText('image.png (inline)');
    // mock/mesh-3d takes one input image: the add row is gone at the cap.
    await expect(page.getByTestId('pg-ref-init-url')).toHaveCount(0);

    await page.getByTestId('playground-prompt').fill('a fox, from the photo');
    const run = await generateAndCapture(page, MESHES, MESH_POLL);
    await expect(page.getByTestId('playground-mesh')).toBeVisible({ timeout: JOB_TIMEOUT });

    const refs = run.wire.reference_images as Array<{ type: string; role: string; value: string }>;
    expect(refs).toHaveLength(1);
    expect(refs[0].role).toBe('init');
    expect(refs[0].type).toBe('base64');
    expect(refs[0].value, 'the file itself, inline').toBe(`data:image/png;base64,${fs.readFileSync(IMAGE_FIXTURE).toString('base64')}`);
    // The job's row records the reference by its hash (never its bytes): the
    // server received exactly the uploaded file.
    const row = await (await request.get(`http://127.0.0.1:5099/v1/generations/${run.answer.id}`, {
      headers: { Authorization: `Bearer ${MASTER_KEY}` },
    })).json();
    const sha = crypto.createHash('sha256').update(fs.readFileSync(IMAGE_FIXTURE)).digest('hex');
    expect(JSON.stringify(row.metadata?.request ?? row), 'the server hashed the very bytes uploaded').toContain(sha);
    expect(JSON.stringify(row), 'and kept no copy of them').not.toContain(refs[0].value.slice(30, 90));

    // History keeps the run but not the megabytes of inline image.
    await page.getByTestId('playground-history-row-0').click();
    await page.getByTestId('playground-tab-request').click();
    expect(await page.getByTestId('playground-request-json').innerText()).not.toContain('reference_images');
  });

  test('Single Mode: multiview-to-3D from uploaded views, each under its own side', async ({ page, request }) => {
    await feature('Multiview-to-3D — each view reaches the server under its own side');
    await openSingleMode(page, ALL_PARAMS);
    // One row per view the model declares, as the catalog serves them.
    for (const side of ['front', 'back', 'left', 'right', 'top']) {
      await expect(page.getByTestId(`pg-ref-view-${side}`), `a ${side} view row`).toBeVisible({ timeout: 30_000 });
    }
    await page.getByTestId('playground-prompt').fill('a fox, from two sides');
    const generate = page.getByTestId('playground-generate');
    const problems = page.getByTestId('playground-problems');
    // The pre-submit check of the body carrying exactly these roles (E11b).
    const roles = (body: Record<string, unknown>) =>
      ((body.reference_images ?? []) as Array<{ role: string }>).map(r => r.role).sort().join(',');
    const checkOfRoles = (want: string) => checkOf(page, 'models3d', body => roles(body) === want);

    // A side view alone: the server refuses it without the front, in its words.
    let quoted = checkOfRoles('view-left');
    await page.getByTestId('pg-ref-view-left-file').setInputFiles(SIDE_FIXTURE);
    expect((await quoted).status()).toBe(400);
    await expect(problems).toContainText('needs a view-front image');
    await expect(generate).toBeDisabled();
    quoted = checkOfRoles('view-front,view-left');
    await page.getByTestId('pg-ref-view-front-file').setInputFiles(IMAGE_FIXTURE);
    expect((await quoted).status()).toBe(200);
    await checked(page);
    await expect(problems).toHaveCount(0);
    await expect(generate).toBeEnabled();

    // An input image beside the views would be image-to-3D and multiview at once.
    quoted = checkOfRoles('init,view-front,view-left');
    await page.getByTestId('pg-ref-init-file').setInputFiles(IMAGE_FIXTURE);
    expect((await quoted).status()).toBe(400);
    await expect(problems).toContainText('not both');
    await expect(generate).toBeDisabled();
    // Back to the body already checked: its verdict stands, nothing is asked again.
    await page.getByTestId('pg-ref-init-remove-0').click();
    await checked(page);
    await expect(problems).toHaveCount(0);
    await expect(generate).toBeEnabled();

    const run = await generateAndCapture(page, MESHES, MESH_POLL);
    await expectMeshLoaded(page.getByTestId('playground-mesh-viewer'));

    // Each file went inline under its own side, and the row records each side
    // by the hash of exactly the bytes uploaded for it.
    const files: Record<string, string> = { 'view-front': IMAGE_FIXTURE, 'view-left': SIDE_FIXTURE };
    const refs = run.wire.reference_images as Array<{ type: string; role: string; value: string }>;
    expect(refs.map(r => r.role).sort()).toEqual(Object.keys(files));
    const row = await (await request.get(`http://127.0.0.1:5099/v1/generations/${run.answer.id}`, {
      headers: { Authorization: `Bearer ${MASTER_KEY}` },
    })).json();
    const recorded = (row.metadata?.request?.reference_images ?? []) as Array<{ role: string; sha256?: string }>;
    for (const [role, file] of Object.entries(files)) {
      const bytes = fs.readFileSync(file);
      expect(refs.find(r => r.role === role)?.value, `${role} is its file, inline`).toBe(`data:image/png;base64,${bytes.toString('base64')}`);
      expect(recorded.find(r => r.role === role)?.sha256, `the server hashed the ${role} bytes`)
        .toBe(crypto.createHash('sha256').update(bytes).digest('hex'));
    }
  });

  test("Single Mode: an operation with no mesh to work on is held back, in the server's words", async ({ page }) => {
    await feature('Pre-submit check — a rig with no mesh is refused by the server, and the page says so');
    await openSingleMode(page, 'mock/rig-3d');
    const generate = page.getByTestId('playground-generate');
    const problems = page.getByTestId('playground-problems');
    // The quote route runs the generation's validation and calls no vendor.
    await expect(problems).toContainText('needs source_model: the mesh to work on');
    await expect(generate).toBeDisabled();

    await feature('Pre-submit check — a mesh named, the quote passes, and Generate opens');
    const id = 'litegen-3d-does-not-exist';
    const quoted = checkOf(page, 'models3d', body => (body.source_model as { value?: string } | undefined)?.value === id);
    await page.getByTestId('pg-param-source_model-value').fill(id);
    // The quote checks the request's shape only; the run would read the mesh.
    expect((await quoted).status(), 'the server takes the body').toBe(200);
    await checked(page);
    await expect(problems).toHaveCount(0);
    await expect(generate).toBeEnabled();
    // Nothing is generated: the id names no mesh.
  });

  test('Single Mode: a model that takes no prompt hides the box and waits for its image', async ({ page }) => {
    await feature('A model that takes no prompt — no prompt box, Generate waits for its image');
    await openSingleMode(page, TRELLIS);
    // fal/trellis is served with prompt.max_length 0: no prompt box at all.
    await expect(page.getByTestId('playground-prompt-unused')).toBeVisible({ timeout: 30_000 });
    await expect(page.getByTestId('playground-prompt')).toHaveCount(0);

    // Its input image is required: Generate stays off and says why, in the server's words.
    const generate = page.getByTestId('playground-generate');
    await expect(page.getByTestId('playground-problems')).toContainText("role 'init' is required");
    await expect(generate).toBeDisabled();
    await expect(page.getByTestId('pg-ref-init-required')).toHaveText('required');

    // With the image, the request passes validation; fal has no key here, so
    // the quote answers provider_not_configured: no verdict, the run would say.
    const quoted = checkOf(page, 'models3d', body => Array.isArray(body.reference_images) && body.reference_images.length === 1);
    await page.getByTestId('pg-ref-init-url').fill('https://example.com/chair.png');
    await page.getByTestId('pg-ref-init-add-url').click();
    await expect(page.getByTestId('pg-ref-init-item-0')).toContainText('https://example.com/chair.png');
    const answer = await quoted;
    if (!answer.ok()) expect(((await answer.json()) as { error?: { type?: string } }).error?.type).toBe('provider_not_configured');
    await checked(page);
    await expect(generate).toBeEnabled();
    await expect(page.getByTestId('playground-problems')).toHaveCount(0);

    // The vendor's API reference, and the vendor-only knobs it documents.
    await expect(page.getByTestId('playground-model-docs')).toHaveAttribute('href', /^https:\/\/fal\.ai\/models\//);
    await page.getByTestId('pg-extras-toggle').click();
    await expect(page.getByTestId('pg-extra-texture_size')).toBeVisible();
    // Nothing is generated: fal is never called from this suite.
  });

  test('Compare: a shared prompt is not sent to a model that takes none', async ({ page }) => {
    await feature('Compare — a shared prompt is withheld from a model that takes none');
    await openCompareMode(page);
    await page.getByTestId('pg-model-filter').fill('3d');
    await pickModel(page, MESH_MODEL).check();
    await page.getByTestId('pg-model-filter').fill('trellis');
    await pickModel(page, TRELLIS).check();
    await expect(page.getByTestId('pg-selected-count')).toContainText('2 selected');

    await page.getByTestId('pg-prompt').fill('a fox');
    await expect(page.getByTestId('pg-prompt-not-sent')).toContainText('trellis');
    // TRELLIS still needs its input image before the run can start.
    await expect(page.getByTestId(`pg-problems-${TRELLIS}`)).toContainText("role 'init' is required");
    await expect(page.getByTestId('pg-generate')).toBeDisabled();

    const quoted = checkOf(page, 'models3d', body => body.model === TRELLIS && Array.isArray(body.reference_images));
    await page.getByTestId('pg-ref-init-url').fill('https://example.com/chair.png');
    await page.getByTestId('pg-ref-init-add-url').click();
    const answer = await quoted;
    if (!answer.ok()) expect(((await answer.json()) as { error?: { type?: string } }).error?.type).toBe('provider_not_configured');
    await checked(page, 'pg-generate');
    await expect(page.getByTestId('pg-problems')).toHaveCount(0);
    await expect(page.getByTestId('pg-generate')).toBeEnabled();

    const reqs = await compareRequests(page);
    // In strict mode a prompt on TRELLIS is a 400: it must not be there.
    expect('prompt' in reqs[TRELLIS]).toBe(false);
    expect(reqs[TRELLIS].reference_images).toEqual([{ type: 'url', value: 'https://example.com/chair.png', role: 'init' }]);
    expect(reqs[MESH_MODEL].prompt).toBe('a fox');
    // n is image-only; neither 3D request carries it.
    expect('n' in reqs[TRELLIS]).toBe(false);
    expect('n' in reqs[MESH_MODEL]).toBe(false);
  });

  test('a model that takes no seed or negative prompt is sent neither, in either mode', async ({ page }) => {
    await feature('Lax mode — a model is sent only what it declares; nothing is dropped');
    await openSingleMode(page, MESH_MODEL);
    await page.getByTestId('playground-prompt').fill('a fox');
    // mock/mesh-3d declares neither: both controls are off, so a value left
    // from another model can never reach it (strict mode would refuse it,
    // lax mode drop it — X-Litegen-Dropped-Params, covered server-side).
    await expect(page.getByTestId('playground-seed')).toBeDisabled();
    await expect(page.getByTestId('playground-negative-prompt')).toBeDisabled();
    await page.getByTestId('playground-strict').uncheck();
    const submit = nextCall(page, 'POST', MESHES);
    const run = generateAndCapture(page, MESHES, MESH_POLL);
    const answered = await submit;
    // The server dropped nothing, because nothing undeclared was sent.
    expect(answered.headers()['x-litegen-dropped-params'], 'no X-Litegen-Dropped-Params').toBeUndefined();
    const { wire } = await run;
    await expect(page.getByTestId('playground-mesh')).toBeVisible({ timeout: JOB_TIMEOUT });
    await expect(page.getByTestId('playground-dropped')).toHaveCount(0);
    expect(wire).not.toHaveProperty('seed');
    expect(wire).not.toHaveProperty('negative_prompt');
    expect(wire.strict, 'lax mode is what was sent').toBe(false);
  });
});

test.describe('Model detail drawer for a 3D model', () => {
  test.use({ permissions: ['clipboard-read', 'clipboard-write'] });

  test.beforeEach(async ({ page }) => {
    await authenticate(page);
  });

  test('states what fal/trellis takes, links its API reference, and copies a request that validates', async ({ page, request }) => {
    await feature('Model drawer — fal/trellis: inputs, docs link, a copied request the server accepts');
    await page.goto('/models');
    await page.getByTestId(`model-row-${TRELLIS}`).click();
    const panel = page.getByTestId('model-detail-panel');
    await expect(panel).toBeVisible();

    await expect(page.getByTestId('model-detail-docs')).toHaveAttribute('href', 'https://fal.ai/models/fal-ai/trellis/api');
    await expect(page.getByTestId('model-detail-prompt')).toContainText('Not used');
    await expect(page.getByTestId('model-detail-ref-init')).toContainText('required · exactly 1');
    const formats = await servedFormats(request, TRELLIS);
    await expect(page.getByTestId('param-output_formats'))
      .toContainText(`${formats.enum_values.join(' | ')} · ${budgetText(formats).drawer}`);
    await expect(page.getByTestId('model-detail-extra')).toContainText('texture_size');

    await page.getByTestId('model-detail-copy-curl').click();
    const curl = await page.evaluate(() => navigator.clipboard.readText());
    const body = JSON.parse(curl.slice(curl.indexOf("-d '") + 4, curl.lastIndexOf("'"))) as Record<string, unknown>;
    expect(body).toEqual({
      model: TRELLIS,
      reference_images: [{ type: 'url', value: 'https://example.com/input.png', role: 'init' }],
    });
    // The copied body passes validation. The cost endpoint validates first and
    // never generates: a body it refused would come back as a validation_error.
    // Past validation, an unconfigured fal answers provider_not_configured and
    // a configured one returns a price — either way the request was valid.
    const cost = await request.post('http://127.0.0.1:5099/v1/models3d/cost', {
      headers: { Authorization: `Bearer ${MASTER_KEY}` },
      data: body,
    });
    if (!cost.ok()) {
      const err = ((await cost.json()) as { error?: { type?: string } }).error;
      expect(err?.type, `copied body was refused: ${JSON.stringify(err)}`).toBe('provider_not_configured');
    }
  });
});
