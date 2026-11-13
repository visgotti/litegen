import { describe, expect, it } from 'vitest';
import * as THREE from 'three';
import { FileServer, fixTextUsdRotations, needsZUpTurn, showVertexColors, usdRootIsText } from './loaders';

const text = (s: string) => new TextEncoder().encode(s);

/** A zip whose entries are named `names`, empty (stored), as a USDZ is laid out. */
function zipOf(names: string[]): Uint8Array {
  const parts: number[] = [];
  for (const name of names) {
    const n = text(name);
    const header = new Uint8Array(30);
    const view = new DataView(header.buffer);
    view.setUint32(0, 0x04034b50, true);
    view.setUint16(26, n.length, true);
    parts.push(...header, ...n);
  }
  return new Uint8Array(parts);
}

describe('fixTextUsdRotations', () => {
  it('reorders rotation keys read real-first into three.js order', () => {
    // USD text writes identity as (1, 0, 0, 0), real first; three.js r183
    // reads those four numbers as x, y, z, w.
    const asRead = new THREE.QuaternionKeyframeTrack('knee.quaternion', [0, 1], [1, 0, 0, 0, 0.94, 0, 0, 0.34]);
    const moves = new THREE.VectorKeyframeTrack('knee.position', [0], [1, 2, 3]);
    const clip = new THREE.AnimationClip('Bend', 1, [asRead, moves]);
    fixTextUsdRotations([clip]);
    expect(Array.from(asRead.values).map((v) => +v.toFixed(2))).toEqual([0, 0, 0, 1, 0, 0, 0.34, 0.94]);
    expect(Array.from(moves.values)).toEqual([1, 2, 3]);
  });
});

describe('usdRootIsText', () => {
  it('reads a .usda or .usdc file by its first bytes, whichever name it has', () => {
    expect(usdRootIsText('usda', text('#usda 1.0\n'))).toBe(true);
    expect(usdRootIsText('usda', text('PXR-USDC'))).toBe(false);
    // Either name reads either encoding, so the bytes decide.
    expect(usdRootIsText('usdc', text('PXR-USDC'))).toBe(false);
    expect(usdRootIsText('usdc', text('#usda 1.0\n'))).toBe(true);
  });
  it('reads a USDZ by its first entry, the root layer', () => {
    expect(usdRootIsText('usdz', zipOf(['model.usda', 'textures/a.png']))).toBe(true);
    expect(usdRootIsText('usdz', zipOf(['model.usdc', 'textures/a.png']))).toBe(false);
  });
  it('is false for every other format, whatever the bytes', () => {
    for (const format of ['glb', 'gltf', 'obj', 'stl', 'ply', '3mf', 'dae', 'fbx'] as const) {
      expect(usdRootIsText(format, text('#usda 1.0\n')), format).toBe(false);
      expect(usdRootIsText(format, zipOf(['model.usda'])), format).toBe(false);
    }
  });
});

describe('showVertexColors', () => {
  it("turns on vertex colours for a mesh that carries them, as ColladaLoader doesn't", () => {
    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute('position', new THREE.Float32BufferAttribute([0, 0, 0, 1, 0, 0, 0, 1, 0], 3));
    geometry.setAttribute('color', new THREE.Float32BufferAttribute([0, 0, 1, 0, 0, 1, 0, 0, 1], 3));
    const coloured = new THREE.Mesh(geometry, new THREE.MeshPhongMaterial());
    const plain = new THREE.Mesh(new THREE.BufferGeometry(), new THREE.MeshPhongMaterial());
    const root = new THREE.Group().add(coloured, plain);
    showVertexColors(root);
    expect((coloured.material as THREE.Material).vertexColors).toBe(true);
    expect((plain.material as THREE.Material).vertexColors).toBe(false);
  });
});


describe('needsZUpTurn', () => {
  it('turns a file its format stores +Z up (3MF), whatever the caller says', () => {
    expect(needsZUpTurn('z_up', undefined)).toBe(true);
    expect(needsZUpTurn('z_up', 'y')).toBe(true);
  });

  it('turns a frameless format only when the caller says it was written +Z up', () => {
    expect(needsZUpTurn('caller', 'z')).toBe(true);
    expect(needsZUpTurn('caller', 'y')).toBe(false);
    expect(needsZUpTurn('caller', undefined)).toBe(false);
  });

  it('leaves +Y files, files that declare their axis (the loader applies it), and unknown frames alone', () => {
    expect(needsZUpTurn('y_up', 'z')).toBe(false);
    expect(needsZUpTurn('declared', 'z')).toBe(false);
    expect(needsZUpTurn(undefined, 'z')).toBe(false);
    expect(needsZUpTurn(undefined, undefined)).toBe(false);
  });
});

describe('FileServer.settled', () => {
  const tick = (ms: number) => new Promise((r) => setTimeout(r, ms));

  it('waits for every load the loaders started, whether it worked or failed', async () => {
    const server = new FileServer([]);
    const m = server.manager;
    m.itemStart('textures/a.png');
    m.itemStart('textures/missing.png');
    let result: boolean | undefined;
    const waiting = server.settled(2_000).then((v) => { result = v; });
    m.itemEnd('textures/a.png');
    await tick(80);
    expect(result, 'one load still in flight').toBeUndefined();
    // A file that was not given fails at once: a loader reports the error,
    // then the end, and the model draws untextured rather than waiting.
    m.itemError('textures/missing.png');
    m.itemEnd('textures/missing.png');
    await waiting;
    expect(result).toBe(true);
    server.dispose();
  });

  it('is settled at once when nothing is loading', async () => {
    const server = new FileServer([]);
    await expect(server.settled(2_000)).resolves.toBe(true);
    server.dispose();
  });

  it('gives up after its time, and when the load it serves is cancelled', async () => {
    const server = new FileServer([]);
    server.manager.itemStart('slow.png');
    await expect(server.settled(60)).resolves.toBe(false);
    let cancelled = false;
    const waiting = server.settled(10_000, () => cancelled);
    cancelled = true;
    const started = Date.now();
    await expect(waiting).resolves.toBe(false);
    expect(Date.now() - started).toBeLessThan(1_000);
    server.dispose();
  });
});
