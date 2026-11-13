// The cache, retry, timeout and breaker settings on the Gateway page
// (docs/gateway-settings.md): the form and its checks, kept free of React so
// they are unit-tested. The ranges are the gateway's own; the cache's memory
// bound is sent as typed, and the server's 400 says its range.
import type { GatewaySettings } from '@litegen/sdk';

/** The form as typed: `''` in any field follows `litegen.yaml`. */
export interface RuntimeForm {
  cacheEnabled: '' | 'on' | 'off';
  cacheTtl: string;
  cacheMaxItems: string;
  /** `cache_max_bytes`: about how much memory the cached answers may take, in bytes. */
  cacheMaxBytes: string;
  retries: string;
  backoffMs: string;
  maxBackoffMs: string;
  vendorTimeout: string;
  breakerThreshold: string;
  breakerOpenFor: string;
}

type RuntimeFields = Pick<
  GatewaySettings,
  | 'cache_enabled' | 'cache_ttl_seconds' | 'cache_max_items' | 'cache_max_bytes'
  | 'retry_max_retries' | 'retry_backoff_ms' | 'retry_max_backoff_ms'
  | 'vendor_timeout_seconds' | 'circuit_breaker_threshold' | 'circuit_breaker_open_for_seconds'
>;

/** A whole number in `lo..=hi`, `null` when blank, or why it won't do. */
function whole(value: string, name: string, lo: number, hi: number): number | null | string {
  const v = value.trim();
  if (v === '') return null;
  const n = Number(v);
  if (!Number.isInteger(n) || n < lo || n > hi) return `${name} must be a whole number from ${lo} to ${hi}.`;
  return n;
}

/** A number as typed, `null` when blank, or why it is not a number. Its
 *  range is the server's to say (a 400 names it). */
function number(value: string, name: string): number | null | string {
  const v = value.trim();
  if (v === '') return null;
  const n = Number(v);
  return Number.isFinite(n) ? n : `${name} must be a number.`;
}

/** The fields to send, every one (`PUT /v1/admin/settings` clears what it isn't sent), or why not. */
export function runtimeBody(f: RuntimeForm): { body?: Required<{ [K in keyof RuntimeFields]: RuntimeFields[K] }>; error?: string } {
  const checks = {
    cache_ttl_seconds: whole(f.cacheTtl, 'Cache TTL (seconds)', 1, 604_800),
    cache_max_items: whole(f.cacheMaxItems, 'Cache size', 1, 1_000_000),
    cache_max_bytes: number(f.cacheMaxBytes, 'Cache memory (bytes)'),
    retry_max_retries: whole(f.retries, 'Retries', 0, 5),
    retry_backoff_ms: whole(f.backoffMs, 'First backoff (ms)', 0, 60_000),
    retry_max_backoff_ms: whole(f.maxBackoffMs, 'Longest backoff (ms)', 0, 60_000),
    vendor_timeout_seconds: whole(f.vendorTimeout, 'Vendor timeout (seconds)', 5, 3600),
    circuit_breaker_threshold: whole(f.breakerThreshold, 'Breaker threshold', 1, 1000),
    circuit_breaker_open_for_seconds: whole(f.breakerOpenFor, 'Breaker open time (seconds)', 1, 3600),
  };
  for (const v of Object.values(checks)) {
    if (typeof v === 'string') return { error: v };
  }
  const n = checks as Record<keyof typeof checks, number | null>;
  if (n.retry_backoff_ms !== null && n.retry_max_backoff_ms !== null && n.retry_max_backoff_ms < n.retry_backoff_ms) {
    return { error: 'Longest backoff (ms) must be at least the first backoff.' };
  }
  return {
    body: {
      cache_enabled: f.cacheEnabled === '' ? null : f.cacheEnabled === 'on',
      ...n,
    },
  };
}

/**
 * The whole `PUT /v1/admin/settings` body: every field the server sent
 * (`stored`), with the form's edits over it. The PUT clears what it is not
 * sent, so a setting this page has no field for — one the server gained
 * after the page was written — survives a save (registry NEW 5).
 */
export function settingsBody(stored: Partial<GatewaySettings>, edits: Partial<GatewaySettings>): Partial<GatewaySettings> {
  return { ...stored, ...edits };
}

const text = (v: number | null | undefined) => (v === null || v === undefined ? '' : String(v));

/** The form for what the gateway stores. */
export function runtimeFormOf(s: Partial<GatewaySettings>): RuntimeForm {
  return {
    cacheEnabled: s.cache_enabled === true ? 'on' : s.cache_enabled === false ? 'off' : '',
    cacheTtl: text(s.cache_ttl_seconds),
    cacheMaxItems: text(s.cache_max_items),
    cacheMaxBytes: text(s.cache_max_bytes),
    retries: text(s.retry_max_retries),
    backoffMs: text(s.retry_backoff_ms),
    maxBackoffMs: text(s.retry_max_backoff_ms),
    vendorTimeout: text(s.vendor_timeout_seconds),
    breakerThreshold: text(s.circuit_breaker_threshold),
    breakerOpenFor: text(s.circuit_breaker_open_for_seconds),
  };
}
