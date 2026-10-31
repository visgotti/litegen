import { test, expect } from '@playwright/test';
import type { Page } from '@playwright/test';
import path from 'path';
import { fileURLToPath } from 'url';

// End-to-end coverage for the Playground controls of the 3D doc / PixelGenie
// params (Task 43): every structured param kind, the fields a model takes by
// its shape (mask growth, kept pixels), keyframe times, video and
// segmentation runs, the source picker by generation id, the 3D inspector's
// manifest and clips, and the model detail drawer's catalog facts. Each test
// drives one mock model — free and deterministic — and checks both the
// request the Playground sent and what came back.

const MASTER_KEY = process.env.PLAYWRIGHT_MASTER_KEY ?? 'test-master-key-please-rotate';
const JOB_TIMEOUT = 90_000;

const __dirname = path.dirname(fileURLToPath(import.meta.url));
/** A real 256×256 PNG (the Storybook fixture), uploaded as an inline image. */
const IMAGE_FIXTURE = path.resolve(__dirname, '../.storybook/fixtures/image.png');

async function authenticate(page: Page) {
  await page.addInitScript((key) => {
    localStorage.setItem('litegen_api_key', key);
  }, MASTER_KEY);
}

async function openModel(page: Page, modelId: string) {
  const select = page.getByTestId('playground-model');
  await expect(select).toHaveValue(/.+/, { timeout: 30_000 });
  await select.selectOption(modelId);
  await expect(select).toHaveValue(modelId);
}

async function upload(page: Page, role: string) {
  await page.getByTestId(`pg-ref-${role}-file`).setInputFiles(IMAGE_FIXTURE);
}

/** Generate, wait for the run to end, and return the request it sent. */
async function generate(page: Page): Promise<Record<string, unknown>> {
  const button = page.getByTestId('playground-generate');
  await expect(button).toBeEnabled();
  await button.click();
  await expect(button).toHaveText('Generate', { timeout: JOB_TIMEOUT });
  await expect(page.locator('.alert-error')).toHaveCount(0);
  await page.getByTestId('playground-tab-request').click();
  const body = JSON.parse(await page.getByTestId('playground-request-json').innerText()) as Record<string, unknown>;
  await page.getByTestId('playground-tab-image').click();
  return body;
}

test.describe('Playground: structured params', () => {
  test.beforeEach(async ({ page }) => {
    await authenticate(page);
    await page.goto('/playground');
  });

  test('a region edit sends the box, the mask growth and the kept pixels', async ({ page }) => {
    await openModel(page, 'mock/region-image');
    await page.getByTestId('playground-prompt').fill('a brass key');
    await upload(page, 'init');
    for (const [k, v] of Object.entries({ x: '64', y: '64', w: '128', h: '128' })) {
      await page.getByTestId(`pg-param-region-${k}`).fill(v);
    }
    await page.getByTestId('pg-param-mask_grow_px').fill('8');
    await page.getByTestId('pg-param-preserve_unmasked').check();
    const body = await generate(page);
    expect(body.region).toEqual({ x: 64, y: 64, w: 128, h: 128 });
    expect(body.mask_grow_px).toBe(8);
    expect(body.preserve_unmasked).toBe(true);
    await expect(page.getByTestId('playground-image')).toBeVisible();
  });

  test('an expand sends the sides that grow', async ({ page }) => {
    await openModel(page, 'mock/expand-image');
    await upload(page, 'init');
    await page.getByTestId('pg-param-expand-left').fill('64');
    await page.getByTestId('pg-param-expand-right').fill('64');
    const body = await generate(page);
    expect(body.expand).toEqual({ left: 64, top: 0, right: 64, bottom: 0 });
    await expect(page.getByTestId('playground-image')).toBeVisible();
  });

  test('layout boxes go as fractions, and one off the canvas is flagged and not sent', async ({ page }) => {
    await openModel(page, 'mock/layout-image');
    await page.getByTestId('playground-prompt').fill('a poster');
    await page.getByTestId('pg-param-layout-add').click();
    await page.getByTestId('pg-param-layout-kind-0').selectOption('text');
    await page.getByTestId('pg-param-layout-text-0').fill('SALE');
    await page.getByTestId('pg-param-layout-add').click();
    await page.getByTestId('pg-param-layout-text-1').fill('a red bicycle');
    await page.getByTestId('pg-param-layout-1-x').fill('0.9');
    await expect(page.getByTestId('pg-param-layout-unsent-1')).toBeVisible();
    await page.getByTestId('pg-param-layout-1-x').fill('0.5');
    await expect(page.getByTestId('pg-param-layout-unsent-1')).toHaveCount(0);
    const body = await generate(page);
    expect(body.layout).toEqual([
      { kind: 'text', text: 'SALE', box: { x: 0.1, y: 0.1, w: 0.3, h: 0.3 } },
      { kind: 'object', prompt: 'a red bicycle', box: { x: 0.5, y: 0.1, w: 0.3, h: 0.3 } },
    ]);
    await expect(page.getByTestId('playground-image')).toBeVisible();
  });

  test('a palette goes as its colours, capped at what the model takes', async ({ page }) => {
    await openModel(page, 'mock/pixel-art-image');
    await page.getByTestId('playground-prompt').fill('a knight');
    await page.getByTestId('pg-param-palette-add').click();
    await page.getByTestId('pg-param-palette-add').click();
    await page.getByTestId('pg-param-palette-color-1').fill('#ffcd75');
    const body = await generate(page);
    expect(body.palette).toEqual(['#4a90d9', '#ffcd75']);
    await expect(page.getByTestId('playground-image')).toBeVisible();
  });

  test('background and output format go as chosen', async ({ page }) => {
    await openModel(page, 'mock/format-image');
    await page.getByTestId('playground-prompt').fill('a sticker of a cat');
    await page.getByTestId('pg-param-background').selectOption('transparent');
    // png is the default, so a different container is what shows it is sent.
    await page.getByTestId('pg-param-output_format').selectOption('webp');
    const body = await generate(page);
    expect(body.background).toBe('transparent');
    expect(body.output_format).toBe('webp');
    await expect(page.getByTestId('playground-image')).toBeVisible();
  });

  test('multiview views go as the set ticked', async ({ page }) => {
    await openModel(page, 'mock/multiview-image');
    await page.getByTestId('playground-prompt').fill('a teapot');
    await upload(page, 'init');
    // The default is front, left, back and right.
    await expect(page.getByTestId('pg-param-views-left')).toBeChecked();
    await page.getByTestId('pg-param-views-left').uncheck();
    await page.getByTestId('pg-param-views-right').uncheck();
    const body = await generate(page);
    expect(body.views).toEqual(['front', 'back']);
    await expect(page.getByTestId('playground-image')).toBeVisible();
  });
});

