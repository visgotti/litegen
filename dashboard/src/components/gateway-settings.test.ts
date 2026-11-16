import { describe, it, expect } from 'vitest';
import type { GatewaySettingLimits, GatewaySettings } from '@litegen/sdk';
import { cacheSummary, numberIn, runtimeBody, runtimeFormOf, sealFormOf, sealValue, settingsBody, type RuntimeForm } from './gateway-settings';

const blank: RuntimeForm = {
  cacheEnabled: '', cacheChargeHits: '', cacheTtl: '', cacheMaxItems: '', cacheMaxBytes: '',
  retries: '', backoffMs: '', maxBackoffMs: '',
  vendorTimeout: '', breakerThreshold: '', breakerOpenFor: '',
};

/** The ranges as GET /v1/admin/settings serves them (`limits`, misc-server's GatewaySettingLimits). */
const LIMITS: GatewaySettingLimits = {
  cache_ttl_seconds: { min: 1, max: 604_800 },
  cache_max_items: { min: 1, max: 1_000_000 },
  cache_max_bytes: { min: 1_048_576, max: 68_719_476_736 },
  retry_max_retries: { min: 0, max: 5 },
  retry_backoff_ms: { min: 0, max: 60_000 },
  retry_max_backoff_ms: { min: 0, max: 60_000 },
  vendor_timeout_seconds: { min: 5, max: 3600 },
  circuit_breaker_threshold: { min: 1, max: 1000 },
  circuit_breaker_open_for_seconds: { min: 1, max: 3600 },
  cost_markup_percent: { min: 0, max: 1000 },
  storage_access_signed_url_ttl_seconds: { min: 60, max: 604_800 },
  storage_access_retention_days: { min: 0, max: 36_500 },
};

describe('runtimeBody', () => {
  it('sends every field, blank ones as null so they follow the config', () => {
    expect(runtimeBody(blank, LIMITS)).toEqual({
      body: {
        cache_enabled: null, cache_charge_hits: null, cache_ttl_seconds: null, cache_max_items: null, cache_max_bytes: null,
        retry_max_retries: null, retry_backoff_ms: null, retry_max_backoff_ms: null,
        vendor_timeout_seconds: null, circuit_breaker_threshold: null, circuit_breaker_open_for_seconds: null,
      },
    });
    const set = runtimeBody({ ...blank, cacheEnabled: 'on', cacheTtl: ' 60 ', retries: '0', vendorTimeout: '30', breakerOpenFor: '5' }, LIMITS);
    expect(set.body).toMatchObject({ cache_enabled: true, cache_ttl_seconds: 60, retry_max_retries: 0, vendor_timeout_seconds: 30, circuit_breaker_open_for_seconds: 5 });
    expect(runtimeBody({ ...blank, cacheEnabled: 'off' }, LIMITS).body?.cache_enabled).toBe(false);
  });

  it("refuses a value outside the server's served range, naming the field and the range", () => {
    for (const [form, says] of [
      [{ cacheTtl: '0' }, 'Cache TTL (seconds) must be a whole number from 1 to 604800'],
      [{ cacheMaxItems: '1000001' }, 'Cache size must be a whole number from 1 to 1000000'],
      [{ cacheMaxBytes: '1024' }, 'Cache memory (bytes) must be a whole number from 1048576 to 68719476736'],
      [{ retries: '6' }, 'Retries must be a whole number from 0 to 5'],
      [{ retries: '1.5' }, 'Retries must be a whole number from 0 to 5'],
      [{ backoffMs: '60001' }, 'First backoff (ms)'],
      [{ vendorTimeout: '4' }, 'Vendor timeout (seconds) must be a whole number from 5 to 3600'],
      [{ breakerThreshold: '0' }, 'Breaker threshold'],
      [{ breakerOpenFor: 'soon' }, 'Breaker open time'],
    ] as Array<[Partial<RuntimeForm>, string]>) {
      expect(runtimeBody({ ...blank, ...form }, LIMITS).error, JSON.stringify(form)).toContain(says);
    }
  });

  it('holds to the range the server serves, not a copy of it', () => {
    const narrow = { ...LIMITS, retry_max_retries: { min: 0, max: 3 } };
    expect(runtimeBody({ ...blank, retries: '4' }, narrow).error).toBe('Retries must be a whole number from 0 to 3.');
    expect(runtimeBody({ ...blank, retries: '4' }, LIMITS).body?.retry_max_retries).toBe(4);
  });

  it('leaves a rule between two settings to the server, which says it in its 400', () => {
    // `retry_max_backoff_ms` must be at least `retry_backoff_ms`: the server's rule.
    expect(runtimeBody({ ...blank, backoffMs: '2000', maxBackoffMs: '1000' }, LIMITS).body)
      .toMatchObject({ retry_backoff_ms: 2000, retry_max_backoff_ms: 1000 });
  });

  it('sends a whole number as typed where no range is served', () => {
    expect(runtimeBody({ ...blank, retries: '9', cacheMaxBytes: '1024' }).body)
      .toMatchObject({ retry_max_retries: 9, cache_max_bytes: 1024 });
    expect(runtimeBody({ ...blank, retries: 'lots' }).error).toContain('Retries must be a whole number');
  });
});

