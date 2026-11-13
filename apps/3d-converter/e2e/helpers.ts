import type { Locator, Page } from '@playwright/test';
import { expect } from '@playwright/test';

function bits(base64: string): Uint8Array {
  return Uint8Array.from(Buffer.from(base64, 'base64'));
}

function popcount(n: number): number {
  let c = 0;
  for (let v = n; v; v &= v - 1) c++;
  return c;
}

/** Intersection over union of two viewer silhouettes (data-silhouette). */
export function iou(a: string, b: string): number {
  const x = bits(a);
  const y = bits(b);
  let inter = 0;
  let union = 0;
  for (let i = 0; i < x.length; i++) {
    inter += popcount(x[i]! & y[i]!);
    union += popcount(x[i]! | y[i]!);
  }
  return union === 0 ? 1 : inter / union;
}

/** The viewer has drawn this format's file. */
export async function drawn(viewer: Locator, format: string): Promise<{ coverage: number; silhouette: string }> {
  await expect(viewer).toHaveAttribute('data-format', format, { timeout: 60_000 });
  await expect(viewer).toHaveAttribute('data-state', /loaded|error/, { timeout: 60_000 });
  await expect(viewer, 'the viewer drew the file').toHaveAttribute('data-state', 'loaded');
  return {
    coverage: Number(await viewer.getAttribute('data-coverage')),
    silhouette: (await viewer.getAttribute('data-silhouette')) ?? '',
  };
}

/** Opens the app and fails the test on any page error. */
/** The page shows the result of this write, not an earlier one: the output
 *  carries the format and packaging it was written with. */
export async function written(page: Page, format: string, packaging: 'file' | 'zip' | 'files'): Promise<void> {
  const done = page.getByTestId('output-done');
  await expect(done).toHaveAttribute('data-format', format, { timeout: 60_000 });
  await expect(done).toHaveAttribute('data-package', packaging, { timeout: 60_000 });
}

/** The next write has replaced the current one: each output's download has
 *  a new blob URL. */
export async function nextWrite(page: Page, change: () => Promise<void>): Promise<void> {
  const before = await page.getByTestId('download').getAttribute('href');
  await change();
  await expect(page.getByTestId('download')).not.toHaveAttribute('href', before ?? '', { timeout: 60_000 });
}

export async function openApp(page: Page): Promise<string[]> {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('./');
  return errors;
}
