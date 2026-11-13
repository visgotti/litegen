import { describe, it, expect } from 'vitest';
import type { AccessSettings } from '@litegen/sdk';
import { accessBody, describeAccess, type AccessForm } from './access';

/** Every field left on "use the next layer". */
const BLANK: AccessForm = { mode: '', ownerScope: '', ttl: '', keepOutputs: '', retentionDays: '', signedLinks: '' };

/** Settings with keeping, retention and links at their defaults. */
const KEPT = { keep_outputs: true, retention_days: 0, signed_links: 'store' } as const;

describe('accessBody', () => {
  it('sends only what is set; blank fields use the next layer', () => {
    expect(accessBody(BLANK)).toEqual({ body: {} });
    expect(accessBody({ ...BLANK, mode: 'signed', ttl: ' 600 ' })).toEqual({ body: { mode: 'signed', signed_url_ttl_seconds: 600 } });
    expect(accessBody({ ...BLANK, ownerScope: 'org' })).toEqual({ body: { owner_scope: 'org' } });
  });

  it('refuses a link life outside a minute to a week, or not a whole number', () => {
    for (const ttl of ['59', '604801', '1.5', 'soon', '-60']) {
      expect(accessBody({ ...BLANK, mode: 'signed', ttl }).error, ttl).toMatch(/60 and 604800/);
    }
    expect(accessBody({ ...BLANK, ttl: '60' }).body).toEqual({ signed_url_ttl_seconds: 60 });
    expect(accessBody({ ...BLANK, ttl: '604800' }).body).toEqual({ signed_url_ttl_seconds: 604800 });
  });

  it('sends keeping, retention and who signs links when set', () => {
    expect(accessBody({ ...BLANK, keepOutputs: 'false', retentionDays: ' 30 ', signedLinks: 'litegen' })).toEqual({
      body: { keep_outputs: false, retention_days: 30, signed_links: 'litegen' },
    });
    expect(accessBody({ ...BLANK, keepOutputs: 'true', retentionDays: '0' })).toEqual({ body: { keep_outputs: true, retention_days: 0 } });
  });

  it('refuses a retention that is not 0 to 36500 whole days', () => {
    for (const days of ['-1', '36501', '1.5', 'week']) {
      expect(accessBody({ ...BLANK, retentionDays: days }).error, days).toMatch(/0 and 36500/);
    }
  });
});

describe('describeAccess', () => {
  const s = (over: Partial<AccessSettings>): AccessSettings => ({ mode: 'public', owner_scope: 'key', signed_url_ttl_seconds: 3600, ...KEPT, ...over });

  it('says how links work and who may open them', () => {
    expect(describeAccess(s({}))).toBe('public links · file route open to the key that made the file');
    expect(describeAccess(s({ mode: 'signed', owner_scope: 'app', signed_url_ttl_seconds: 600 }))).toBe('signed links for 10 min · open to any key of the app');
    expect(describeAccess(s({ mode: 'signed', owner_scope: 'org', signed_url_ttl_seconds: 604800 }))).toBe('signed links for 7 days · open to any key of the org');
    expect(describeAccess(s({ mode: 'signed', signed_url_ttl_seconds: 90 }))).toBe('signed links for 90 s · open to the key that made the file');
    expect(describeAccess(s({ mode: 'private' }))).toBe('private: the file route only · open to the key that made the file');
  });

  it('says when nothing is kept, how long kept files stay, and when litegen signs', () => {
    expect(describeAccess(s({ keep_outputs: false }))).toBe('keeps nothing · public links · file route open to the key that made the file');
    expect(describeAccess(s({ retention_days: 30 }))).toBe('public links · file route open to the key that made the file · deleted after 30 days');
    expect(describeAccess(s({ retention_days: 1 }))).toMatch(/deleted after 1 day$/);
    expect(describeAccess(s({ mode: 'signed', signed_links: 'litegen' }))).toBe('signed links by litegen for 1 h · open to the key that made the file');
  });
});
