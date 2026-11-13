/**
 * What a prebuild sync script does when it cannot read its source.
 *
 * By default it fails the build: a deploy must not ship generated files that
 * no longer match models/ or sdks/openapi.json. Set
 * LITEGEN_SYNC_KEEP_COMMITTED=1 to build with the committed files anyway (the
 * old fail-open behaviour, for an offline or partial checkout); the script then
 * warns, and CI's landing job fails on that warning.
 */
export const KEEP_COMMITTED = 'LITEGEN_SYNC_KEEP_COMMITTED';

/** Exits 1, or returns (after the warning) when the opt-out is set so the caller can keep its committed file. */
export function syncFailed(tag, msg, kept) {
  if (process.env[KEEP_COMMITTED] === '1') {
    console.warn(`[${tag}] warning: ${msg}; ${kept} (${KEEP_COMMITTED}=1)`);
    return;
  }
  console.error(`[${tag}] error: ${msg}`);
  console.error(`[${tag}] the committed file is left as it is; to build with it anyway, set ${KEEP_COMMITTED}=1`);
  process.exit(1);
}
