/**
 * Pure helpers behind the reference-image picker, kept out of the component so
 * they are unit-testable without a DOM and so the component module exports
 * only components (react-refresh requires that for hot reload).
 */
import type { RefEntry } from './types';

/**
 * Upload caps. The generation endpoints buffer a request body of at most
 * 25 MiB (`axum::body::to_bytes` in the validated extractors), and an uploaded
 * image travels inside that body as base64 — 4/3 its size. These leave room
 * for the encoding and the rest of the request; anything bigger should be a URL,
 * which the server fetches itself (up to 20 MiB per image).
 */
export const MAX_UPLOAD_BYTES = 10 * 1024 * 1024;
export const MAX_INLINE_TOTAL_BYTES = 18 * 1024 * 1024;

/** Whether `s` is a URL the server can fetch: absolute http(s) only. */
export function isFetchableUrl(s: string): boolean {
  try {
    const u = new URL(s);
    return u.protocol === 'http:' || u.protocol === 'https:';
  } catch {
    return false;
  }
}

/** `10.0 MB` — for limits and file sizes in the picker's messages. */
export function formatMiB(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Raw bytes the inline (base64) entries carry: a data URL's payload is 4/3
 *  the size of the bytes it encodes. */
/**
 * The references in a role the selected models take. A model switch can leave
 * others behind; they are never sent (`refsForModel`), so they mustn't count
 * toward the new model's caps either, or they'd block its uploads unseen.
 */
export function refsOfRoles(refs: RefEntry[], roles: ReadonlyArray<{ role: string }>): RefEntry[] {
  return refs.filter(x => roles.some(r => r.role === x.role));
}

export function inlineBytes(refs: RefEntry[]): number {
  let total = 0;
  for (const r of refs) {
    if (r.type !== 'base64') continue;
    const comma = r.value.indexOf(',');
    const payload = comma === -1 ? r.value.length : r.value.length - comma - 1;
    total += Math.floor((payload * 3) / 4);
  }
  return total;
}

/** A file as a `data:` URL — the form the API accepts for `type: "base64"`
 *  (it strips the prefix itself, and uses its mime type when the provider
 *  needs one). */
export function readAsDataUrl(file: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => (typeof reader.result === 'string' ? resolve(reader.result) : reject(new Error('unreadable')));
    reader.onerror = () => reject(reader.error ?? new Error('unreadable'));
    reader.readAsDataURL(file);
  });
}

/**
 * A request body fit for localStorage: inline (base64) reference images are
 * removed, URL ones kept. History is capped by entry count, not size, and the
 * origin's quota is ~5 MB, so a single uploaded photo would make the save throw
 * — after the generation it records had already succeeded and been billed.
 * Re-running such an entry asks for the image again rather than failing on a
 * mangled one.
 */
export function withoutInlineRefs(body: Record<string, unknown>): Record<string, unknown> {
  let rest = body;
  // A segmentation's `image` is always the image itself.
  if (inlineImage(body)) {
    rest = { ...rest };
    delete rest.image;
  }
  const refs = body.reference_images;
  if (!Array.isArray(refs)) return rest;
  const kept = refs.filter(r => (r as { type?: unknown } | null)?.type !== 'base64');
  if (kept.length === refs.length) return rest;
  rest = { ...rest };
  if (kept.length) rest.reference_images = kept;
  else delete rest.reference_images;
  return rest;
}

/** Whether `body` carries a segmentation's inline `image`. */
function inlineImage(body: Record<string, unknown>): boolean {
  return typeof body.image === 'string' && !isFetchableUrl(body.image);
}

/** How many inline (base64) reference images `withoutInlineRefs` would remove
 *  from `body` — what a history entry must ask for again before re-running. */
export function inlineRefCount(body: Record<string, unknown>): number {
  const refs = body.reference_images;
  const inline = Array.isArray(refs)
    ? refs.filter(r => (r as { type?: unknown } | null)?.type === 'base64').length
    : 0;
  return inline + (inlineImage(body) ? 1 : 0);
}

/** A segmentation answer without its mask PNGs, which are as large as the
 *  image: what history keeps (the boxes and scores). */
export function withoutInlineMasks(response: Record<string, unknown>): Record<string, unknown> {
  const masks = response.masks;
  if (!Array.isArray(masks)) return response;
  return {
    ...response,
    masks: masks.map(m => {
      if (!m || typeof m !== 'object') return m;
      const kept = { ...(m as Record<string, unknown>) };
      delete kept.png_base64;
      return kept;
    }),
  };
}
