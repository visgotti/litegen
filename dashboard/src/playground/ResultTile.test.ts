import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import type { ResultTileState } from './types';

// The tiles reach the shared SDK client (whose module body reads
// localStorage) through the file-route hook; a plain URL never calls it.
vi.mock('../sdk-client', () => ({ client: {} }));
const { default: ResultTile } = await import('./ResultTile');
const { default: ResultTile3D } = await import('./ResultTile3D');

/** What a tile shows under its result: the charge, then the latency. */
function metaOf(markup: string): string {
  const meta = markup.match(/<div class="pg-tile-meta">(.*?)<\/div>/)?.[1] ?? '';
  return meta.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();
}

describe('a Compare tile', () => {
  // A charge as the server gave it (fal flux-2-klein at 256x256): three
  // fixed decimals showed it as "$0.000".
  it("shows an image result's sub-cent charge with every digit it has", () => {
    const tile: ResultTileState = {
      key: 'fal/flux-2-klein#0', modelId: 'fal/flux-2-klein', index: 0, status: 'done', mediaType: 'image',
      request: { model: 'fal/flux-2-klein', prompt: 'a fox' }, url: 'https://cdn.example/fox.png',
      costUsd: 0.00033, latencyMs: 900,
    };
    expect(metaOf(renderToStaticMarkup(createElement(ResultTile, { tile, onRerun: () => {} })))).toBe('$0.00033 900ms');
  });

  it("shows a 3D result's charge the same way", () => {
    const tile: ResultTileState = {
      key: 'mock/mesh-3d#0', modelId: 'mock/mesh-3d', index: 0, status: 'error', mediaType: 'model3d',
      request: { model: 'mock/mesh-3d', prompt: 'a fox' }, error: 'completed without a mesh asset',
      costUsd: 0.00033, latencyMs: 4200,
    };
    expect(metaOf(renderToStaticMarkup(createElement(ResultTile3D, { tile, onRerun: () => {} })))).toBe('$0.00033 4200ms');
  });
});
