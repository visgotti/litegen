import { describe, expect, it, vi } from 'vitest';
import { LiteGenAPIError, type HealthResponse } from '@litegen/sdk';
import { readHealth } from './health-read';

// `GET /health` answers 200 when every provider checked healthy and 503 with
// the same body when one did not (FU-HEALTH503; the binary's own 503, F3's
// probe: openai on a closed port). The page shows that body either way;
// anything else stays an error.

const DEGRADED: HealthResponse = {
  status: 'degraded',
  providers: [
    {
      provider: 'openai', modality: 'image', healthy: false, latency_ms: 1, last_checked: '2026-11-13T15:51:25.003609Z',
      message: 'OpenAI health check failed: error sending request for url (http://127.0.0.1:9/models)',
    },
    { provider: 'mock', modality: 'image', healthy: true, message: 'Mock provider ready', latency_ms: 0, last_checked: '2026-11-13T15:51:25.003609Z' },
  ],
  cache: { enabled: false, entries: 0 },
};
const HEALTHY: HealthResponse = { ...DEGRADED, status: 'healthy', providers: [DEGRADED.providers[1]] };

/** Options whose fetch answers `status` with `body` (JSON unless a string), and records each URL asked. */
function answering(status: number, body: unknown) {
  const asked: string[] = [];
  const onError = vi.fn();
  const fetch = async (input: RequestInfo | URL) => {
    asked.push(String(input));
    const text = typeof body === 'string' ? body : JSON.stringify(body);
    return new Response(text, { status, headers: { 'Content-Type': typeof body === 'string' ? 'text/html' : 'application/json' } });
  };
  return { options: { baseUrl: 'http://gw.test', fetch, onError }, asked, onError };
}

describe('reading /health', () => {
  it('takes the body of a 503 (a provider down) as the answer, not an error', async () => {
    const { options, asked } = answering(503, DEGRADED);
    await expect(readHealth(options)).resolves.toEqual(DEGRADED);
    expect(asked).toEqual(['http://gw.test/health']);
  });

  it('takes a 200 as the answer, as before', async () => {
    await expect(readHealth(answering(200, HEALTHY).options)).resolves.toEqual(HEALTHY);
  });

  it("still throws a tenant's 403, with its status (the page's operator note)", async () => {
    const refusal = { error: { message: 'Provider health covers the gateway’s own vendor keys', type: 'forbidden', code: 'forbidden' } };
    const err = await readHealth(answering(403, refusal).options).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(LiteGenAPIError);
    expect((err as LiteGenAPIError).status).toBe(403);
  });

  it('still throws a 503 without a health body (something in front of the gateway)', async () => {
    for (const body of ['<html>503 Service Temporarily Unavailable</html>', { error: { message: 'busy', type: 'server_error', code: 'server_at_capacity' } }]) {
      const err = await readHealth(answering(503, body).options).catch((e: unknown) => e);
      expect(err, JSON.stringify(body)).toBeInstanceOf(LiteGenAPIError);
      expect((err as LiteGenAPIError).status).toBe(503);
    }
  });

  it('still throws any other failure', async () => {
    const err = await readHealth(answering(500, { error: { message: 'boom', type: 'server_error', code: 'internal' } }).options).catch((e: unknown) => e);
    expect((err as LiteGenAPIError).status).toBe(500);
  });

  it("hands every refusal to the caller's onError too (a 401 signs out)", async () => {
    const a = answering(503, DEGRADED);
    await readHealth(a.options);
    expect(a.onError).toHaveBeenCalledWith(503, DEGRADED);
    const b = answering(401, { error: { message: 'Not logged in', type: 'unauthorized', code: 'not_authenticated' } });
    await readHealth(b.options).catch(() => {});
    expect(b.onError).toHaveBeenCalledWith(401, expect.anything());
  });
});
