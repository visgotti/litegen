// The cache, retry, timeout and breaker settings on the Gateway page
// (docs/gateway-settings.md): the form and its checks, kept free of React so
// they are unit-tested. Every range is the server's, read from the settings it
// serves (`GatewaySettingsView.limits`); a rule between two settings
// (`retry_max_backoff_ms` at least `retry_backoff_ms`) is the server's to say,
// in its 400.
import type { GatewaySettingLimits, GatewaySettings, GatewaySettingsView, SettingRange } from '@litegen/sdk';

/** The form as typed: `''` in any field follows `litegen.yaml`. */
export interface RuntimeForm {
  cacheEnabled: '' | 'on' | 'off';
  /** `cache_charge_hits`: whether a hit is charged the cached answer's price. */
  cacheChargeHits: '' | 'on' | 'off';
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
  | 'cache_enabled' | 'cache_charge_hits' | 'cache_ttl_seconds' | 'cache_max_items' | 'cache_max_bytes'
  | 'retry_max_retries' | 'retry_backoff_ms' | 'retry_max_backoff_ms'
  | 'vendor_timeout_seconds' | 'circuit_breaker_threshold' | 'circuit_breaker_open_for_seconds'
>;

const within = (range: SettingRange | undefined) => (range ? ` from ${range.min} to ${range.max}` : '');

/** A whole number (the API's type for the setting) in its served range, `null`
 *  when blank, or why it won't do. Without a served range it goes as typed. */
function whole(value: string, name: string, range: SettingRange | undefined): number | null | string {
  const v = value.trim();
  if (v === '') return null;
  const n = Number(v);
  if (!Number.isInteger(n) || (range && (n < range.min || n > range.max))) return `${name} must be a whole number${within(range)}.`;
  return n;
}

/** A number in its served range, `null` when blank, or why it won't do. NaN
 *  would serialize as null and silently clear the field, so it is refused. */
export function numberIn(value: string, name: string, range: SettingRange | undefined): number | null | string {
  const v = value.trim();
  if (v === '') return null;
  const n = Number(v);
  if (!Number.isFinite(n) || (range && (n < range.min || n > range.max))) return `${name} must be a number${within(range)}.`;
  return n;
}

/** The fields to send, every one (`PUT /v1/admin/settings` clears what it isn't sent), or why not. */
export function runtimeBody(
  f: RuntimeForm,
  limits?: Partial<GatewaySettingLimits>,
): { body?: Required<{ [K in keyof RuntimeFields]: RuntimeFields[K] }>; error?: string } {
  const l = limits ?? {};
  const checks = {
    cache_ttl_seconds: whole(f.cacheTtl, 'Cache TTL (seconds)', l.cache_ttl_seconds),
    cache_max_items: whole(f.cacheMaxItems, 'Cache size', l.cache_max_items),
    cache_max_bytes: whole(f.cacheMaxBytes, 'Cache memory (bytes)', l.cache_max_bytes),
    retry_max_retries: whole(f.retries, 'Retries', l.retry_max_retries),
    retry_backoff_ms: whole(f.backoffMs, 'First backoff (ms)', l.retry_backoff_ms),
    retry_max_backoff_ms: whole(f.maxBackoffMs, 'Longest backoff (ms)', l.retry_max_backoff_ms),
    vendor_timeout_seconds: whole(f.vendorTimeout, 'Vendor timeout (seconds)', l.vendor_timeout_seconds),
    circuit_breaker_threshold: whole(f.breakerThreshold, 'Breaker threshold', l.circuit_breaker_threshold),
    circuit_breaker_open_for_seconds: whole(f.breakerOpenFor, 'Breaker open time (seconds)', l.circuit_breaker_open_for_seconds),
  };
  for (const v of Object.values(checks)) {
    if (typeof v === 'string') return { error: v };
  }
  return {
    body: {
      cache_enabled: f.cacheEnabled === '' ? null : f.cacheEnabled === 'on',
      cache_charge_hits: f.cacheChargeHits === '' ? null : f.cacheChargeHits === 'on',
      ...(checks as Record<keyof typeof checks, number | null>),
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

/** The `encrypt_provider_credentials` select: `'on'`, or `''` for off (the default). */
export function sealFormOf(s: Partial<GatewaySettings>): '' | 'on' {
  return s.encrypt_provider_credentials === true ? 'on' : '';
}

/** What to send for it: unchanged, what is stored; switched on, `true`;
 *  switched off, `null` (the default, off). */
export function sealValue(form: '' | 'on', stored: boolean | null | undefined): boolean | null {
  if (form === sealFormOf({ encrypt_provider_credentials: stored })) return stored ?? null;
  return form === 'on' ? true : null;
}

const text = (v: number | null | undefined) => (v === null || v === undefined ? '' : String(v));

/** A byte count in MiB: `256 MiB`, `1.5 MiB`. */
const mib = (bytes: number) => `${(bytes / 1_048_576).toLocaleString('en-US', { maximumFractionDigits: 1 })} MiB`;

/** The cache in force, as the server serves it, and what it holds now. */
export function cacheSummary(c: GatewaySettingsView['effective_cache'], entries: number): string {
  return `${c.enabled ? 'on' : 'off'} · answers kept ${c.ttl_seconds} s · up to ${c.max_items} in ${mib(c.max_bytes)}`
    + ` · hits ${c.charge_hits ? 'charged' : 'free'} · ${entries} cached now`;
}

/** The form for what the gateway stores. */
export function runtimeFormOf(s: Partial<GatewaySettings>): RuntimeForm {
  return {
    cacheEnabled: s.cache_enabled === true ? 'on' : s.cache_enabled === false ? 'off' : '',
    cacheChargeHits: s.cache_charge_hits === true ? 'on' : s.cache_charge_hits === false ? 'off' : '',
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