describe('charging cache hits', () => {
  // `cache_charge_hits`: whether a hit is charged the cached answer's price;
  // null follows the YAML (default true).
  it('sends the toggle as set, and null to follow the config', () => {
    expect(runtimeBody({ ...blank, cacheChargeHits: 'off' }, LIMITS).body?.cache_charge_hits).toBe(false);
    expect(runtimeBody({ ...blank, cacheChargeHits: 'on' }, LIMITS).body?.cache_charge_hits).toBe(true);
    expect(runtimeBody(blank, LIMITS).body?.cache_charge_hits).toBeNull();
  });

  it('shows what is stored', () => {
    expect(runtimeFormOf({ cache_charge_hits: false }).cacheChargeHits).toBe('off');
    expect(runtimeFormOf({ cache_charge_hits: true }).cacheChargeHits).toBe('on');
    expect(runtimeFormOf({}).cacheChargeHits).toBe('');
  });
});

describe('cacheSummary', () => {
  it('says the cache in force, whether hits are charged, and what it holds now', () => {
    expect(cacheSummary({ enabled: true, ttl_seconds: 600, max_items: 50, max_bytes: 2_097_152, charge_hits: true }, 3))
      .toBe('on · answers kept 600 s · up to 50 in 2 MiB · hits charged · 3 cached now');
    expect(cacheSummary({ enabled: false, ttl_seconds: 3600, max_items: 1000, max_bytes: 268_435_456, charge_hits: false }, 0))
      .toBe('off · answers kept 3600 s · up to 1000 in 256 MiB · hits free · 0 cached now');
  });
});

describe('numberIn', () => {
  it('takes a number in the served range, blank as null, and says why not', () => {
    const range = LIMITS.cost_markup_percent;
    expect(numberIn(' 12.5 ', 'Cost markup', range)).toBe(12.5);
    expect(numberIn('', 'Cost markup', range)).toBeNull();
    expect(numberIn('1500', 'Cost markup', range)).toBe('Cost markup must be a number from 0 to 1000.');
    expect(numberIn('abc', 'Cost markup', range)).toBe('Cost markup must be a number from 0 to 1000.');
    expect(numberIn('-5', 'Cost markup', undefined)).toBe(-5);
    expect(numberIn('abc', 'Default end-user budget', undefined)).toBe('Default end-user budget must be a number.');
  });
});

describe('the encrypt_provider_credentials select', () => {
  it('shows on only when the server stores true', () => {
    expect(sealFormOf({ encrypt_provider_credentials: true })).toBe('on');
    expect(sealFormOf({ encrypt_provider_credentials: false })).toBe('');
    expect(sealFormOf({})).toBe('');
  });

  it('sends what is stored while unchanged, true when switched on, and null (the default, off) when switched off', () => {
    expect(sealValue('', false)).toBe(false);
    expect(sealValue('', undefined)).toBeNull();
    expect(sealValue('on', true)).toBe(true);
    expect(sealValue('on', null)).toBe(true);
    expect(sealValue('', true)).toBeNull();
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
