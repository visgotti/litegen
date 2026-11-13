import { expect, test, type Page, type Response } from '@playwright/test';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

// Shared by the e2e specs: what went over the wire, what came back, and what
// the returned media really contains. A test that reads only the Playground's
// own "Request" tab proves what the page meant to send, not what the server
// got or did; these helpers check the request on the wire against that tab,
// take the server's own answer, and decode the bytes it returned.

export const MASTER_KEY = process.env.PLAYWRIGHT_MASTER_KEY ?? 'test-master-key-please-rotate';
/** Direct-to-backend base for raw API calls (bypasses the dashboard proxy). */
export const API_BASE = process.env.LITEGEN_API_BASE ?? 'http://127.0.0.1:5099';
export const authHeaders = { Authorization: `Bearer ${MASTER_KEY}` };

const here = path.dirname(fileURLToPath(import.meta.url));
/** A real 256×256 PNG (the Storybook fixture), uploaded as an inline image. */
export const IMAGE_FIXTURE = path.resolve(here, '../.storybook/fixtures/image.png');
export const FIXTURE_DATA_URI = `data:image/png;base64,${fs.readFileSync(IMAGE_FIXTURE).toString('base64')}`;

/** Names the feature the next part of a test exercises: a step of its own,
 *  so the run's timeline (and the annotated video) can say where it starts. */
export async function feature(name: string): Promise<void> {
  await test.step(`§ ${name}`, async () => {});
}

const pathOf = (r: Response) => new URL(r.url()).pathname;

/** The next `method` call to a path matching `route`, and its answer: arm it
 *  BEFORE the click that sends it. */
export function nextCall(page: Page, method: string, route: RegExp, timeout = 90_000): Promise<Response> {
  return page.waitForResponse(r => r.request().method() === method && route.test(pathOf(r)), { timeout });
}

/** The poll that saw a job (`/v1/videos/…`, `/v1/models3d/…`) reach a
 *  terminal state, with its body. Arm it before the click that starts the job. */
export function terminalPoll(page: Page, route: RegExp, timeout = 120_000): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { page.off('response', onResponse); reject(new Error(`no terminal poll of ${route}`)); }, timeout);
    const onResponse = async (r: Response) => {
      if (r.request().method() !== 'GET' || !route.test(pathOf(r))) return;
      let body: Record<string, unknown>;
      try { body = (await r.json()) as Record<string, unknown>; } catch { return; }
      if (body.status === 'completed' || body.status === 'failed' || body.status === 'cancelled') {
        clearTimeout(timer);
        page.off('response', onResponse);
        resolve(body);
      }
    };
    page.on('response', onResponse);
  });
}

/** The request the Playground's Request tab says it sent. */
export async function requestTab(page: Page): Promise<Record<string, unknown>> {
  await page.getByTestId('playground-tab-request').click();
  const body = JSON.parse(await page.getByTestId('playground-request-json').innerText()) as Record<string, unknown>;
  await page.getByTestId('playground-tab-image').click();
  return body;
}

/** Click Generate and return the request that went over the wire to `route`,
 *  its answer, and the terminal poll when the job is asynchronous. The
 *  Request tab must show exactly the body on the wire. */
export async function generateAndCapture(page: Page, route: RegExp, poll?: RegExp) {
  const button = page.getByTestId('playground-generate');
  await expect(button).toBeEnabled();
  const call = nextCall(page, 'POST', route);
  const terminal = poll ? terminalPoll(page, poll) : undefined;
  await button.click();
  const response = await call;
  const wire = response.request().postDataJSON() as Record<string, unknown>;
  expect(response.status(), `${route} answered ${response.status()}: ${await response.text()}`).toBe(200);
  const answer = (await response.json()) as Record<string, unknown>;
  const done = terminal ? await terminal : undefined;
  if (done) expect(done.status, `the job ended ${done.status}: ${JSON.stringify(done.error ?? '')}`).toBe('completed');
  await expect(button).toHaveText('Generate', { timeout: 120_000 });
  await expect(page.locator('.alert-error')).toHaveCount(0);
  expect(await requestTab(page), 'the Request tab shows what went over the wire').toEqual(wire);
  return { wire, answer, done };
}

/** RGBA at each point of an image (a data: URI or URL the page can load),
 *  with its size — decoded in the browser, as the page displays it. */
export async function pixelsAt(page: Page, src: string, points: Array<[number, number]>) {
  return page.evaluate(async ({ src, points }) => {
    const img = new Image();
    img.src = src;
    await img.decode();
    const c = document.createElement('canvas');
    c.width = img.naturalWidth;
    c.height = img.naturalHeight;
    const g = c.getContext('2d', { willReadFrequently: true })!;
    g.drawImage(img, 0, 0);
    const d = g.getImageData(0, 0, c.width, c.height).data;
    return {
      width: c.width,
      height: c.height,
      at: points.map(([x, y]) => Array.from(d.slice((y * c.width + x) * 4, (y * c.width + x) * 4 + 4))),
    };
  }, { src, points });
}

/** Every distinct colour in an image, as #rrggbb (alpha ignored). */
export async function coloursOf(page: Page, src: string): Promise<string[]> {
  return page.evaluate(async (src) => {
    const img = new Image();
    img.src = src;
    await img.decode();
    const c = document.createElement('canvas');
    c.width = img.naturalWidth;
    c.height = img.naturalHeight;
    const g = c.getContext('2d', { willReadFrequently: true })!;
    g.drawImage(img, 0, 0);
    const d = g.getImageData(0, 0, c.width, c.height).data;
    const seen = new Set<string>();
    for (let i = 0; i < d.length; i += 4) {
      seen.add('#' + [d[i], d[i + 1], d[i + 2]].map(v => v.toString(16).padStart(2, '0')).join(''));
    }
    return [...seen].sort();
  }, src);
}

/** RGB of a #rrggbb colour. */
export const rgb = (hex: string) => [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16));

/** A GIF's frames (count, and a fingerprint of each asked index), decoded
 *  with the browser's WebCodecs ImageDecoder from bytes fetched server-side. */
export async function gifFrames(page: Page, gifBase64: string, indices: number[]) {
  return page.evaluate(async ({ gifBase64, indices }) => {
    const bytes = Uint8Array.from(atob(gifBase64), ch => ch.charCodeAt(0));
    // @ts-expect-error ImageDecoder is WebCodecs, in Chromium.
    const decoder = new ImageDecoder({ data: bytes, type: 'image/gif' });
    await decoder.tracks.ready;
    const count = decoder.tracks.selectedTrack.frameCount as number;
    const prints: string[] = [];
    for (const index of indices) {
      const { image } = await decoder.decode({ frameIndex: index < 0 ? count + index : index });
      const c = new OffscreenCanvas(image.displayWidth, image.displayHeight);
      const g = c.getContext('2d')!;
      g.drawImage(image, 0, 0);
      const d = g.getImageData(0, 0, c.width, c.height).data;
      let h = 2166136261;
      for (let i = 0; i < d.length; i++) h = Math.imul(h ^ d[i], 16777619);
      prints.push(`${c.width}x${c.height}:${(h >>> 0).toString(16)}`);
      image.close();
    }
    return { count, prints };
  }, { gifBase64, indices });
}
