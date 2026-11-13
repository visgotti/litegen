// The page's other paths: several files as one model, a zip in, the options
// (shown only where they apply, and taking effect), a zip download, and a
// file that is not what it says.
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type Page } from '@playwright/test';
import { convert } from '@litegen/3d-converter';
import { drawn, nextWrite, openApp, written } from './helpers';

const samples = fileURLToPath(new URL('../public/samples/', import.meta.url));
const testdata = fileURLToPath(new URL('../../../litegen-core/src/mesh/testdata/', import.meta.url));
const corrupt = fileURLToPath(new URL('../../../dashboard/.storybook/fixtures/corrupt.glb', import.meta.url));

/** A sample's files, as the package takes them. */
function sampleFiles(id: string): Record<string, Uint8Array> {
  const list = JSON.parse(readFileSync(path.join(samples, 'samples.json'), 'utf8')) as Array<{ id: string; files: string[] }>;
  const sample = list.find((s) => s.id === id)!;
  return Object.fromEntries(sample.files.map((name) => [name, new Uint8Array(readFileSync(path.join(samples, id, name)))]));
}

/** The bytes the page's download gives. */
async function downloaded(page: Page, testId = 'download'): Promise<Buffer> {
  const [download] = await Promise.all([page.waitForEvent('download'), page.getByTestId(testId).click()]);
  return readFileSync((await download.path())!);
}

const same = (a: Uint8Array, b: Uint8Array) => Buffer.from(a).equals(Buffer.from(b));

test('several files make one model, each in its role', async ({ page }) => {
  const errors = await openApp(page);
  const dir = path.join(samples, 'textured-gltf');
  await page.getByTestId('file-input').setInputFiles(['model.gltf', 'model.bin', 'quadrants.png'].map((f) => path.join(dir, f)));
  await drawn(page.getByTestId('input-viewer'), 'gltf');
  const roles = page.getByTestId('input-files');
  await expect(roles).toContainText('model.bin');
  await expect(roles.locator('.mp-kind--buffer')).toHaveCount(1);
  await expect(roles.locator('.mp-kind--texture')).toHaveCount(1);
  await page.getByTestId('target-glb').click();
  await drawn(page.getByTestId('output-viewer'), 'glb');
  expect(errors).toEqual([]);
});

test('a zip of a package reads as that package', async ({ page }) => {
  const errors = await openApp(page);
  const zipped = convert(new Uint8Array(readFileSync(path.join(testdata, 'fbx/textured_assimp_7500.fbx'))), { to: 'obj', package: 'zip' });
  const file = path.join(mkdtempSync(path.join(tmpdir(), 'converter-e2e-')), 'textured.obj.zip');
  writeFileSync(file, zipped.files[0]!.data);
  await page.getByTestId('file-input').setInputFiles(file);
  await drawn(page.getByTestId('input-viewer'), 'obj');
  await expect(page.getByTestId('input-files').locator('.mp-kind--material_library')).toHaveCount(1);
  await page.getByTestId('target-stl').click();
  await drawn(page.getByTestId('output-viewer'), 'stl');
  expect(errors).toEqual([]);
});

test('options show where they apply', async ({ page }) => {
  const errors = await openApp(page);
  await page.getByTestId('sample-cube-stl').click();
  await drawn(page.getByTestId('input-viewer'), 'stl');
  // STL in, OBJ out: both unitless, so units and up axis apply both ways;
  // the FBX colour space and the light units apply to neither.
  await page.getByTestId('target-obj').click();
  await written(page, 'obj', 'file');
  for (const id of ['option-fromUnits', 'option-fromUp', 'option-units', 'option-up']) await expect(page.getByTestId(id)).toBeVisible();
  for (const id of ['option-fbxColorSpace', 'option-fbxLightUnits', 'option-usdLightUnits', 'option-daeMorphs', 'option-usdzRoot', 'option-usdClips']) {
    await expect(page.getByTestId(id)).toHaveCount(0);
  }
  // glTF declares its own units and axis.
  await page.getByTestId('target-glb').click();
  await written(page, 'glb', 'file');
  await expect(page.getByTestId('option-units')).toHaveCount(0);
  await expect(page.getByTestId('option-up')).toHaveCount(0);
  // FBX has a colour space, light units and a frame rate.
  await page.getByTestId('target-fbx').click();
  await written(page, 'fbx', 'file');
  for (const id of ['option-fbxColorSpace', 'option-fbxLightUnits', 'option-fps']) await expect(page.getByTestId(id)).toBeVisible();
  for (const id of ['option-usdLightUnits', 'option-daeMorphs', 'option-usdzRoot', 'option-usdClips']) await expect(page.getByTestId(id)).toHaveCount(0);
  // USD has light units of its own, a clip choice and a frame rate; a USDZ
  // also its root layer.
  await page.getByTestId('target-usdz').click();
  await written(page, 'usdz', 'file');
  for (const id of ['option-usdLightUnits', 'option-usdzRoot', 'option-usdClips', 'option-fps']) await expect(page.getByTestId(id)).toBeVisible();
  for (const id of ['option-fbxColorSpace', 'option-fbxLightUnits', 'option-daeMorphs']) await expect(page.getByTestId(id)).toHaveCount(0);
  await page.getByTestId('target-usdc').click();
  await written(page, 'usdc', 'file');
  for (const id of ['option-usdLightUnits', 'option-usdClips', 'option-fps']) await expect(page.getByTestId(id)).toBeVisible();
  for (const id of ['option-usdzRoot', 'option-fbxColorSpace', 'option-fbxLightUnits', 'option-daeMorphs']) await expect(page.getByTestId(id)).toHaveCount(0);
  // Collada has its morph choice, and a frame rate.
  await page.getByTestId('target-dae').click();
  await written(page, 'dae', 'file');
  for (const id of ['option-daeMorphs', 'option-fps']) await expect(page.getByTestId(id)).toBeVisible();
  for (const id of ['option-fbxColorSpace', 'option-fbxLightUnits', 'option-usdLightUnits', 'option-usdzRoot', 'option-usdClips']) {
    await expect(page.getByTestId(id)).toHaveCount(0);
  }
  expect(errors).toEqual([]);
});

