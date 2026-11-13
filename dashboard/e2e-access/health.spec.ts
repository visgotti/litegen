import { test, expect } from '@playwright/test';
import { feature } from '../e2e/support';

// The Health page when a provider is down (FU-HEALTH503). `GET /health`
// answers 503 with the same body as its 200 (one entry per provider and
// modality, `status: degraded`), so the page shows that table in its
// not-healthy state rather than an error. The operator (the master key) sets
// openai up at runtime on a loopback port nothing listens on, so its check
// fails at once and no vendor is called; the spec takes it away again.
// A hosted tenant's 403 note is e2e-mt/roles.spec.ts's.

const BACKEND = 'http://127.0.0.1:5399';
const MASTER_KEY = 'access-e2e-master';
const auth = { authorization: `Bearer ${MASTER_KEY}` };

type Health = { status: string; providers: Array<{ provider: string; healthy: boolean; message?: string | null }> };

test("a provider that is down: the Health page shows /health's 503 table, not an error", async ({ page, request }) => {
  await feature('The operator sets openai up on a port nothing answers');
  const put = await request.put(`${BACKEND}/v1/admin/providers/openai`, {
    headers: auth,
    data: { api_key: 'sk-e2e-health-not-a-key', api_base: 'http://127.0.0.1:9' },
  });
  expect(put.status(), await put.text()).toBe(200);
  try {
    await feature('The Health page reads /health with the master key: 503, a provider down');
    await page.addInitScript(k => localStorage.setItem('litegen_api_key', k), MASTER_KEY);
    await page.goto('/');
    const answer = page.waitForResponse(r => new URL(r.url()).pathname === '/health');
    await page.getByRole('link', { name: 'Health', exact: true }).click();
    const res = await answer;
    expect(res.status(), 'a provider is down').toBe(503);
    const body = (await res.json()) as Health;
    expect(body.status).toBe('degraded');
    const down = body.providers.filter(p => !p.healthy);
    expect(down.map(p => p.provider)).toContain('openai');

    await expect(page.locator('.alert-error')).toHaveCount(0);
    await expect(page.getByTestId('health-overall')).toHaveText('degraded');
    await expect(page.getByTestId('health-overall')).toHaveClass(/\bred\b/);
    await expect(page.locator('table tbody tr')).toHaveCount(body.providers.length);
    await expect(page.locator('table .badge.unhealthy')).toHaveCount(down.length);
    await expect(page.locator('table .badge.healthy')).toHaveCount(body.providers.length - down.length);
    await expect(page.locator('table')).toContainText(down[0].message!);
    await expect(page.getByTestId('health-liveness-badge')).toContainText('alive');
    await expect(page.getByTestId('health-readiness-badge')).toContainText('ready');
    await expect(page.getByRole('button', { name: 'Clear Cache' })).toBeVisible();

    await feature('openai taken away: Refresh shows every provider healthy again');
    const removed = await request.delete(`${BACKEND}/v1/admin/providers/openai`, { headers: auth });
    expect(removed.status(), await removed.text()).toBe(200);
    const again = page.waitForResponse(r => new URL(r.url()).pathname === '/health');
    await page.getByRole('button', { name: 'Refresh' }).click();
    expect((await again).status()).toBe(200);
    await expect(page.getByTestId('health-overall')).toHaveText('healthy');
    await expect(page.getByTestId('health-overall')).toHaveClass(/\bgreen\b/);
    await expect(page.locator('table .badge.unhealthy')).toHaveCount(0);
  } finally {
    await request.delete(`${BACKEND}/v1/admin/providers/openai`, { headers: auth });
  }
});
