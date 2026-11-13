// Opens every story of the built Storybook, the shared viewer package's
// (packages/3d-viewer) included. A story fails on any page error or console
// error. By its tags, a story must also:
//   - `viewer`: finish loading its model and draw it (its silhouette covers
//     at least 1% of the frame), within 8 s when also tagged `viewer-fast`;
//   - `viewer-error`: end in the error state, for the reason its
//     `reason-<why>` tag names when it has one, with a download link, and
//     with Retry exactly when a second attempt can help (`load`, `context`),
//     a context Retry then drawing the model; `block-gltf-loader` stories
//     run with the GLB loader's chunk blocked;
//   - `viewer-downloading`: hold part-way, the bar at 40% over the poster;
//   - `viewer-pending`: hold before any byte arrives, the bar sweeping (no
//     made-up percentage);
//   - `viewer-many`: draw every one of its viewers as it scrolls by, never
//     holding more WebGL contexts than Chrome keeps (16);
//   - `page`: finish the conversion its play function starts.
import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';

interface Entry { id: string; title: string; name: string; type: string; tags?: string[] }
const index = JSON.parse(readFileSync(new URL('../storybook-static/index.json', import.meta.url), 'utf8')) as { entries: Record<string, Entry> };
const stories = Object.values(index.entries).filter((e) => e.type === 'story');

test('the Storybook has its stories', () => {
  // A stories file or a tag that goes missing would drop its tests silently.
  expect(stories).toHaveLength(47);
  const tagged = (tag: string) => stories.filter((s) => (s.tags ?? []).includes(tag)).length;
  expect({
    page: tagged('page'),
    viewer: tagged('viewer'),
    error: tagged('viewer-error'),
    downloading: tagged('viewer-downloading'),
    pending: tagged('viewer-pending'),
    many: tagged('viewer-many'),
    fast: tagged('viewer-fast'),
  }).toEqual({ page: 1, viewer: 23, error: 8, downloading: 1, pending: 1, many: 1, fast: 1 });
});

for (const story of stories) {
  test(`${story.title} / ${story.name}`, async ({ page }) => {
    const tags = story.tags ?? [];
    const blocked = tags.includes('block-gltf-loader');
    // What a story sets up on purpose may be logged: the blocked chunk's
    // failed request, and three.js's own line for a refused context.
    const expected = [
      ...(blocked ? [/Failed to load resource|ERR_FAILED/] : []),
      ...(tags.includes('reason-webgl') ? [/^THREE\.WebGLRenderer: Error creating WebGL context\.$/] : []),
    ];
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
    page.on('console', (m) => {
      if (m.type() === 'error' && !expected.some((re) => re.test(m.text()))) errors.push(`console: ${m.text()}`);
    });
    if (blocked) await page.route(/\/GLTFLoader-[^/]*\.js$/, (route) => route.abort());
    await page.goto(`iframe.html?id=${story.id}&viewMode=story`);
    if (tags.includes('page')) {
      // The story's play function drives a conversion; it ends with both
      // viewers drawn.
      const output = page.getByTestId('output-viewer');
      await expect(output).toHaveAttribute('data-state', 'loaded', { timeout: 60_000 });
      expect(Number(await output.getAttribute('data-coverage'))).toBeGreaterThanOrEqual(0.01);
    } else if (tags.includes('viewer')) {
      const viewer = page.locator('[data-viewer]').first();
      await expect(viewer).toHaveAttribute('data-state', 'loaded', { timeout: tags.includes('viewer-fast') ? 8_000 : 30_000 });
      const coverage = Number(await viewer.getAttribute('data-coverage'));
      expect(coverage, 'the model covers part of the frame').toBeGreaterThanOrEqual(0.01);
    } else if (tags.includes('viewer-error')) {
      const viewer = page.locator('[data-viewer]').first();
      await expect(viewer).toHaveAttribute('data-state', 'error', { timeout: 30_000 });
      const reason = tags.find((t) => t.startsWith('reason-'))?.slice('reason-'.length);
      if (reason) {
        await expect(viewer).toHaveAttribute('data-reason', reason);
        await expect(viewer.getByRole('link', { name: /^Download / })).toHaveAttribute('href', /.+/);
        const retry = viewer.getByRole('button', { name: 'Retry' });
        await expect(retry).toHaveCount(reason === 'load' || reason === 'context' ? 1 : 0);
        if (reason === 'context') {
          // A second context is not taken: Retry draws the model.
          await retry.click();
          await expect(viewer).toHaveAttribute('data-state', 'loaded', { timeout: 30_000 });
          expect(Number(await viewer.getAttribute('data-coverage'))).toBeGreaterThanOrEqual(0.01);
        }
      }
    } else if (tags.includes('viewer-downloading')) {
      const viewer = page.locator('[data-viewer]').first();
      await expect(viewer.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '40', { timeout: 30_000 });
      await expect(viewer).toHaveAttribute('data-state', 'loading');
      await expect(viewer.locator('img.mv-poster')).toBeVisible();
    } else if (tags.includes('viewer-pending')) {
      const viewer = page.locator('[data-viewer]').first();
      await expect(viewer.getByRole('progressbar')).toBeVisible({ timeout: 30_000 });
      await expect(viewer.getByRole('progressbar')).not.toHaveAttribute('aria-valuenow');
      await expect(viewer).toHaveAttribute('data-state', 'loading');
    } else if (tags.includes('viewer-many')) {
      const viewers = page.locator('[data-viewer]');
      // The story renders after Storybook's preview loads, slower under load.
      await expect(viewers).toHaveCount(24, { timeout: 30_000 });
      let most = 0;
      for (let i = 0; i < 24; i++) {
        const viewer = viewers.nth(i);
        await viewer.scrollIntoViewIfNeeded();
        await expect(viewer).toHaveAttribute('data-state', 'loaded', { timeout: 30_000 });
        most = Math.max(most, await page.locator('canvas.mv-canvas').count());
      }
      // The first viewers, long scrolled away, hold no context any more.
      await expect(viewers.first()).toHaveAttribute('data-state', 'idle');
      await expect(viewers.first().locator('canvas')).toHaveCount(0);
      expect(most, 'live WebGL canvases at any one time').toBeLessThanOrEqual(16);
    } else {
      await page.waitForLoadState('networkidle');
    }
    expect(errors, errors.join('\n')).toEqual([]);
  });
}