test('units and up axis take effect', async ({ page }) => {
  const errors = await openApp(page);
  await page.getByTestId('sample-cube-stl').click();
  await drawn(page.getByTestId('input-viewer'), 'stl');
  const dims = (pane: 'input' | 'output') => page.getByTestId(`${pane}-stats`).locator('div', { hasText: 'Dimensions' }).locator('dd');
  // The cube's numbers are 0.05 x 0.07 x 0.09, read as metres by default.
  await expect(dims('input')).toHaveText('0.05 × 0.07 × 0.09 m');
  await page.getByTestId('target-obj').click();
  await written(page, 'obj', 'file');
  // Read as millimetres: a thousand times smaller.
  await nextWrite(page, async () => void (await page.getByTestId('option-fromUnits').selectOption('mm')));
  await expect(dims('input')).toHaveText('5.0e-5 × 7.0e-5 × 9.0e-5 m');
  // Written +Z up: its depth and height trade places.
  await nextWrite(page, async () => void (await page.getByTestId('option-up').selectOption('z')));
  await expect(dims('output')).toHaveText('5.0e-5 × 9.0e-5 × 7.0e-5 m');
  // The download is the package's own conversion with those options.
  const want = convert(sampleFiles('cube-stl'), { to: 'obj', fromUnits: 'mm', up: 'z' }).files[0]!.data;
  expect(same(await downloaded(page), want), 'the downloaded OBJ is the package\'s, with fromUnits mm and up z').toBe(true);
  await expect(page.getByTestId('error')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('a new target leaves the output shown as it is until the new one is written', async ({ page }) => {
  const errors = await openApp(page);
  await page.getByTestId('sample-cube-stl').click();
  await drawn(page.getByTestId('input-viewer'), 'stl');
  await page.getByTestId('target-glb').click();
  await written(page, 'glb', 'file');
  await drawn(page.getByTestId('output-viewer'), 'glb');
  // Every state the output viewer passes through, with the format it shows.
  // (In-page code: the e2e files compile without the DOM's types.)
  type El = { getAttribute(name: string): string | null };
  type Page$ = {
    document: { querySelector(selector: string): El };
    MutationObserver: new (cb: () => void) => { observe(el: El, init: { attributes: boolean; attributeFilter: string[] }): void };
    seen: string[];
  };
  await page.evaluate(() => {
    const w = globalThis as unknown as Page$;
    const el = w.document.querySelector('[data-testid="output-viewer"]');
    w.seen = [];
    new w.MutationObserver(() => w.seen.push(`${el.getAttribute('data-format')}:${el.getAttribute('data-state')}`)).observe(el, { attributes: true, attributeFilter: ['data-state'] });
  });
  // 3MF is stored +Z up: its frame must reach the 3MF, never the GLB on
  // screen while the 3MF is being written.
  await page.getByTestId('target-3mf').click();
  await written(page, '3mf', 'file');
  await drawn(page.getByTestId('output-viewer'), '3mf');
  const seen = await page.evaluate(() => (globalThis as unknown as Page$).seen);
  expect(seen, 'the observer saw the 3MF arrive (it watched the right element)').toContain('3mf:loaded');
  expect(seen.filter((s) => s.startsWith('glb:')), 'the GLB on screen was never drawn again').toEqual([]);
  expect(errors).toEqual([]);
});

test('the FBX colour space and frame rate take effect', async ({ page }) => {
  const errors = await openApp(page);
  // Textured, so the colour space changes the bytes (a plain cube's would not).
  const files = sampleFiles('textured-obj');
  const want = convert(files, { to: 'fbx', fbxColorSpace: 'linear', fps: 24 }).files[0]!.data;
  expect(same(want, convert(files, { to: 'fbx', fps: 24 }).files[0]!.data), 'the colour space changes this file').toBe(false);
  expect(same(want, convert(files, { to: 'fbx', fbxColorSpace: 'linear' }).files[0]!.data), 'the frame rate changes this file').toBe(false);
  await page.getByTestId('sample-textured-obj').click();
  await drawn(page.getByTestId('input-viewer'), 'obj');
  await page.getByTestId('target-fbx').click();
  await written(page, 'fbx', 'file');
  await nextWrite(page, async () => void (await page.getByTestId('option-fbxColorSpace').selectOption('linear')));
  await nextWrite(page, () => page.getByTestId('option-fps').fill('24'));
  expect(same(await downloaded(page), want), 'the downloaded FBX is the package\'s, linear at 24 fps').toBe(true);
  await expect(page.getByTestId('error')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('the FBX light units take effect', async ({ page }) => {
  const errors = await openApp(page);
  // Lit (a directional, a point and a spot light), so the units change the bytes.
  const dae = path.join(testdata, 'dae/assimp/staged_assimp.dae');
  const files = { 'staged_assimp.dae': new Uint8Array(readFileSync(dae)) };
  const want = convert(files, { to: 'fbx', fbxLightUnits: 'blender' }).files[0]!.data;
  expect(same(want, convert(files, { to: 'fbx' }).files[0]!.data), 'the light units change this file').toBe(false);
  await page.getByTestId('file-input').setInputFiles(dae);
  await drawn(page.getByTestId('input-viewer'), 'dae');
  await page.getByTestId('target-fbx').click();
  await written(page, 'fbx', 'file');
  await nextWrite(page, async () => void (await page.getByTestId('option-fbxLightUnits').selectOption('blender')));
  expect(same(await downloaded(page), want), 'the downloaded FBX is the package\'s, in Blender\'s light units').toBe(true);
  await expect(page.getByTestId('error')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('the Collada morph choice takes effect', async ({ page }) => {
  const errors = await openApp(page);
  // Morphed, so keeping the targets changes the bytes.
  const glb = path.join(testdata, 'gltf/gltfpack/morph.glb');
  const files = { 'morph.glb': new Uint8Array(readFileSync(glb)) };
  const want = convert(files, { to: 'dae', daeMorphs: 'keep' }).files[0]!.data;
  expect(same(want, convert(files, { to: 'dae' }).files[0]!.data), 'keep changes this file').toBe(false);
  await page.getByTestId('file-input').setInputFiles(glb);
  await drawn(page.getByTestId('input-viewer'), 'glb');
  await page.getByTestId('target-dae').click();
  await written(page, 'dae', 'file');
  // Baked by default, as every Collada reader opens it.
  await drawn(page.getByTestId('output-viewer'), 'dae');
  await nextWrite(page, async () => void (await page.getByTestId('option-daeMorphs').selectOption('keep')));
  expect(same(await downloaded(page), want), 'the downloaded Collada is the package\'s, its morph targets kept').toBe(true);
  // three.js, which draws the preview, cannot load morph controllers, as the
  // choice's label says; the file is still written and downloads.
  const viewer = page.getByTestId('output-viewer');
  await expect(viewer).toHaveAttribute('data-state', 'error', { timeout: 60_000 });
  await expect(viewer).toHaveAttribute('data-reason', 'parse');
  await expect(page.getByTestId('error')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('the USDZ root choice takes effect, and a crate draws', async ({ page }) => {
  const errors = await openApp(page);
  const glb = path.join(testdata, 'fbx/skinned_small_source.glb');
  const files = { 'skinned.glb': new Uint8Array(readFileSync(glb)) };
  const want = convert(files, { to: 'usdz', usdzRoot: 'usdc' }).files[0]!.data;
  expect(same(want, convert(files, { to: 'usdz' }).files[0]!.data), 'a crate root changes this file').toBe(false);
  await page.getByTestId('file-input').setInputFiles(glb);
  await drawn(page.getByTestId('input-viewer'), 'glb');
  await page.getByTestId('target-usdz').click();
  await written(page, 'usdz', 'file');
  await nextWrite(page, async () => void (await page.getByTestId('option-usdzRoot').selectOption('usdc')));
  expect(same(await downloaded(page), want), 'the downloaded USDZ is the package\'s, rooted in a crate').toBe(true);
  // Rooted in a crate, as Apple's own packagers write it, it still draws.
  await drawn(page.getByTestId('output-viewer'), 'usdz');
  // usdc writes the crate alone, which draws too.
  await page.getByTestId('target-usdc').click();
  await written(page, 'usdc', 'file');
  await drawn(page.getByTestId('output-viewer'), 'usdc');
  expect(same(await downloaded(page), convert(files, { to: 'usdc' }).files[0]!.data), 'the downloaded USDC is the package\'s').toBe(true);
  await expect(page.getByTestId('error')).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('a zip download holds the package', async ({ page }) => {
  const errors = await openApp(page);
  await page.getByTestId('sample-textured-obj').click();
  await drawn(page.getByTestId('input-viewer'), 'obj');
  await page.getByTestId('target-obj').click();
  await page.getByTestId('package-zip').click();
  await written(page, 'obj', 'zip');
  await expect(page.getByTestId('download')).toContainText('.obj.zip');
  const want = convert(sampleFiles('textured-obj'), { to: 'obj', package: 'zip' }).files[0]!.data;
  expect(same(await downloaded(page), want), 'the downloaded zip is the package\'s, every file and folder').toBe(true);
  expect(errors).toEqual([]);
});

test('a file that is not what it says shows its error, and the page still works', async ({ page }) => {
  const errors = await openApp(page);
  await page.getByTestId('file-input').setInputFiles(corrupt);
  const error = page.getByTestId('error');
  await expect(error).toBeVisible({ timeout: 30_000 });
  await expect(error).toHaveAttribute('data-code', 'mesh_malformed');
  await expect(error).toContainText('malformed mesh: chunk at offset 12 declares 4096 bytes but only 80 remain');
  await page.getByTestId('sample-textured-usdz').click();
  await drawn(page.getByTestId('input-viewer'), 'usdz');
  await expect(error).toHaveCount(0);
  expect(errors).toEqual([]);
});

test('when several files could be the model, the page asks which', async ({ page }) => {
  const errors = await openApp(page);
  await page.getByTestId('file-input').setInputFiles([path.join(samples, 'cube-stl/model.stl'), path.join(samples, 'cube-ply/cube_be.ply')]);
  const choices = page.getByTestId('model-choices');
  await expect(choices).toBeVisible({ timeout: 30_000 });
  await expect(choices.getByRole('button')).toHaveText(['cube_be.ply', 'model.stl']);
  await page.getByTestId('model-choice-cube_be.ply').click();
  await drawn(page.getByTestId('input-viewer'), 'ply');
  await expect(page.getByTestId('input-files')).toContainText('model.stl');
  await expect(page.getByTestId('model-choices')).toHaveCount(0);
  await page.getByTestId('target-glb').click();
  await drawn(page.getByTestId('output-viewer'), 'glb');
  expect(errors.filter((e) => !e.includes('input_ambiguous'))).toEqual([]);
});

test('separate files in folders are offered as the zip that keeps them', async ({ page }) => {
  const errors = await openApp(page);
  await page.getByTestId('sample-textured-obj').click();
  await drawn(page.getByTestId('input-viewer'), 'obj');
  await page.getByTestId('target-obj').click();
  await page.getByTestId('package-files').click();
  await expect(page.getByTestId('output-done')).toHaveAttribute('data-package', 'files', { timeout: 30_000 });
  await expect(page.getByTestId('downloads-folders')).toBeVisible();
  const want = convert(sampleFiles('textured-obj'), { to: 'obj', package: 'zip' }).files[0]!.data;
  expect(same(await downloaded(page, 'download-zip'), want), 'the zip keeps textures/ as the model names it').toBe(true);
  expect(errors).toEqual([]);
});

test('the input viewer keeps its WebGL context through many conversions', async ({ page }) => {
  const errors = await openApp(page);
  await page.getByTestId('sample-cube-stl').click();
  await drawn(page.getByTestId('input-viewer'), 'stl');
  // Browsers keep only so many WebGL contexts (Chrome: 16) and lose the
  // oldest live one first: the input viewer's, if each output kept its own.
  const cycle = ['glb', 'obj', 'ply', 'fbx', 'dae', 'gltf'];
  for (let i = 0; i < 24; i++) {
    const to = cycle[i % cycle.length]!;
    await page.getByTestId(`target-${to}`).click();
    await expect(page.getByTestId('output-done')).toHaveAttribute('data-format', to, { timeout: 30_000 });
    await drawn(page.getByTestId('output-viewer'), to);
  }
  // Runs in the page; this project's types have no DOM, hence the shape.
  type Canvas = { getContext(kind: 'webgl2'): { isContextLost(): boolean } | null };
  const lost = await page.getByTestId('input-viewer').locator('canvas').evaluate((c) => (c as unknown as Canvas).getContext('webgl2')?.isContextLost() ?? true);
  expect(lost, 'the input viewer lost its WebGL context').toBe(false);
  expect(errors).toEqual([]);
});
