/**
 * The post-sign-in target from a `?next=` value, used by Login and Signup
 * both for their own redirect and for the `next` they hand the OAuth start.
 * It is a path on `origin` (the page's own, `window.location.origin`), else
 * `/`.
 *
 * Browsers resolve some values that look like paths to another host:
 * - `/\evil.example` and `/\/evil.example`, because they read `\` as `/`;
 * - `/<tab>/evil.example`, because they strip tab, line feed and carriage
 *   return before parsing, which leaves `//evil.example`.
 *
 * react-router can't push a URL on another origin, so it falls back to a full
 * `window.location.assign`, and a genuine sign-in would then end on an
 * attacker's page. So `\`, every control character, space and DEL are
 * refused outright. The value must also resolve on `origin`. The normalized
 * path is returned, never a `//` one, and never a sign-in page, since that
 * would loop. A sign-in page is recognized the way the router matches
 * routes: in any case, after decoding.
 *
 * Every step is linear in the value's length, so a crafted link can't
 * freeze the tab.
 */
export function safeNext(raw: string | null | undefined, origin: string): string {
  if (!raw || !raw.startsWith('/') || raw.startsWith('//')) return '/';
  for (let i = 0; i < raw.length; i++) {
    const c = raw.charCodeAt(i);
    if (c < 0x21 || c === 0x7f || c === 0x5c) return '/';
  }
  let url: URL;
  let base: URL;
  try {
    base = new URL(origin);
    url = new URL(raw, base);
  } catch {
    return '/';
  }
  if (url.origin !== base.origin || url.pathname.startsWith('//')) return '/';
  let page = url.pathname;
  try {
    page = decodeURIComponent(page);
  } catch {
    // Not valid percent-encoding: judge it as written.
  }
  // Anchored at both ends, so it runs in linear time.
  if (/^\/(?:login|signup)\/*$/i.test(page)) return '/';
  return url.pathname + url.search + url.hash;
}
