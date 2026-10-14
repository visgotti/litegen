import { describe, expect, it } from 'vitest';
import {
  MAX_INLINE_TOTAL_BYTES,
  MAX_UPLOAD_BYTES,
  formatMiB,
  inlineBytes,
  isFetchableUrl,
  inlineRefCount,
  withoutInlineRefs,
} from './ref-images';

describe('isFetchableUrl', () => {
  it('accepts absolute http(s) URLs', () => {
    expect(isFetchableUrl('https://img.test/chair.png')).toBe(true);
    expect(isFetchableUrl('http://img.test/chair.png?sig=abc')).toBe(true);
  });

  it('refuses what the server cannot fetch', () => {
    expect(isFetchableUrl('/relative/chair.png')).toBe(false);
    expect(isFetchableUrl('data:image/png;base64,iVBOR')).toBe(false);
    expect(isFetchableUrl('file:///etc/passwd')).toBe(false);
    expect(isFetchableUrl('javascript:alert(1)')).toBe(false);
    expect(isFetchableUrl('')).toBe(false);
  });
});

describe('inlineBytes', () => {
  it('counts the decoded size of inline images only', () => {
    // 8 base64 chars encode 6 bytes.
    const refs = [
      { id: 'a', role: 'init', type: 'base64' as const, value: 'data:image/png;base64,AAAAAAAA' },
      { id: 'b', role: 'init', type: 'url' as const, value: 'https://img.test/a.png' },
    ];
    expect(inlineBytes(refs)).toBe(6);
  });

  it('handles a bare base64 value with no data: prefix', () => {
    expect(inlineBytes([{ id: 'a', role: 'init', type: 'base64', value: 'AAAA' }])).toBe(3);
  });
});

describe('upload caps', () => {
  it('leave room for base64 inside the 25 MiB request body the server buffers', () => {
    expect(Math.ceil((MAX_INLINE_TOTAL_BYTES * 4) / 3)).toBeLessThan(25 * 1024 * 1024);
    expect(MAX_UPLOAD_BYTES).toBeLessThanOrEqual(MAX_INLINE_TOTAL_BYTES);
  });

  it('formats a size in MB', () => {
    expect(formatMiB(MAX_UPLOAD_BYTES)).toBe('10.0 MB');
  });
});

describe('withoutInlineRefs', () => {
  it('drops inline images and keeps URL ones', () => {
    const body = {
      model: 'fal/trellis',
      reference_images: [
        { type: 'base64', value: 'data:image/png;base64,AAAA', role: 'init' },
        { type: 'url', value: 'https://img.test/a.png', role: 'init' },
      ],
    };
    expect(withoutInlineRefs(body)).toEqual({
      model: 'fal/trellis',
      reference_images: [{ type: 'url', value: 'https://img.test/a.png', role: 'init' }],
    });
  });

  it('removes the field when every image was inline', () => {
    const out = withoutInlineRefs({ model: 'm', reference_images: [{ type: 'base64', value: 'AAAA', role: 'init' }] });
    expect(out).toEqual({ model: 'm' });
  });

  it('returns the same body when there is nothing to strip, without mutating it', () => {
    const body = { model: 'm', reference_images: [{ type: 'url', value: 'https://img.test/a.png' }] };
    expect(withoutInlineRefs(body)).toBe(body);
    const noRefs = { model: 'm' };
    expect(withoutInlineRefs(noRefs)).toBe(noRefs);
  });
});

describe('inlineRefCount', () => {
  // A history entry records this so a Re-submit asks for the stripped
  // uploads instead of running without them.
  it('counts the uploads withoutInlineRefs removes, not the URLs it keeps', () => {
    const body = {
      model: 'm',
      reference_images: [
        { type: 'base64', value: 'AAAA', role: 'init' },
        { type: 'url', value: 'https://x/y.png', role: 'style' },
        { type: 'base64', value: 'BBBB', role: 'view-back' },
      ],
    };
    expect(inlineRefCount(body)).toBe(2);
    expect(inlineRefCount(withoutInlineRefs(body))).toBe(0);
  });

  it('is zero without reference images', () => {
    expect(inlineRefCount({ model: 'm' })).toBe(0);
    expect(inlineRefCount({ model: 'm', reference_images: 'nope' })).toBe(0);
  });
});
