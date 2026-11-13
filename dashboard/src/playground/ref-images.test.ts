import { describe, expect, it } from 'vitest';
import {
  MAX_INLINE_TOTAL_BYTES,
  MAX_UPLOAD_BYTES,
  formatMiB,
  inlineBytes,
  isFetchableUrl,
  inlineRefCount,
  refsOfRoles,
  withoutInlineMasks,
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

describe('what a history entry keeps', () => {
  it('drops a segmentation run\'s inline image and mask PNGs, and counts the image to ask for again', () => {
    const body = { model: 'mock/segment-image', image: 'data:image/png;base64,iVBORw0KGgo=', prompts: [{ type: 'point', x: 1, y: 2 }] };
    expect(withoutInlineRefs(body)).toEqual({ model: 'mock/segment-image', prompts: [{ type: 'point', x: 1, y: 2 }] });
    expect(inlineRefCount(body)).toBe(1);
    const response = { masks: [{ png_base64: 'iVBORw0KGgo=', box: { x: 0, y: 0, w: 1, h: 1 }, score: 0.9 }] };
    expect(withoutInlineMasks(response)).toEqual({ masks: [{ box: { x: 0, y: 0, w: 1, h: 1 }, score: 0.9 }] });
  });
});

describe('a history write that cannot be stored', () => {
  it('never fails the run it records', async () => {
    const store = new Map<string, string>();
    const full = {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: () => { throw new DOMException('quota exceeded', 'QuotaExceededError'); },
      removeItem: (k: string) => { store.delete(k); },
    };
    const saved = (globalThis as { localStorage?: unknown }).localStorage;
    (globalThis as { localStorage?: unknown }).localStorage = full;
    try {
      const { pushPlaygroundHistory } = await import('../playground-history');
      expect(() => pushPlaygroundHistory({
        id: '1', model: 'm', prompt: 'p', timestamp: '2026-10-31T00:00:00Z', requestBody: {}, responseBody: {},
      })).not.toThrow();
    } finally {
      (globalThis as { localStorage?: unknown }).localStorage = saved;
    }
  });
});


describe('refsOfRoles', () => {
  it('leaves out references in a role the selected models no longer take', () => {
    // After a switch from an image-to-video model (first_frame) to an edit
    // model (init), the old first_frame upload is never sent, so it must not
    // count toward the new model's caps either.
    const refs = [
      { id: 'a', role: 'first_frame', type: 'base64' as const, value: 'data:image/png;base64,AAAA' },
      { id: 'b', role: 'init', type: 'url' as const, value: 'https://example.com/a.png' },
    ];
    expect(refsOfRoles(refs, [{ role: 'init' }]).map(r => r.id)).toEqual(['b']);
    expect(refsOfRoles(refs, [])).toEqual([]);
  });
});
