import { describe, expect, it } from 'vitest';
import { copyToClipboard, drawnNote, formatBytes, formatDimensions } from './format';

// The union of the dashboard's (dashboard/src/components/model3d-assets.test.ts)
// and the converter tool's (apps/3d-converter/src/format-helpers.test.ts)
// expectations, which agreed: both apps now read these from here.
describe('formatBytes', () => {
  it.each([
    [0, '0 B'],
    [1, '1 B'],
    [1023, '1023 B'],
    [1024, '1.0 KB'],
    [1536, '1.5 KB'],
    // Rounds up into the next unit rather than printing "1024.0 KB".
    [1024 * 1024 - 1, '1.0 MB'],
    [1.5 * 1024 * 1024, '1.5 MB'],
    [3 * 1024 ** 3, '3.0 GB'],
  ])('%d -> %s', (n, want) => {
    expect(formatBytes(n)).toBe(want);
  });

  it.each([null, undefined, -1, Number.NaN, Number.POSITIVE_INFINITY])('renders %s as a dash', (n) => {
    expect(formatBytes(n)).toBe('—');
  });
});

describe('formatDimensions', () => {
  it('formats a bounding box in metres, to two places', () => {
    expect(formatDimensions({ x: 1, y: 1, z: 1 })).toBe('1.00 × 1.00 × 1.00 m');
    expect(formatDimensions({ x: 2, y: 0.5, z: 1.25 })).toBe('2.00 × 0.50 × 1.25 m');
  });

  it('does not collapse a millimetre-scale model to zero', () => {
    expect(formatDimensions({ x: 0.0005, y: 0.001, z: 0 })).toBe('5.0e-4 × 1.0e-3 × 0.00 m');
  });

  it('renders an unknown box as a dash', () => {
    expect(formatDimensions(null)).toBe('—');
    expect(formatDimensions(undefined)).toBe('—');
    expect(formatDimensions({ x: Number.NaN, y: 1, z: 1 })).toBe('—');
  });
});

describe('drawnNote', () => {
  it('says when the loader drew fewer triangles than litegen reads', () => {
    expect(drawnNote(64, 52)).toBe("three.js's loader for this format drew 52 of the 64 triangles litegen reads in this file.");
    expect(drawnNote(12_000, 9_500)).toBe("three.js's loader for this format drew 9,500 of the 12,000 triangles litegen reads in this file.");
  });

  it('says nothing when the loader drew them all, or more (a mesh placed twice is drawn twice)', () => {
    expect(drawnNote(64, 64)).toBeNull();
    expect(drawnNote(64, 128)).toBeNull();
  });

  it('says nothing when either count is unknown', () => {
    expect(drawnNote(undefined, 3)).toBeNull();
    expect(drawnNote(null, 3)).toBeNull();
    expect(drawnNote(64, undefined)).toBeNull();
  });
});

describe('copyToClipboard', () => {
  it('reports success when the write resolves', async () => {
    const writes: string[] = [];
    const clipboard = { writeText: async (t: string) => { writes.push(t); } };
    await expect(copyToClipboard('https://x/m.glb', clipboard)).resolves.toBe(true);
    expect(writes).toEqual(['https://x/m.glb']);
  });

  it('reports failure without throwing when the clipboard API is missing', async () => {
    await expect(copyToClipboard('u', undefined)).resolves.toBe(false);
    await expect(copyToClipboard('u', null)).resolves.toBe(false);
  });

  it('reports failure without throwing when the write is denied', async () => {
    const clipboard = { writeText: () => Promise.reject(new DOMException('denied', 'NotAllowedError')) };
    await expect(copyToClipboard('u', clipboard)).resolves.toBe(false);
  });
});
