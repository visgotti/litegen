import { test, expect } from '@playwright/test';
import type { APIRequestContext, Page } from '@playwright/test';
import {
  API_BASE, FIXTURE_DATA_URI, IMAGE_FIXTURE, MASTER_KEY, authHeaders, coloursOf, feature,
  generateAndCapture, gifFrames, pixelsAt, rgb,
} from './support';

// End-to-end coverage for the Playground controls of the 3D doc / PixelGenie
// params (Task 43). Each test drives one mock model — free and deterministic —
// against the real gateway, and checks three things: the request on the wire
// is the one the Request tab shows; it carries the param as the API takes it;
// and what came back shows the server applied it (the mocks' documented
// effects in models/mock.yaml: the region filled with #ffcd75 and nothing
// else, the canvas grown, the palette's colours only, a transparent frame,
// one image per view, a clip of the asked length, the source frames kept…).

const IMAGES = /^\/v1\/images\/generations$/;
const VIDEOS = /^\/v1\/videos\/generations$/;
const VIDEO_POLL = /^\/v1\/videos\/litegen-vid-[^/]+$/;
const MESHES = /^\/v1\/models3d\/generations$/;
const MESH_POLL = /^\/v1\/models3d\/litegen-3d-[^/]+$/;

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

/** A solid 256×256 PNG drawn by the browser, for a second, distinct upload. */
async function solidPng(page: Page, colour: string): Promise<Buffer> {
  const dataUri = await page.evaluate((colour) => {
    const c = document.createElement('canvas');
    c.width = 256;
    c.height = 256;
    const g = c.getContext('2d')!;
    g.fillStyle = colour;
    g.fillRect(0, 0, 256, 256);
    return c.toDataURL('image/png');
  }, colour);
  return Buffer.from(dataUri.split(',')[1], 'base64');
}

/** The image the server answered with, as a data: URI the page can decode. */
function firstImage(answer: Record<string, unknown>): string {
  const data = answer.data as Array<{ b64_json?: string }>;
  expect(data?.[0]?.b64_json, 'the answer carries the image itself').toBeTruthy();
  return `data:image/png;base64,${data[0].b64_json}`;
}

