import { describe, it, expect } from 'vitest';
import type { GatewaySettings } from '@litegen/sdk';
import { runtimeBody, runtimeFormOf, settingsBody, type RuntimeForm } from './gateway-settings';

const blank: RuntimeForm = {
  cacheEnabled: '', cacheTtl: '', cacheMaxItems: '', cacheMaxBytes: '',
  retries: '', backoffMs: '', maxBackoffMs: '',
  vendorTimeout: '', breakerThreshold: '', breakerOpenFor: '',
};

describe('runtimeBody', () => {
  it('sends every field, blank ones as null so they follow the config', () => {
    expect(runtimeBody(blank)).toEqual({
      body: {
        cache_enabled: null, cache_ttl_seconds: null, cache_max_items: null, cache_max_bytes: null,
        retry_max_retries: null, retry_backoff_ms: null, retry_max_backoff_ms: null,
        vendor_timeout_seconds: null, circuit_breaker_threshold: null, circuit_breaker_open_for_seconds: null,
      },
    });
    const set = runtimeBody({ ...blank, cacheEnabled: 'on', cacheTtl: ' 60 ', retries: '0', vendorTimeout: '30', breakerOpenFor: '5' });
    expect(set.body).toMatchObject({ cache_enabled: true, cache_ttl_seconds: 60, retry_max_retries: 0, vendor_timeout_seconds: 30, circuit_breaker_open_for_seconds: 5 });
    expect(runtimeBody({ ...blank, cacheEnabled: 'off' }).body?.cache_enabled).toBe(false);
  });

  it('refuses what the gateway would, naming the field', () => {
    for (const [form, says] of [
      [{ cacheTtl: '0' }, 'Cache TTL'],
      [{ cacheMaxItems: '1000001' }, 'Cache size'],
      [{ retries: '6' }, 'Retries'],
      [{ retries: '1.5' }, 'Retries'],
      [{ backoffMs: '60001' }, 'First backoff'],
      [{ backoffMs: '2000', maxBackoffMs: '1000' }, 'Longest backoff'],
      [{ vendorTimeout: '4' }, 'Vendor timeout'],
      [{ breakerThreshold: '0' }, 'Breaker threshold'],
      [{ breakerOpenFor: 'soon' }, 'Breaker open time'],
    ] as Array<[Partial<RuntimeForm>, string]>) {
      expect(runtimeBody({ ...blank, ...form }).error, JSON.stringify(form)).toContain(says);
    }
  });

  it("sends the cache's memory bound as typed: its range is the server's to say (registry NEW 5)", () => {
    expect(runtimeBody({ ...blank, cacheMaxBytes: '2097152' }).body?.cache_max_bytes).toBe(2_097_152);
    // Below the server's 1 MiB floor, yet sent: the server's 400 is what the page shows.
    expect(runtimeBody({ ...blank, cacheMaxBytes: '1024' })).toMatchObject({ body: { cache_max_bytes: 1024 } });
    expect(runtimeBody({ ...blank, cacheMaxBytes: 'lots' }).error).toContain('Cache memory');
  });
});

describe('runtimeFormOf', () => {
  it('shows what is stored, and blanks for what follows the config', () => {
    expect(runtimeFormOf({ cache_enabled: false, retry_backoff_ms: 250 })).toEqual({ ...blank, cacheEnabled: 'off', backoffMs: '250' });
    expect(runtimeFormOf({ cache_max_bytes: 2_097_152 })).toEqual({ ...blank, cacheMaxBytes: '2097152' });
    expect(runtimeFormOf({})).toEqual(blank);
  });
});

describe('settingsBody', () => {
  it('sends back every field the server gave, with the edits over it: a setting the page has no field for survives a save', () => {
    // PUT /v1/admin/settings clears what it is not sent (registry NEW 5).
    const stored = { cost_markup_percent: 10, cache_max_bytes: 2_097_152, a_later_setting: 'kept' } as unknown as GatewaySettings;
    expect(settingsBody(stored, { cost_markup_percent: 20, retry_max_retries: null }))
      .toEqual({ cost_markup_percent: 20, cache_max_bytes: 2_097_152, a_later_setting: 'kept', retry_max_retries: null });
  });
});
