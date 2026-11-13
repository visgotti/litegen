// Every 3D story of the built Storybook, as the dashboard shows a result: the
// shared viewer must end in the state the story is about. A story fails on
// any page error or console error, except the one failed request a
// missing-mesh story makes on purpose.
import { readFileSync } from 'node:fs';
import { expect, test, type Page } from '@playwright/test';

interface Entry { id: string; title: string; name: string; type: string }
const index = JSON.parse(readFileSync(new URL('../storybook-static/index.json', import.meta.url), 'utf8')) as { entries: Record<string, Entry> };
const byName = new Map(Object.values(index.entries).filter((e) => e.type === 'story').map((e) => [`${e.title}/${e.name}`, e.id]));

type Expect =
  | { state: 'loaded'; format: string; background?: string }
  | { state: 'error'; reason: 'load' | 'parse' | 'format'; retry: boolean }
  | { state: 'downloading' }
  | { state: 'no-mesh'; message: RegExp };

const PREVIEW = 'Observability/ModelPreview';
const VIEWER = 'Observability/ModelViewer';
const STORIES: Array<[string, string, string, Expect]> = [
  [PREVIEW, 'Full Asset Set', 'story-preview', { state: 'loaded', format: 'glb' }],
  [PREVIEW, 'Mesh Only', 'story-preview', { state: 'loaded', format: 'glb' }],
  [PREVIEW, 'High Polycount', 'story-preview', { state: 'loaded', format: 'glb' }],
  [PREVIEW, 'Light Background', 'story-preview', { state: 'loaded', format: 'glb', background: 'light' }],
  [PREVIEW, 'Checker Background', 'story-preview', { state: 'loaded', format: 'glb', background: 'checker' }],
  [PREVIEW, 'Obj Mesh', 'story-preview', { state: 'loaded', format: 'obj' }],
  [PREVIEW, 'Three Mf Mesh', 'story-preview', { state: 'loaded', format: '3mf' }],
  [PREVIEW, 'Mesh Load Failure', 'story-preview', { state: 'error', reason: 'load', retry: true }],
  [PREVIEW, 'Mesh Parse Failure', 'story-preview', { state: 'error', reason: 'parse', retry: false }],
  [PREVIEW, 'Unknown Format', 'story-preview', { state: 'error', reason: 'format', retry: false }],
  [PREVIEW, 'No Mesh Contract Violation', 'story-preview', { state: 'no-mesh', message: /without a mesh asset/ }],
  [PREVIEW, 'Caller Reported Error', 'story-preview', { state: 'no-mesh', message: /content policy/ }],
  [VIEWER, 'Loaded', 'story-viewer', { state: 'loaded', format: 'glb' }],
  [VIEWER, 'Loaded High Poly', 'story-viewer', { state: 'loaded', format: 'glb' }],
  [VIEWER, 'Light Background', 'story-viewer', { state: 'loaded', format: 'glb', background: 'light' }],
  [VIEWER, 'Obj Mesh', 'story-viewer', { state: 'loaded', format: 'obj' }],
  [VIEWER, 'Loading', 'story-viewer', { state: 'downloading' }],
  [VIEWER, 'Mesh Not Found', 'story-viewer', { state: 'error', reason: 'load', retry: true }],
  [VIEWER, 'Corrupt Glb', 'story-viewer', { state: 'error', reason: 'parse', retry: false }],
  [VIEWER, 'Unknown Format', 'story-viewer', { state: 'error', reason: 'format', retry: false }],
];

test('every 3D story is checked here', () => {
  // A story added to either file without a line above would go unchecked.
  const all = [...byName.keys()].filter((k) => k.startsWith(`${PREVIEW}/`) || k.startsWith(`${VIEWER}/`)).sort();
  expect(all).toEqual(STORIES.map(([t, n]) => `${t}/${n}`).sort());
});

function errorsOf(page: Page, allow404: boolean): string[] {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
  page.on('console', (m) => {
    if (m.type() !== 'error') return;
    if (allow404 && /status of 404/.test(m.text())) return;
    errors.push(`console: ${m.text()}`);
  });
  return errors;
}

