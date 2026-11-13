import { describe, it, expect } from 'vitest';
import { safeNext } from './safe-next';

const ORIGIN = 'https://app.litegen.ai';

// What a browser does with each value once it is a navigation target: a
// result that leaves the origin is an open redirect after a genuine sign-in.
const offOrigin = (target: string) => new URL(target, ORIGIN).origin !== ORIGIN;

describe('safeNext', () => {
  it('keeps a path on this origin, with its query and fragment', () => {
    for (const path of ['/', '/keys', '/generations?id=g1&tab=files', '/models#image', '/orgs/o1/members']) {
      expect(safeNext(path, ORIGIN)).toBe(path);
    }
  });

  it('falls back to / with no value', () => {
    for (const raw of [null, undefined, '']) {
      expect(safeNext(raw, ORIGIN)).toBe('/');
    }
  });

  it('refuses a value the browser resolves to another host', () => {
    const attacks = [
      'https://evil.example',
      '//evil.example',
      '/\\evil.example', // backslash: browsers read `\` as `/`
      '/\\/evil.example',
      '/\t/evil.example', // tab: browsers strip it, leaving `//evil.example`
      '/\n/evil.example', // line feed: stripped the same way
      '/\r/evil.example', // carriage return: stripped the same way
      '\t//evil.example',
      ' //evil.example',
      'javascript:alert(1)',
    ];
    for (const raw of attacks) {
      expect(safeNext(raw, ORIGIN), JSON.stringify(raw)).toBe('/');
    }
    // The values above that start with `/` really do escape when used as-is.
    expect(offOrigin('/\\evil.example')).toBe(true);
    expect(offOrigin('/\t/evil.example')).toBe(true);
  });

  it('refuses control characters, space and DEL anywhere', () => {
    for (const raw of ['/keys\u0000', '/a b', '/keys\u007f', '/keys\u001b[0m', '/x\u000b']) {
      expect(safeNext(raw, ORIGIN), JSON.stringify(raw)).toBe('/');
    }
  });

  it('never returns a sign-in page, which would loop', () => {
    for (const raw of ['/login', '/signup', '/login/', '/signup?next=/keys', '/login#x']) {
      expect(safeNext(raw, ORIGIN), raw).toBe('/');
    }
  });

  it('knows a sign-in page in any case or encoding, as the router matches it', () => {
    // react-router matches routes case-insensitively, after decoding.
    for (const raw of ['/LOGIN', '/Login', '/SignUp', '/SIGNUP/', '/%6Cogin', '/log%69n', '/sign%75p', '/%6C%6F%67%69%6E', '/./login', '/x/../login', '/login//']) {
      expect(safeNext(raw, ORIGIN), raw).toBe('/');
    }
  });

  it('keeps a path that only looks like a sign-in page', () => {
    for (const raw of ['/login-help', '/x/login', '/signups', '/loginx', '/oauth/login', '/apps/signup/']) {
      expect(safeNext(raw, ORIGIN), raw).toBe(raw);
    }
  });

  it('keeps a path with a malformed escape rather than throwing', () => {
    // Browsers leave a stray `%` in the path as it is.
    expect(safeNext('/keys%zz', ORIGIN)).toBe('/keys%zz');
    expect(safeNext('/%', ORIGIN)).toBe('/%');
  });

  it('returns the normalized path', () => {
    expect(safeNext('/a/../b', ORIGIN)).toBe('/b');
    expect(safeNext('/a/./b?x=1#y', ORIGIN)).toBe('/a/b?x=1#y');
  });

  it('refuses a dot segment that normalizes to a `//` path', () => {
    for (const raw of ['/.//evil.example', '/..//evil.example', '/%2e//evil.example', '/%2e%2e//evil.example', '/a/..//evil.example', '/a/b/../..//evil.example']) {
      expect(safeNext(raw, ORIGIN), raw).toBe('/');
    }
  });

  it('stays linear on a long run of slashes', () => {
    // A crafted link must not freeze the tab: the old trailing-slash regex
    // was quadratic in a run of slashes that doesn't end the path.
    const raw = '/a' + '/'.repeat(100_000) + 'b';
    const started = performance.now();
    const got = safeNext(raw, ORIGIN);
    expect(performance.now() - started).toBeLessThan(500);
    expect(offOrigin(got)).toBe(false);
  });

  it('only ever returns a same-origin path', () => {
    for (const raw of ['/keys', '/%2F/evil.example', '/a/../b', '/.//evil.example']) {
      const got = safeNext(raw, ORIGIN);
      expect(got.startsWith('/') && !got.startsWith('//'), `${raw} -> ${got}`).toBe(true);
      expect(offOrigin(got), `${raw} -> ${got}`).toBe(false);
    }
  });
});
