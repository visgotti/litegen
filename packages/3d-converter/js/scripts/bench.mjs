// Times a fixed workload against a built dist/ (default ../dist):
//   node scripts/bench.mjs [distDir]
// Prints the median of 5 timed rounds, after one warm-up round:
//   - three real files (litegen's skinned glb, assimp's textured fbx, Apple's
//     textured usdz) converted to all ten formats;
//   - a dense 512x512 grid (about 520k triangles), as OBJ, into the formats
//     that do the most work per triangle.
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = fileURLToPath(new URL('.', import.meta.url));
const dist = path.resolve(process.argv[2] ?? path.join(here, '../dist'));
const { convert, readModel } = await import(pathToFileURL(path.join(dist, 'node.js')).href);

const testdata = (p) => new Uint8Array(readFileSync(path.join(here, '../../../../litegen-core/src/mesh/testdata', p)));
const real = ['fbx/skinned_small_source.glb', 'fbx/textured_assimp_7500.fbx', 'usd/textured_apple.usdz'].map(testdata);
const targets = ['glb', 'gltf', 'obj', 'stl', 'ply', '3mf', 'dae', 'fbx', 'usdz', 'usda'];

function grid(n) {
  const lines = [];
  for (let y = 0; y <= n; y++) for (let x = 0; x <= n; x++) lines.push(`v ${x / n} ${Math.sin(x / 9) * Math.cos(y / 7) * 0.05} ${y / n}`);
  for (let y = 0; y < n; y++) {
    for (let x = 0; x < n; x++) {
      const a = y * (n + 1) + x + 1;
      lines.push(`f ${a} ${a + 1} ${a + n + 2}`, `f ${a} ${a + n + 2} ${a + n + 1}`);
    }
  }
  return new TextEncoder().encode(`${lines.join('\n')}\n`);
}
const dense = grid(512);

function round() {
  const started = performance.now();
  for (const bytes of real) for (const to of targets) convert(bytes, { to });
  const m = readModel(dense, { from: 'obj' });
  try {
    for (const to of ['glb', 'fbx', 'usdz', '3mf', 'dae', 'stl', 'ply']) m.write({ to });
  } finally {
    m.free();
  }
  return performance.now() - started;
}

round();
const times = Array.from({ length: 5 }, round).sort((a, b) => a - b);
console.log(JSON.stringify({ dist, median_ms: Math.round(times[2]), min_ms: Math.round(times[0]), max_ms: Math.round(times[4]) }));
