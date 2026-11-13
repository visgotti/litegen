import { describe, expect, it } from 'vitest';
import { filesFromEntries, filesFromList, type EntryLike, type FileLike } from './intake';

const file = (name: string, bytes: number[], webkitRelativePath = ''): FileLike => ({
  name,
  webkitRelativePath,
  arrayBuffer: async () => new Uint8Array(bytes).buffer,
});

/** A dropped folder as the File and Directory Entries API presents it:
 *  directories read in batches until one comes back empty. */
function dir(name: string, fullPath: string, children: EntryLike[]): EntryLike {
  return {
    isFile: false,
    isDirectory: true,
    name,
    fullPath,
    createReader() {
      let batches = [children.slice(0, 1), children.slice(1), []];
      return {
        readEntries(ok: (entries: EntryLike[]) => void) {
          const [next, ...rest] = batches;
          batches = rest;
          ok(next ?? []);
        },
      };
    },
  };
}
function leaf(name: string, fullPath: string, bytes: number[]): EntryLike {
  return { isFile: true, isDirectory: false, name, fullPath, file: (ok: (f: FileLike) => void) => ok(file(name, bytes)) };
}

describe('intake', () => {
  it('a flat list uses the file names', async () => {
    const got = await filesFromList([file('model.obj', [1]), file('model.mtl', [2])]);
    expect(Object.keys(got).sort()).toEqual(['model.mtl', 'model.obj']);
    expect([...got['model.obj']!]).toEqual([1]);
  });

  it('webkitRelativePath wins when a folder was picked', async () => {
    const got = await filesFromList([file('t.png', [3], 'pack/textures/t.png'), file('model.obj', [1], 'pack/model.obj')]);
    expect(Object.keys(got).sort()).toEqual(['pack/model.obj', 'pack/textures/t.png']);
  });

  it('dropped folders keep their relative paths, read in every batch', async () => {
    const tree = dir('pack', '/pack', [
      leaf('model.obj', '/pack/model.obj', [1]),
      leaf('model.mtl', '/pack/model.mtl', [2]),
      dir('textures', '/pack/textures', [leaf('t.png', '/pack/textures/t.png', [3])]),
    ]);
    const got = await filesFromEntries([tree]);
    expect(Object.keys(got).sort()).toEqual(['pack/model.mtl', 'pack/model.obj', 'pack/textures/t.png']);
    expect([...got['pack/textures/t.png']!]).toEqual([3]);
  });
});
