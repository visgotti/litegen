import { describe, expect, it } from 'vitest';
import type { Conversion } from '@litegen/3d-converter';
import { foldersIn } from './Downloads';

const conversion = (names: string[]): Conversion =>
  ({ format: 'obj', files: names.map((name) => ({ name, mediaType: 'model/obj', role: 'model', data: new Uint8Array(0) })), losses: [], warnings: [] }) as Conversion;

describe('foldersIn', () => {
  it('is true when a written file sits in a folder, which a browser download drops', () => {
    expect(foldersIn(conversion(['model.obj', 'model.mtl', 'textures/texture_0.png']))).toBe(true);
  });
  it('is false for flat names', () => {
    expect(foldersIn(conversion(['model.gltf', 'model.bin', 'quadrants.png']))).toBe(false);
    expect(foldersIn(conversion(['model.obj']))).toBe(false);
  });
});