for (const [title, name, testId, want] of STORIES) {
  test(`${title} / ${name}`, async ({ page }) => {
    const id = byName.get(`${title}/${name}`);
    expect(id, 'the story exists').toBeTruthy();
    const errors = errorsOf(page, want.state === 'error' && want.reason === 'load');
    await page.goto(`iframe.html?id=${id}&viewMode=story`);
    const viewer = page.locator('[data-viewer]').first();
    if (want.state === 'loaded') {
      await expect(viewer).toHaveAttribute('data-state', 'loaded', { timeout: 30_000 });
      await expect(viewer).toHaveAttribute('data-format', want.format);
      expect(Number(await viewer.getAttribute('data-coverage')), 'the model covers part of the frame').toBeGreaterThanOrEqual(0.01);
      if (want.background) await expect(viewer).toHaveClass(new RegExp(`mv--${want.background}`));
      if (title === PREVIEW) {
        // The inspector's controls work on a drawn model.
        await expect(page.getByTestId(`${testId}-autorotate`)).toBeEnabled();
        await expect(page.getByTestId(`${testId}-stat-format`).locator('dd')).toHaveText(want.format.toUpperCase());
      }
    } else if (want.state === 'error') {
      await expect(viewer).toHaveAttribute('data-state', 'error', { timeout: 30_000 });
      await expect(viewer).toHaveAttribute('data-reason', want.reason);
      await expect(page.getByTestId(`${testId === 'story-preview' ? 'story-preview-viewer' : testId}-fallback-link`)).toHaveAttribute('href', /.+/);
      await expect(viewer.getByRole('button', { name: 'Retry' })).toHaveCount(want.retry ? 1 : 0);
      if (title === PREVIEW) {
        // Viewer-only controls disable; copying the URL still works.
        await expect(page.getByTestId(`${testId}-autorotate`)).toBeDisabled();
        await expect(page.getByTestId(`${testId}-copy-url`)).toBeEnabled();
      }
    } else if (want.state === 'downloading') {
      await expect(viewer.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '40', { timeout: 30_000 });
      await expect(viewer).toHaveAttribute('data-state', 'loading');
      await expect(viewer.locator('img.mv-poster')).toBeVisible();
    } else {
      await expect(page.getByTestId(`${testId}-error`)).toContainText(want.message, { timeout: 30_000 });
      await expect(page.locator('[data-viewer]')).toHaveCount(0);
    }
    expect(errors, errors.join('\n')).toEqual([]);
  });
}

// A deploy deletes the old chunks, and the server answers a missing one with
// index.html: a tab left open across it then fails the 3D viewer's lazy
// import the first time it shows a result. That must leave the dashboard
// standing (no uncaught error unmounting it), with the mesh downloadable.
const BLOCKED_CHUNKS: Array<[string, string, string, RegExp]> = [
  [PREVIEW, 'Full Asset Set', 'story-preview-viewer', /\/ModelPreviewBody-[^/]*\.js$/],
  [VIEWER, 'Loaded', 'story-viewer', /\/src-[^/]*\.js$/],
];

for (const [title, name, testId, chunk] of BLOCKED_CHUNKS) {
  test(`${title} / ${name}, its 3D chunk gone after a deploy`, async ({ page }) => {
    const id = byName.get(`${title}/${name}`);
    expect(id, 'the story exists').toBeTruthy();
    const uncaught: string[] = [];
    page.on('pageerror', (e) => uncaught.push(e.message));
    await page.route(chunk, (route) => route.abort());
    await page.goto(`iframe.html?id=${id}&viewMode=story`);
    const fallback = page.getByTestId(`${testId}-fallback`);
    await expect(fallback).toBeVisible({ timeout: 30_000 });
    await expect(fallback).toHaveAttribute('data-reason', 'import');
    await expect(fallback).toContainText(/reload the page/i);
    await expect(fallback.getByRole('link', { name: /Download/ })).toHaveAttribute('href', /cube\.glb$/);
    expect(uncaught, uncaught.join('\n')).toEqual([]);
  });
}