test.describe('Playground: video', () => {
  test.beforeEach(async ({ page }) => {
    await authenticate(page);
    await page.goto('/playground');
  });

  test('a clip runs with its camera, audio and loop, at a listed length', async ({ page }) => {
    await openModel(page, 'mock/sprite-video');
    await page.getByTestId('playground-prompt').fill('a square crossing');
    // Size is an image's; a clip takes a resolution or aspect ratio instead.
    await expect(page.getByTestId('playground-size')).toBeDisabled();
    // Only the listed lengths are offered.
    const duration = page.getByTestId('pg-param-duration_seconds');
    await expect(duration.locator('option')).toHaveText(['2', '4']);
    await duration.selectOption('4');
    await page.getByTestId('pg-param-camera').selectOption('fixed');
    await page.getByTestId('pg-param-audio').check();
    await page.getByTestId('pg-param-loop').check();
    const body = await generate(page);
    expect(body).toMatchObject({ duration_seconds: 4, camera: 'fixed', audio: true, loop: true });
    expect(body).not.toHaveProperty('n');
    expect(body).not.toHaveProperty('size');
    await expect(page.getByTestId('playground-video')).toBeVisible();
  });

  test('keyframes go with the times they are pinned at', async ({ page }) => {
    await openModel(page, 'mock/keyframe-video');
    await page.getByTestId('playground-prompt').fill('a sunrise');
    await upload(page, 'keyframe');
    await upload(page, 'keyframe');
    await page.getByTestId('pg-ref-keyframe-time-0').fill('0');
    await page.getByTestId('pg-ref-keyframe-time-1').fill('2.5');
    const body = await generate(page);
    const refs = body.reference_images as Array<{ role: string; time_s?: number }>;
    expect(refs.map(r => [r.role, r.time_s])).toEqual([['keyframe', 0], ['keyframe', 2.5]]);
    await expect(page.getByTestId('playground-video')).toBeVisible();
  });

  test('a clip of mine is picked by generation id to extend, then to retake a window of', async ({ page }) => {
    await openModel(page, 'mock/sprite-video');
    await page.getByTestId('playground-prompt').fill('a square crossing');
    await generate(page);
    await expect(page.getByTestId('playground-video')).toBeVisible();

    await openModel(page, 'mock/extend-video');
    await page.getByTestId('playground-prompt').fill('keep going');
    const pick = page.getByTestId('pg-param-source_video-pick');
    await expect(pick.locator('option')).not.toHaveCount(1, { timeout: 15_000 });
    await pick.selectOption({ index: 1 });
    const clipId = await pick.inputValue();
    await page.getByTestId('pg-param-duration_seconds').fill('1');
    const extend = await generate(page);
    expect(extend.source_video).toEqual({ type: 'generation', value: clipId });
    await expect(page.getByTestId('playground-video')).toBeVisible();

    await openModel(page, 'mock/retake-video');
    await page.getByTestId('playground-prompt').fill('a different middle');
    await page.getByTestId('pg-param-source_video-value').fill(clipId);
    await page.getByTestId('pg-param-time_range-start_s').fill('0.5');
    await page.getByTestId('pg-param-time_range-end_s').fill('1.5');
    const retake = await generate(page);
    expect(retake.source_video).toEqual({ type: 'generation', value: clipId });
    expect(retake.time_range).toEqual({ start_s: 0.5, end_s: 1.5 });
    await expect(page.getByTestId('playground-video')).toBeVisible();
  });
});

