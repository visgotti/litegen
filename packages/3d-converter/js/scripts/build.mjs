// Builds dist/ for @litegen/3d-converter:
//   1. cargo (rustup's toolchain, pinned by ../rust-toolchain.toml) for
//      wasm32-unknown-unknown, profile release-wasm;
//   2. wasm-bindgen --target web (its version must equal the crate's);
//   3. wasm-opt;
//   4. the two entries (node.js, web.js) and index.d.ts;
// then prints the wasm's raw, gzip and brotli sizes.
//
// CONVERTER_WASM_OPT and CONVERTER_WASM_SIMD override Task 7's chosen
// settings, for measuring the variants.
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { brotliCompressSync, constants, gzipSync } from 'node:zlib';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const js = fileURLToPath(new URL('..', import.meta.url));
const workspace = path.resolve(js, '..');
const dist = path.join(js, 'dist');
const env = { ...process.env, PATH: `/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:${process.env.PATH}`, CARGO_INCREMENTAL: '0' };

// Chosen by measurement over opt-level 3/s x simd128 on/off x -O3/-Oz
// (js/README.md, "How it is built"): -O3 without simd128 was fastest, and no
// variant within 5% of it was 15% smaller. simd128 bought no speed.
const OPT = process.env.CONVERTER_WASM_OPT ?? '-O3';
const SIMD = (process.env.CONVERTER_WASM_SIMD ?? '0') === '1';
const PROFILE = 'release-wasm';
// rustc's default wasm32 features (bulk memory, saturating float-to-int, sign
// extension, mutable globals, reference types, multivalue), which wasm-opt
// must be told it may keep.
const FEATURES = ['--enable-bulk-memory', '--enable-nontrapping-float-to-int', '--enable-sign-ext', '--enable-mutable-globals', '--enable-reference-types', '--enable-multivalue'];

const run = (cmd, args, extra = {}) => execFileSync(cmd, args, { stdio: 'inherit', env, ...extra });

const lockedBindgen = (() => {
  const lock = readFileSync(path.join(workspace, 'Cargo.lock'), 'utf8');
  const m = lock.match(/\[\[package\]\]\nname = "wasm-bindgen"\nversion = "([^"]+)"/);
  if (!m) throw new Error('wasm-bindgen is not in Cargo.lock');
  return m[1];
})();
const bindgen = path.join(workspace, '.tools/bin/wasm-bindgen');
const bindgenVersion = execFileSync(bindgen, ['--version'], { encoding: 'utf8' }).trim().split(' ')[1];
if (bindgenVersion !== lockedBindgen) {
  throw new Error(`wasm-bindgen CLI ${bindgenVersion} != crate ${lockedBindgen}; install it with \`cargo install wasm-bindgen-cli --version ${lockedBindgen} --locked --root ${path.join(workspace, '.tools')}\``);
}

const rustflags = [process.env.RUSTFLAGS, SIMD ? '-C target-feature=+simd128' : ''].filter(Boolean).join(' ');
run('cargo', ['build', '-p', 'litegen-3d-converter-js', '--target', 'wasm32-unknown-unknown', '--profile', PROFILE], { cwd: workspace, env: { ...env, RUSTFLAGS: rustflags } });

rmSync(dist, { recursive: true, force: true });
const built = path.join(workspace, 'target/wasm32-unknown-unknown', PROFILE, 'litegen_3d_converter_js.wasm');
run(bindgen, [built, '--target', 'web', '--out-dir', dist, '--out-name', 'litegen_3d_converter']);
rmSync(path.join(dist, 'litegen_3d_converter_bg.wasm.d.ts'), { force: true });
// The entries instantiate the wasm themselves, so the init declarations go:
// they name DOM-only types (RequestInfo, Response, BufferSource) that a Node
// project without lib.dom cannot resolve.
const dts = path.join(dist, 'litegen_3d_converter.d.ts');
const declarations = readFileSync(dts, 'utf8');
const initAt = declarations.indexOf('\nexport type InitInput');
if (initAt < 0) throw new Error('wasm-bindgen .d.ts: no InitInput block to strip');
// Model's Symbol.dispose needs the disposable lib, which a project on ES2022
// alone lacks: the declarations bring it, so they compile there too.
writeFileSync(dts, `/// <reference lib="esnext.disposable" />\n${declarations.slice(0, initAt)}\n`);

const wasm = path.join(dist, 'litegen_3d_converter_bg.wasm');
run('wasm-opt', [wasm, OPT, ...FEATURES, ...(SIMD ? ['--enable-simd'] : []), '-o', wasm]);

const walk = (dir) => readdirSync(dir).flatMap((n) => (statSync(path.join(dir, n)).isDirectory() ? walk(path.join(dir, n)) : [path.join(dir, n)]));
const snippets = walk(path.join(dist, 'snippets')).filter((p) => p.endsWith(`${path.sep}convert_error.js`));
if (snippets.length !== 1) throw new Error(`expected one convert_error.js snippet, found ${snippets.length}`);
const snippet = `./${path.relative(dist, snippets[0]).split(path.sep).join('/')}`;
for (const entry of ['node.js', 'web.js']) {
  const text = readFileSync(path.join(js, 'entry', entry), 'utf8').replace('__CONVERT_ERROR__', snippet);
  writeFileSync(path.join(dist, entry), text);
}
copyFileSync(path.join(js, 'entry/index.d.ts'), path.join(dist, 'index.d.ts'));
copyFileSync(path.join(js, 'entry/model.js'), path.join(dist, 'model.js'));
mkdirSync(dist, { recursive: true });

const bytes = readFileSync(wasm);
const kb = (n) => `${(n / 1024).toFixed(0)} KiB`;
const br = brotliCompressSync(bytes, { params: { [constants.BROTLI_PARAM_QUALITY]: 11 } }).length;
console.log(`wasm: ${kb(bytes.length)} raw, ${kb(gzipSync(bytes, { level: 9 }).length)} gzip, ${kb(br)} brotli (opt ${OPT}, simd ${SIMD ? 'on' : 'off'})`);
