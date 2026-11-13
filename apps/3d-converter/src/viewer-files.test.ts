import { describe, expect, it } from 'vitest';
import { viewerFilesOf } from './viewer-files';

describe('viewerFilesOf', () => {
  it("keeps a zip's entries whose names differ from the read's only in separators", () => {
    const entries = { 'model.obj': new Uint8Array([1]), 'textures\\a.png': new Uint8Array([2]), '__MACOSX/._model.obj': new Uint8Array([3]) };
    const roles = new Map([['model.obj', 'model' as const], ['textures/a.png', 'texture' as const]]);
    expect(viewerFilesOf(entries, roles).map((f) => [f.name, f.role])).toEqual([
      ['model.obj', 'model'],
      ['textures/a.png', 'texture'],
    ]);
  });

  it('drops folder entries and leaves a file the read gave no role without one', () => {
    const entries = { 'textures/': new Uint8Array(), 'readme.txt': new Uint8Array([4]) };
    expect(viewerFilesOf(entries, new Map())).toEqual([{ name: 'readme.txt', data: new Uint8Array([4]) }]);
  });
});