test.describe('Playground: 3D operations', () => {
  test.beforeEach(async ({ page }) => {
    await authenticate(page);
    await page.goto('/playground');
  });

  test('a mesh of mine is rigged, then animated, and the inspector shows its skin and clips', async ({ page }) => {
    await openModel(page, 'mock/mesh-3d');
    await page.getByTestId('playground-prompt').fill('a robot');
    await generate(page);
    await expect(page.getByTestId('playground-mesh')).toBeVisible();
    await expect(page.getByTestId('playground-mesh-scene')).toContainText('Units / up / front');

    await openModel(page, 'mock/rig-3d');
    const rigPick = page.getByTestId('pg-param-source_model-pick');
    await expect(rigPick.locator('option')).not.toHaveCount(1, { timeout: 15_000 });
    await rigPick.selectOption({ index: 1 });
    const meshId = await rigPick.inputValue();
    await page.getByTestId('pg-param-joint_naming').selectOption('vrm');
    const rig = await generate(page);
    expect(rig).toMatchObject({ source_model: { type: 'generation', value: meshId }, joint_naming: 'vrm' });
    await expect(page.getByTestId('playground-mesh-scene')).toContainText('joints');

    await openModel(page, 'mock/animate-3d');
    const animPick = page.getByTestId('pg-param-source_model-pick');
    await expect(animPick.locator('option')).not.toHaveCount(2, { timeout: 15_000 });
    // Newest first: the rigged mesh just made.
    await animPick.selectOption({ index: 1 });
    await page.getByTestId('pg-param-animations-add').click();
    await page.getByTestId('pg-param-animations-preset-0').selectOption('mock:walk');
    await page.getByTestId('pg-param-animations-loop-0').check();
    const animate = await generate(page);
    // Named for the preset's canonical name: `mock:walk` is not a legal clip name.
    expect(animate.animations).toEqual([{ name: 'walk', preset: 'mock:walk', loop: true }]);
    await expect(page.getByTestId('playground-mesh-clips')).toContainText('walk');
  });
});

test.describe('Playground: segmentation', () => {
  test.beforeEach(async ({ page }) => {
    await authenticate(page);
    await page.goto('/playground');
  });

  test('an image is segmented by a point and a box at its own endpoint, and a text prompt goes alone', async ({ page }) => {
    await openModel(page, 'mock/segment-image');
    await expect(page.getByTestId('playground-problems')).toContainText('needs an image to segment');
    await upload(page, 'image');
    await page.getByTestId('pg-param-prompts-add').click();
    await page.getByTestId('pg-param-prompts-0-x').fill('128');
    await page.getByTestId('pg-param-prompts-0-y').fill('128');
    await page.getByTestId('pg-param-prompts-add').click();
    // A text prompt selects every instance of what it names: never with a point.
    await page.getByTestId('pg-param-prompts-type-1').selectOption('text');
    await page.getByTestId('pg-param-prompts-text-1').fill('the square');
    await expect(page.getByTestId('playground-problems')).toContainText('takes a text prompt alone');
    await expect(page.getByTestId('playground-generate')).toBeDisabled();
    await page.getByTestId('pg-param-prompts-type-1').selectOption('box');
    for (const [k, v] of Object.entries({ x: '96', y: '96', w: '64', h: '64' })) {
      await page.getByTestId(`pg-param-prompts-1-${k}`).fill(v);
    }
    const body = await generate(page);
    expect(body.prompts).toEqual([{ type: 'point', x: 128, y: 128 }, { type: 'box', x: 96, y: 96, w: 64, h: 64 }]);
    expect(typeof body.image).toBe('string');
    await expect(page.getByTestId('playground-masks')).toBeVisible();
    await expect(page.getByTestId('playground-mask-0')).toBeVisible();
  });
});

test.describe('Model detail drawer: catalog facts', () => {
  test.beforeEach(async ({ page }) => {
    await authenticate(page);
  });

  test("states an operation's task, and a keyframe model's keyframes and lengths", async ({ page }) => {
    await page.goto('/models');
    await page.getByTestId('model-row-mock/rig-3d').click();
    await expect(page.getByTestId('model-detail-fact-task')).toContainText('rig');
    await expect(page.getByTestId('param-source_model')).toContainText('by generation id or URL');
    await page.getByTestId('model-detail-close').click();

    await page.getByTestId('model-row-mock/sprite-video').click();
    await expect(page.getByTestId('model-detail-fact-durations')).toContainText('2 s, 4 s');
    await expect(page.getByTestId('model-detail-panel')).toContainText('Loop');
  });
});
