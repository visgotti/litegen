// What `npm publish` would ship: the built files and nothing else.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { brotliCompressSync, constants } from 'node:zlib';
import { fileURLToPath } from 'node:url';

const pkgDir = fileURLToPath(new URL('..', import.meta.url));

test('the tarball holds exactly the built package', () => {
  const [report] = JSON.parse(execFileSync('npm', ['pack', '--dry-run', '--json', '--ignore-scripts'], { cwd: pkgDir, encoding: 'utf8' }));
  const files = report.files.map((f) => f.path).sort();
  const snippet = files.filter((p) => /^dist\/snippets\/.+\/convert_error\.js$/.test(p));
  assert.equal(snippet.length, 1, `one ConvertError snippet: ${files.join(', ')}`);
  assert.deepEqual(
    files.filter((p) => !snippet.includes(p)),
    [
      'LICENSE',
      'README.md',
      'dist/index.d.ts',
      'dist/litegen_3d_converter.d.ts',
      'dist/litegen_3d_converter.js',
      'dist/litegen_3d_converter_bg.wasm',
      'dist/model.js',
      'dist/node.js',
      'dist/web.js',
      'package.json',
    ],
  );
});

// The chosen build (opt-level 3, wasm-opt -O3, no simd128), brotli-compressed:
// 882 KiB on 2026-11-05; 939 KiB with Unicode name matching (NFC tables) and
// lights and cameras; 1041 KiB on 2026-11-06, once mesh/ decoded Draco
// (draco-core) and meshopt glTF; 1157 KiB on 2026-11-10, once it wrote,
// among others, Collada morph controllers and USDC crates with animation clip
// variants (mesh-gaps items 8 and 6). A build 10% past the last is a
// regression to look at, not a drift to accept.
const BASELINE_KIB = 1157;

test('the wasm stays within its size budget', () => {
  const wasm = readFileSync(new URL('../dist/litegen_3d_converter_bg.wasm', import.meta.url));
  const brotli = brotliCompressSync(wasm, { params: { [constants.BROTLI_PARAM_QUALITY]: 11 } }).length;
  assert.ok(brotli <= BASELINE_KIB * 1024 * 1.1, `${Math.round(brotli / 1024)} KiB brotli, budget ${Math.round(BASELINE_KIB * 1.1)} KiB`);
});
