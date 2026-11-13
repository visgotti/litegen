import { LiteGenClient, type HealthResponse, type LiteGenClientOptions } from '@litegen/sdk';

/**
 * `GET /health` as the server answers it: 200 when every provider checked
 * healthy, and 503 with the same body (`status: "degraded"`, each provider's
 * answer) when one did not. The SDK throws on any non-2xx, and its error keeps
 * no body, but it hands the parsed body to `onError` first: a 503 carrying a
 * health body is read from there. Anything else (a tenant's 403, a 503 from
 * something in front of the gateway, a 5xx, the network) still throws. The
 * caller's own `onError` sees every refusal, as before.
 */
export async function readHealth(options: LiteGenClientOptions, signal?: AbortSignal): Promise<HealthResponse> {
  const refused: { status?: number; body?: unknown } = {};
  const reader = new LiteGenClient({
    ...options,
    onError: (status, body) => {
      refused.status = status;
      refused.body = body;
      options.onError?.(status, body);
    },
  });
  try {
    return await reader.health.check(signal);
  } catch (err) {
    if (refused.status === 503 && isHealthBody(refused.body)) return refused.body;
    throw err;
  }
}

/** A `/health` body: what the Health page reads from it is there. */
function isHealthBody(body: unknown): body is HealthResponse {
  const b = body as Partial<HealthResponse> | null | undefined;
  return typeof b?.status === 'string' && Array.isArray(b.providers) && typeof b.cache === 'object' && b.cache !== null;
}