/** A finished clip's bytes, fetched from the URL the server answered with. */
async function clipBase64(request: APIRequestContext, done: Record<string, unknown>): Promise<string> {
  const url = done.video_url as string;
  expect(url, 'a finished clip names its URL').toMatch(/^https?:\/\//);
  const res = await request.get(url, { headers: authHeaders });
  expect(res.status()).toBe(200);
  return (await res.body()).toString('base64');
}

async function clipOf(request: APIRequestContext, id: string): Promise<string> {
  const res = await request.get(`${API_BASE}/v1/videos/${id}`, { headers: authHeaders });
  expect(res.status()).toBe(200);
  return clipBase64(request, (await res.json()) as Record<string, unknown>);
}

const near = (a: number[], b: number[], tolerance: number) => a.every((v, i) => Math.abs(v - b[i]) <= tolerance);

test.describe('Playground: structured params', () => {
  test.beforeEach(async ({ page }) => {
    await authenticate(page);
    await page.goto('/playground');
  });

  test('a region edit changes only the grown region and keeps every other pixel', async ({ page }) => {
    await feature('Region edit — box, mask growth, kept pixels');
    await openModel(page, 'mock/region-image');
    await page.getByTestId('playground-prompt').fill('a brass key');
    await upload(page, 'init');
    for (const [k, v] of Object.entries({ x: '64', y: '64', w: '128', h: '128' })) {
      await page.getByTestId(`pg-param-region-${k}`).fill(v);
    }
    await page.getByTestId('pg-param-mask_grow_px').fill('8');
    await page.getByTestId('pg-param-preserve_unmasked').check();
    const { wire, answer } = await generateAndCapture(page, IMAGES);
    expect(wire.region).toEqual({ x: 64, y: 64, w: 128, h: 128 });
    expect(wire.mask_grow_px).toBe(8);
    expect(wire.preserve_unmasked).toBe(true);

    await feature('Region edit — the server filled exactly the grown region');
    const out = firstImage(answer);
    await expect(page.getByTestId('playground-image')).toHaveAttribute('src', out);
    // Inside the region; 4 px outside it (inside the 8 px growth); 12 px
    // outside it and in two corners (untouched).
    const points: Array<[number, number]> = [[128, 128], [60, 128], [52, 128], [20, 20], [230, 230]];
    const got = await pixelsAt(page, out, points);
    const init = await pixelsAt(page, FIXTURE_DATA_URI, points);
    expect([got.width, got.height]).toEqual([init.width, init.height]);
    const fill = rgb('#ffcd75');
    for (const i of [1, 2, 3, 4]) expect(init.at[i].slice(0, 3), 'the fixture is not the fill colour there').not.toEqual(fill);
    expect(got.at[0].slice(0, 3), 'the region is edited').toEqual(fill);
    expect(got.at[1].slice(0, 3), 'mask_grow_px widened the edit').toEqual(fill);
    expect(got.at[2], 'past the growth, the init pixel').toEqual(init.at[2]);
    expect(got.at[3]).toEqual(init.at[3]);
    expect(got.at[4]).toEqual(init.at[4]);
  });

  test('an expand grows the canvas on the sides asked, the init kept exactly', async ({ page }) => {
    await feature('Expand — sides that grow');
    await openModel(page, 'mock/expand-image');
    await upload(page, 'init');
    await page.getByTestId('pg-param-expand-left').fill('64');
    await page.getByTestId('pg-param-expand-right').fill('64');
    const { wire, answer } = await generateAndCapture(page, IMAGES);
    expect(wire.expand).toEqual({ left: 64, top: 0, right: 64, bottom: 0 });

    await feature('Expand — 64 px each side, new pixels #ffcd75, the init at (64, 0)');
    const out = firstImage(answer);
    const got = await pixelsAt(page, out, [[10, 128], [64 + 10, 128], [64 + 250, 128], [380, 128]]);
    const init = await pixelsAt(page, FIXTURE_DATA_URI, [[10, 128], [250, 128]]);
    expect([got.width, got.height]).toEqual([init.width + 128, init.height]);
    expect(got.at[0].slice(0, 3)).toEqual(rgb('#ffcd75'));
    expect(got.at[1]).toEqual(init.at[0]);
    expect(got.at[2]).toEqual(init.at[1]);
    expect(got.at[3].slice(0, 3)).toEqual(rgb('#ffcd75'));
  });

  test('layout boxes go as fractions and land where their fractions put them', async ({ page }) => {
    await feature('Layout — boxes as fractions; one off the canvas is not sent');
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
    const { wire, answer } = await generateAndCapture(page, IMAGES);
    expect(wire.layout).toEqual([
      { kind: 'text', text: 'SALE', box: { x: 0.1, y: 0.1, w: 0.3, h: 0.3 } },
      { kind: 'object', prompt: 'a red bicycle', box: { x: 0.5, y: 0.1, w: 0.3, h: 0.3 } },
    ]);

    await feature('Layout — box 0 painted #e6194b, box 1 #3cb44b, the rest grey');
    const got = await pixelsAt(page, firstImage(answer), [[64, 64], [166, 64], [64, 200]]);
    expect([got.width, got.height]).toEqual([256, 256]);
    expect(got.at[0].slice(0, 3)).toEqual(rgb('#e6194b'));
    expect(got.at[1].slice(0, 3)).toEqual(rgb('#3cb44b'));
    const [r, g, b] = got.at[2];
    expect(r === g && g === b, `outside every box is grey: ${got.at[2]}`).toBe(true);
  });

  test('a palette constrains the pixel art to exactly its colours', async ({ page }) => {
    await feature('Palette — colours capped at what the model takes');
    await openModel(page, 'mock/pixel-art-image');
    await page.getByTestId('playground-prompt').fill('a knight');
    await page.getByTestId('pg-param-palette-add').click();
    await page.getByTestId('pg-param-palette-add').click();
    await page.getByTestId('pg-param-palette-color-1').fill('#ffcd75');
    const { wire, answer } = await generateAndCapture(page, IMAGES);
    expect(wire.palette).toEqual(['#4a90d9', '#ffcd75']);

    await feature('Palette — the native 32 px art uses those two colours and no other');
    const out = firstImage(answer);
    const size = await pixelsAt(page, out, [[0, 0]]);
    expect([size.width, size.height]).toEqual([32, 32]);
    expect(await coloursOf(page, out)).toEqual(['#4a90d9', '#ffcd75']);
  });

  test('background and output format come back as asked', async ({ page }) => {
    await feature('Output format and transparent background');
    await openModel(page, 'mock/format-image');
    await page.getByTestId('playground-prompt').fill('a sticker of a cat');
    await page.getByTestId('pg-param-background').selectOption('transparent');
    // png is the default, so a different container is what shows it is sent.
    await page.getByTestId('pg-param-output_format').selectOption('webp');
    const { wire, answer } = await generateAndCapture(page, IMAGES);
    expect(wire.background).toBe('transparent');
    expect(wire.output_format).toBe('webp');

    await feature('Output format — WebP bytes, labelled by what they are, with a clear frame');
    const item = (answer.data as Array<{ b64_json: string; content_type?: string }>)[0];
    const head = Buffer.from(item.b64_json, 'base64').subarray(0, 12).toString('latin1');
    expect(head.slice(0, 4) + head.slice(8, 12), 'RIFF…WEBP').toBe('RIFFWEBP');
    expect(item.content_type, 'litegen labels the bytes by what they are').toBe('image/webp');
    const got = await pixelsAt(page, `data:image/webp;base64,${item.b64_json}`, [[1, 1], [100, 100]]);
    expect(got.at[0][3], 'the 4 px frame is transparent').toBe(0);
    expect(got.at[1][3], 'the middle is opaque').toBe(255);
  });

  test('multiview returns one image per view ticked, each named with its yaw', async ({ page }) => {
    await feature('Multiview — the views ticked');
    await openModel(page, 'mock/multiview-image');
    await page.getByTestId('playground-prompt').fill('a teapot');
    await upload(page, 'init');
    // The default is front, left, back and right.
    await expect(page.getByTestId('pg-param-views-left')).toBeChecked();
    await page.getByTestId('pg-param-views-left').uncheck();
    await page.getByTestId('pg-param-views-right').uncheck();
    const { wire, answer } = await generateAndCapture(page, IMAGES);
    expect(wire.views).toEqual(['front', 'back']);

    await feature('Multiview — front at 0°, back at 180°, each its own image');
    const data = answer.data as Array<{ b64_json: string; view?: string; yaw_deg?: number }>;
    expect(data.map(d => [d.view, d.yaw_deg])).toEqual([['front', 0], ['back', 180]]);
    expect(data[0].b64_json, 'a different tint per view').not.toBe(data[1].b64_json);
    await expect(page.getByTestId('playground-image')).toHaveAttribute('src', firstImage(answer));
  });
});

test.describe('Playground: video', () => {
  test.beforeEach(async ({ page }) => {
    await authenticate(page);
    await page.goto('/playground');
  });

  test('a clip runs with its camera, audio and loop, at a listed length', async ({ page, request }) => {
    await feature('Video — listed lengths, camera, audio, loop');
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
    const { wire, done } = await generateAndCapture(page, VIDEOS, VIDEO_POLL);
    expect(wire).toMatchObject({ duration_seconds: 4, camera: 'fixed', audio: true, loop: true });
    expect(wire).not.toHaveProperty('n');
    expect(wire).not.toHaveProperty('size');

    await feature('Video — a 4 s, 10 fps, 64 px clip that ends on its first frame');
    expect(done!.media).toMatchObject({ duration_s: 4, fps: 10, width: 64, height: 64 });
    const frames = await gifFrames(page, await clipBase64(request, done!), [0, -1]);
    expect(frames.count).toBe(40);
    expect(frames.prints[1], 'loop: the last frame is the first').toBe(frames.prints[0]);
    await expect(page.getByTestId('playground-video')).toBeVisible();
  });

  test('keyframes go with the times they are pinned at, and the clip shows each there', async ({ page, request }) => {
    await feature('Keyframes — images pinned at 0 s and 2.5 s');
    await openModel(page, 'mock/keyframe-video');
    await page.getByTestId('playground-prompt').fill('a sunrise');
    await upload(page, 'keyframe');
    await page.getByTestId('pg-ref-keyframe-file').setInputFiles({ name: 'red.png', mimeType: 'image/png', buffer: await solidPng(page, '#ff0000') });
    await page.getByTestId('pg-ref-keyframe-time-0').fill('0');
    await page.getByTestId('pg-ref-keyframe-time-1').fill('2.5');
    const { wire, done } = await generateAndCapture(page, VIDEOS, VIDEO_POLL);
    const refs = wire.reference_images as Array<{ role: string; time_s?: number }>;
    expect(refs.map(r => [r.role, r.time_s])).toEqual([['keyframe', 0], ['keyframe', 2.5]]);

    await feature('Keyframes — frame 0 is the photo, frame 25 (2.5 s at 10 fps) the red image');
    const clip = await clipBase64(request, done!);
    const at = await page.evaluate(async ({ clip }) => {
      const bytes = Uint8Array.from(atob(clip), ch => ch.charCodeAt(0));
      // @ts-expect-error WebCodecs ImageDecoder, in Chromium.
      const decoder = new ImageDecoder({ data: bytes, type: 'image/gif' });
      await decoder.tracks.ready;
      const centre = async (frameIndex: number) => {
        const { image } = await decoder.decode({ frameIndex });
        const c = new OffscreenCanvas(image.displayWidth, image.displayHeight);
        const g = c.getContext('2d')!;
        g.drawImage(image, 0, 0);
        const px = Array.from(g.getImageData(c.width >> 1, c.height >> 1, 1, 1).data);
        image.close();
        return px;
      };
      return { first: await centre(0), pinned: await centre(25), between: await centre(12) };
    }, { clip });
    const photo = (await pixelsAt(page, FIXTURE_DATA_URI, [[128, 128]])).at[0];
    expect(near(at.first, photo, 24), `frame 0 ${at.first} is the photo ${photo}`).toBe(true);
    expect(near(at.pinned, [255, 0, 0, 255], 24), `frame 25 ${at.pinned} is the red keyframe`).toBe(true);
    expect(near(at.between, photo, 24) || near(at.between, [255, 0, 0, 255], 24), 'frame 12 is a blend').toBe(false);
    await expect(page.getByTestId('playground-video')).toBeVisible();
  });

  test('a clip of mine is extended, then retaken, its own frames kept exactly', async ({ page, request }) => {
    await feature('Source clip — made, then picked by generation id');
    await openModel(page, 'mock/sprite-video');
    await page.getByTestId('playground-prompt').fill('a square crossing');
    const { answer: made } = await generateAndCapture(page, VIDEOS, VIDEO_POLL);
    await expect(page.getByTestId('playground-video')).toBeVisible();

    await feature('Extend — 1 s more, forward');
    await openModel(page, 'mock/extend-video');
    await page.getByTestId('playground-prompt').fill('keep going');
    const pick = page.getByTestId('pg-param-source_video-pick');
    await expect(pick.locator('option')).not.toHaveCount(1, { timeout: 15_000 });
    await pick.selectOption(made.id as string);
    await page.getByTestId('pg-param-duration_seconds').fill('1');
    const extend = await generateAndCapture(page, VIDEOS, VIDEO_POLL);
    expect(extend.wire.source_video).toEqual({ type: 'generation', value: made.id });

    await feature('Extend — 3 s: the source 2 s, then 1 s generated; the 20 source frames unchanged');
    expect(extend.done!.segments).toEqual([
      { start_s: 0, end_s: 2, origin: 'source' },
      { start_s: 2, end_s: 3, origin: 'generated' },
    ]);
    expect((extend.done!.media as { duration_s: number }).duration_s).toBe(3);
    const source = await clipOf(request, made.id as string);
    const firstTwenty = Array.from({ length: 20 }, (_, i) => i);
    const before = await gifFrames(page, source, firstTwenty);
    const after = await gifFrames(page, await clipBase64(request, extend.done!), [...firstTwenty, 25]);
    expect(after.count).toBe(30);
    expect(after.prints.slice(0, 20)).toEqual(before.prints);
    expect(before.prints, 'a generated frame is new').not.toContain(after.prints[20]);

    await feature('Retake — 0.5 s to 1.5 s of the same clip');
    await openModel(page, 'mock/retake-video');
    await page.getByTestId('playground-prompt').fill('a different middle');
    await page.getByTestId('pg-param-source_video-value').fill(made.id as string);
    await page.getByTestId('pg-param-time_range-start_s').fill('0.5');
    await page.getByTestId('pg-param-time_range-end_s').fill('1.5');
    const retake = await generateAndCapture(page, VIDEOS, VIDEO_POLL);
    expect(retake.wire.source_video).toEqual({ type: 'generation', value: made.id });
    expect(retake.wire.time_range).toEqual({ start_s: 0.5, end_s: 1.5 });

    await feature('Retake — only frames 5–14 regenerated, the rest the source exactly');
    expect(retake.done!.segments).toEqual([
      { start_s: 0, end_s: 0.5, origin: 'source' },
      { start_s: 0.5, end_s: 1.5, origin: 'generated' },
      { start_s: 1.5, end_s: 2, origin: 'source' },
    ]);
    const retaken = await gifFrames(page, await clipBase64(request, retake.done!), firstTwenty);
    for (const i of firstTwenty) {
      if (i >= 5 && i < 15) expect(retaken.prints[i], `frame ${i} regenerated`).not.toBe(before.prints[i]);
      else expect(retaken.prints[i], `frame ${i} kept`).toBe(before.prints[i]);
    }
  });
});

test.describe('Playground: 3D operations', () => {
  test.beforeEach(async ({ page }) => {
    await authenticate(page);
    await page.goto('/playground');
  });

  test('a mesh of mine is rigged, then animated, and the inspector shows the skin and clips the server measured', async ({ page }) => {
    await feature('Mesh — generated for the chain');
    await openModel(page, 'mock/mesh-3d');
    await page.getByTestId('playground-prompt').fill('a robot');
    const mesh = await generateAndCapture(page, MESHES, MESH_POLL);
    await expect(page.getByTestId('playground-mesh')).toBeVisible();
    await expect(page.getByTestId('playground-mesh-facts')).toContainText('Units / up / front');
    const meshScene = (mesh.done!.assets as Array<{ kind: string; scene?: { units: string; up: string } }>).find(a => a.kind === 'mesh')!.scene!;
    expect([meshScene.units, meshScene.up]).toEqual(['m', '+y']);

    await feature('Rig — the mesh by generation id, VRM joint names');
    await openModel(page, 'mock/rig-3d');
    const rigPick = page.getByTestId('pg-param-source_model-pick');
    await expect(rigPick.locator('option')).not.toHaveCount(1, { timeout: 15_000 });
    await rigPick.selectOption(mesh.answer.id as string);
    await page.getByTestId('pg-param-joint_naming').selectOption('vrm');
    const rig = await generateAndCapture(page, MESHES, MESH_POLL);
    expect(rig.wire).toMatchObject({ source_model: { type: 'generation', value: mesh.answer.id }, joint_naming: 'vrm' });

    await feature('Rig — seven joints, named as VRM names them, shown as measured');
    const skin = (rig.done!.assets as Array<{ kind: string; scene?: { skin?: { joint_count: number; joints: string[] } } }>)
      .find(a => a.kind === 'mesh')!.scene!.skin!;
    expect(skin.joint_count).toBe(7);
    expect(skin.joints).toEqual(expect.arrayContaining(['hips', 'head', 'leftUpperArm', 'rightUpperArm', 'leftUpperLeg', 'rightUpperLeg']));
    await expect(page.getByTestId('playground-mesh-facts')).toContainText(`${skin.joint_count} joints`);

    await feature('Animate — the rigged mesh, one looping walk');
    await openModel(page, 'mock/animate-3d');
    const animPick = page.getByTestId('pg-param-source_model-pick');
    await expect(animPick.locator('option')).not.toHaveCount(2, { timeout: 15_000 });
    await animPick.selectOption(rig.answer.id as string);
    await page.getByTestId('pg-param-animations-add').click();
    await page.getByTestId('pg-param-animations-preset-0').selectOption('mock:walk');
    await page.getByTestId('pg-param-animations-loop-0').check();
    const animate = await generateAndCapture(page, MESHES, MESH_POLL);
    // Named for the preset's canonical name: `mock:walk` is not a legal clip name.
    expect(animate.wire.animations).toEqual([{ name: 'walk', preset: 'mock:walk', loop: true }]);

    await feature('Animate — exactly one 1 s clip named walk, loop asked, shown in the inspector');
    const clips = (animate.done!.assets as Array<{ kind: string; scene?: { animations: Array<{ name: string; duration_s: number; loop_requested?: boolean }> } }>)
      .find(a => a.kind === 'mesh')!.scene!.animations;
    expect(clips.map(c => [c.name, c.duration_s, c.loop_requested])).toEqual([['walk', 1, true]]);
    await expect(page.getByTestId('playground-mesh-clips')).toContainText('walk');
  });
});

test.describe('Playground: segmentation', () => {
  test.beforeEach(async ({ page }) => {
    await authenticate(page);
    await page.goto('/playground');
  });

  test('an image is segmented by a point and a box at its own endpoint, and a text prompt goes alone', async ({ page }) => {
    await feature('Segmentation — prompts, and a text prompt alone');
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
    const { wire, answer } = await generateAndCapture(page, /^\/v1\/images\/segmentations$/);
    expect(wire.prompts).toEqual([{ type: 'point', x: 128, y: 128 }, { type: 'box', x: 96, y: 96, w: 64, h: 64 }]);
    expect(wire.image, 'the uploaded image itself, not a URL').toBe(FIXTURE_DATA_URI);

    await feature('Segmentation — one object: the box (which holds the point), white inside, black outside');
    const masks = answer.masks as Array<{ box: Record<string, number>; png_base64: string }>;
    expect(masks).toHaveLength(1);
    expect(masks[0].box).toEqual({ x: 96, y: 96, w: 64, h: 64 });
    const mask = await pixelsAt(page, `data:image/png;base64,${masks[0].png_base64}`, [[128, 128], [97, 97], [20, 20], [200, 128]]);
    expect(mask.at.map(p => p[0])).toEqual([255, 255, 0, 0]);
    await expect(page.getByTestId('playground-masks')).toBeVisible();
    await expect(page.getByTestId('playground-mask-0').locator('img')).toHaveAttribute('src', `data:image/png;base64,${masks[0].png_base64}`);
  });
});

test.describe('Model detail drawer: catalog facts', () => {
  test.beforeEach(async ({ page }) => {
    await authenticate(page);
  });

  test("states an operation's task, and a keyframe model's keyframes and lengths, as the catalog serves them", async ({ page, request }) => {
    await feature('Model drawer — task, source and lengths from the catalog');
    const served = async (id: string) => {
      const res = await request.get(`${API_BASE}/v1/models`, { headers: authHeaders });
      const list = ((await res.json()) as { data: Array<{ id: string; capabilities: Record<string, unknown> }> }).data;
      return list.find(m => m.id === id)!.capabilities;
    };
    const rig = await served('mock/rig-3d');
    const sprite = await served('mock/sprite-video');
    expect(rig.task).toBe('rig');
    expect(sprite.duration_values).toEqual([2, 4]);
    expect(sprite.supports_loop).toBe(true);

    await page.goto('/models');
    await page.getByTestId('model-row-mock/rig-3d').click();
    await expect(page.getByTestId('model-detail-fact-task')).toContainText(rig.task as string);
    await expect(page.getByTestId('param-source_model')).toContainText('by generation id or URL');
    await page.getByTestId('model-detail-close').click();

    await page.getByTestId('model-row-mock/sprite-video').click();
    await expect(page.getByTestId('model-detail-fact-durations'))
      .toContainText((sprite.duration_values as number[]).map(d => `${d} s`).join(', '));
    await expect(page.getByTestId('model-detail-panel')).toContainText('Loop');
  });
});
