import { describe, expect, it } from 'vitest';
import { textureFilesOf, thumbnailBlob } from './texture-thumbnails';

// A thumbnail links to its blob in a new tab. The bytes come from a dropped
// model, and only the file name says they are an image: an untyped blob is
// left to the browser to sniff, so a "texture" that is really HTML could open
// as a page on the converter's origin. Typed as the image its name says, the
// browser shows it as an image or not at all.
describe('thumbnailBlob', () => {
  const bytes = new Uint8Array([60, 104, 116, 109, 108, 62]); // "<html>"

  it('types the blob as the image its extension names', () => {
    for (const [name, type] of [
      ['textures/skin.png', 'image/png'],
      ['a/b/Albedo.JPG', 'image/jpeg'],
      ['c.jpeg', 'image/jpeg'],
      ['d.webp', 'image/webp'],
      ['e.gif', 'image/gif'],
      ['f.avif', 'image/avif'],
    ]) {
      expect(thumbnailBlob({ name, data: bytes, role: 'texture' }).type, name).toBe(type);
    }
  });

  it('makes thumbnails only of the textures it can type', () => {
    const files = [
      { name: 'skin.png', data: bytes, role: 'texture' as const },
      { name: 'normal.ktx2', data: bytes, role: 'texture' as const },
      { name: 'page.html', data: bytes, role: 'texture' as const },
      { name: 'model.gltf', data: bytes, role: 'model' as const },
    ];
    expect(textureFilesOf(files).map((f) => f.name)).toEqual(['skin.png']);
  });
});
