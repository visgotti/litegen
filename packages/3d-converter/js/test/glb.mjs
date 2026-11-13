// A GLB built by hand: one mesh of `triangles` unconnected triangles, placed
// by `instances` nodes. Placing every instance makes instances × 3 ×
// triangles vertices, which is what mesh's measuring (inspect) flattens.
export function instancedGlb(instances, triangles = 20_000) {
  const n = triangles * 3;
  const pos = new Float32Array(n * 3);
  for (let k = 0; k < triangles; k++) {
    const x = (k % 200) * 0.01;
    const y = Math.floor(k / 200) * 0.01;
    pos.set([x, y, 0, x + 0.005, y, 0, x, y + 0.005, 0], k * 9);
  }
  const idx = Uint32Array.from({ length: n }, (_, i) => i);
  const bin = new Uint8Array(pos.byteLength + idx.byteLength);
  bin.set(new Uint8Array(pos.buffer), 0);
  bin.set(new Uint8Array(idx.buffer), pos.byteLength);
  const nodes = Array.from({ length: instances }, (_, i) => ({ mesh: 0, translation: [i * 3, 0, 0] }));
  const json = {
    asset: { version: '2.0' },
    scene: 0,
    scenes: [{ nodes: nodes.map((_, i) => i) }],
    nodes,
    meshes: [{ primitives: [{ attributes: { POSITION: 0 }, indices: 1 }] }],
    accessors: [
      { bufferView: 0, componentType: 5126, count: n, type: 'VEC3', min: [0, 0, 0], max: [2, 1.005, 0] },
      { bufferView: 1, componentType: 5125, count: n, type: 'SCALAR' },
    ],
    bufferViews: [
      { buffer: 0, byteOffset: 0, byteLength: pos.byteLength },
      { buffer: 0, byteOffset: pos.byteLength, byteLength: idx.byteLength },
    ],
    buffers: [{ byteLength: bin.byteLength }],
  };
  const text = new TextEncoder().encode(JSON.stringify(json));
  const jsonChunk = new Uint8Array(Math.ceil(text.length / 4) * 4).fill(0x20);
  jsonChunk.set(text);
  const total = 12 + 8 + jsonChunk.length + 8 + bin.length;
  const out = new Uint8Array(total);
  const view = new DataView(out.buffer);
  out.set(new TextEncoder().encode('glTF'), 0);
  view.setUint32(4, 2, true);
  view.setUint32(8, total, true);
  view.setUint32(12, jsonChunk.length, true);
  view.setUint32(16, 0x4e4f534a, true);
  out.set(jsonChunk, 20);
  const at = 20 + jsonChunk.length;
  view.setUint32(at, bin.length, true);
  view.setUint32(at + 4, 0x004e4942, true);
  out.set(bin, at + 8);
  return out;
}
