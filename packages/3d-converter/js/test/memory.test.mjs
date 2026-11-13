// What the package costs in WebAssembly memory, which only ever grows: an
// input past the cap is refused before it is copied in.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { convert, readModel, maxInputBytes, ConvertError } from '@litegen/3d-converter';
// The module @litegen/3d-converter loads, already instantiated: initSync
// hands back its exports, the memory among them.
import { initSync } from '../dist/litegen_3d_converter.js';

const wasm = initSync({ module: readFileSync(new URL('../dist/litegen_3d_converter_bg.wasm', import.meta.url)) });
const mib = () => wasm.memory.buffer.byteLength / 2 ** 20;

test('an input past the cap is refused before it is copied into WebAssembly memory', () => {
  const before = mib();
  const big = new Uint8Array(maxInputBytes() + 1);
  const half = big.subarray(0, maxInputBytes() / 2 + 1);
  const calls = [
    () => convert(big, { to: 'glb' }),
    () => readModel(big),
    () => convert({ 'a.glb': big }, { to: 'glb' }),
    () => readModel({ 'a.gltf': half, 'a.bin': half }),
  ];
  for (const call of calls) {
    assert.throws(call, (e) => e instanceof ConvertError && e.code === 'input_too_large');
  }
  assert.ok(mib() - before < 16, `memory grew from ${before} MiB to ${mib()} MiB`);
});

test('detectFormat refuses an input past the cap before copying it', async () => {
  const { detectFormat } = await import('@litegen/3d-converter');
  const before = mib();
  const big = new Uint8Array(maxInputBytes() + 1);
  assert.throws(() => detectFormat(big), (e) => e instanceof ConvertError && e.code === 'input_too_large');
  assert.ok(mib() - before < 16, `memory grew from ${before} MiB to ${mib()} MiB`);
});
