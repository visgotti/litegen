import { describe, it, expect } from 'vitest';
import { runtimeBody, runtimeFormOf, type RuntimeForm } from './gateway-settings';

const blank: RuntimeForm = {
  cacheEnabled: '', cacheTtl: '', cacheMaxItems: '',
  retries: '', backoffMs: '', maxBackoffMs: '',
  vendorTimeout: '', breakerThreshold: '', breakerOpenFor: '',
};

describe('runtimeBody', () => {
  it('sends every field, blank ones as null so they follow the config', () => {
    expect(runtimeBody(blank)).toEqual({
      body: {
        cache_enabled: null, cache_ttl_seconds: null, cache_max_items: null,
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
});

describe('runtimeFormOf', () => {
  it('shows what is stored, and blanks for what follows the config', () => {
    expect(runtimeFormOf({ cache_enabled: false, retry_backoff_ms: 250 })).toEqual({ ...blank, cacheEnabled: 'off', backoffMs: '250' });
    expect(runtimeFormOf({})).toEqual(blank);
  });
});
