import { LiteGenClient, type HealthResponse, type LiteGenClientOptions } from '@litegen/sdk';

/**
 * `GET /health` as the server answers it: 200 when every provider checked
 * healthy, and 503 with the same body (`status: "degraded"`, each provider's
 * answer) when one did not. The SDK's `health.check` answers both with the
 * report; anything else (a tenant's 403, a 503 from something in front of the
 * gateway, a 5xx, the network) still throws. The caller's own `onError` sees
 * every refusal, as before.
 */
export async function readHealth(options: LiteGenClientOptions, signal?: AbortSignal): Promise<HealthResponse> {
  return new LiteGenClient(options).health.check(signal);
}
