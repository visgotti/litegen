import { test, expect } from '@playwright/test';
import type { APIRequestContext } from '@playwright/test';

// Zip packages on a 3D generation (`output_packages: true`), against the real
// gateway binary and the mock textured model (models/mock.yaml). OBJ, DAE,
// USDA and glTF have files that go beside the mesh, so each gets a package;
// GLB never does. The dashboard only links the zips. Nothing here opens one.

const MASTER_KEY = process.env.PLAYWRIGHT_MASTER_KEY ?? 'test-master-key-please-rotate';
const API_BASE = process.env.LITEGEN_API_BASE ?? 'http://127.0.0.1:5099';
const authHeaders = { Authorization: `Bearer ${MASTER_KEY}` };

const MODEL = 'mock/textured-3d';
const JOB_TIMEOUT = 60_000;

async function createCompletedWithPackages(request: APIRequestContext): Promise<string> {
  const res = await request.post(`${API_BASE}/v1/models3d/generations`, {
    headers: authHeaders,
    data: {
      model: MODEL,
      prompt: 'a packaged lantern',
      output_formats: ['glb', 'obj', 'dae', 'usda', 'gltf'],
      output_packages: true,
    },
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

test('a mock generation with output_packages shows a zip link per package', async ({ page, request }) => {
  const id = await createCompletedWithPackages(request);

  // What the API delivered is the reference, so the test follows the server.
  const gen = (await (
    await request.get(`${API_BASE}/v1/models3d/${encodeURIComponent(id)}`, { headers: authHeaders })
  ).json()) as { assets?: Array<{ kind: string; format: string; url: string }> };
  const packages = (gen.assets ?? []).filter(a => a.kind === 'package');
  expect(packages.length, 'fixture: mock/textured-3d delivers packages').toBeGreaterThan(0);
  expect(packages.some(p => p.format === 'glb'), 'GLB never gets a package').toBeFalsy();

  await page.addInitScript((key) => localStorage.setItem('litegen_api_key', key), MASTER_KEY);
  await page.goto(`/logs?model=${encodeURIComponent(MODEL)}`);
  const row = page.getByTestId(`logs-row-${id}`);
  await expect(row).toBeVisible({ timeout: 30_000 });
  await row.click();
  const panel = page.getByTestId('trace-panel');
  await expect(panel).toBeVisible();
  await page.getByTestId('trace-tab-visual').click();

  const list = panel.getByTestId('generation-output-packages');
  await expect(list).toBeVisible({ timeout: JOB_TIMEOUT });
  const links = list.locator('a');
  await expect(links).toHaveCount(packages.length);
  for (const [i, p] of packages.entries()) {
    const link = links.nth(i);
    await expect(link).toHaveText(`Download ${p.format.toUpperCase()} package (.zip)`);
    await expect(link).toHaveAttribute('download', '');
    expect(await link.getAttribute('href')).toContain(new URL(p.url, API_BASE).pathname.split('/').pop()!);
  }

  // The link serves a zip.
  const first = await request.get(new URL(packages[0]!.url, API_BASE).toString(), { headers: authHeaders });
  expect(first.ok()).toBeTruthy();
  expect(first.headers()['content-type']).toContain('application/zip');
});
