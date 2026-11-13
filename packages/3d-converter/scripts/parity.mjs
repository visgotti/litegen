// Parity with the hosted converter: every sample's main file through every
// target, as one file and as a zip, three ways: litegen's own HTTP converter
// (POST /v1/utils/models3d/convert, on its release binary), the npm package
// (wasm, in Node) and the PyPI package (native, through parity_py.py). The
// HTTP API takes one file, so all three get the main file alone.
//
//   node packages/3d-converter/scripts/parity.mjs
//   LITEGEN_BINARY=litegen-core/target/debug/litegen node …/parity.mjs
//
// It never builds anything. It uses litegen's binary as it was last built
// (release by default; a debug build writes the same bytes, since Rust never
// relaxes IEEE float semantics), and the packages as they were last built
// (js/dist and the wheel installed in python/.venv314). It runs only when all
// three are newer than every file of litegen-core/src/mesh and the converter
// handler, with no NEGCTL mutant in the tree, so all three were built from
// one source. Exit 0 when every pair was compared; the summary says which
// differ.
import { execFileSync, spawn } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { mkdtempSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../../..');
const binary = path.resolve(repo, process.env.LITEGEN_BINARY ?? 'litegen-core/target/release/litegen');
const samplesDir = path.join(repo, 'apps/3d-converter/public/samples');
const python = path.join(repo, 'packages/3d-converter/python/.venv314/bin/python');
const { convert, formats } = await import(path.join(repo, 'packages/3d-converter/js/dist/node.js'));

function newest(dir) {
  let t = 0;
  for (const name of readdirSync(dir)) {
    const p = path.join(dir, name);
    const s = statSync(p);
    t = Math.max(t, s.isDirectory() ? newest(p) : s.mtimeMs);
  }
  return t;
}

function refuse(why) {
  console.error(`parity: not run: ${why}`);
  process.exit(2);
}

const source = Math.max(newest(path.join(repo, 'litegen-core/src/mesh')), statSync(path.join(repo, 'litegen-core/src/api/handlers/convert.rs')).mtimeMs);
const installedWheel = execFileSync(python, ['-I', '-c', 'import litegen_3d_converter, os; print(os.path.dirname(litegen_3d_converter.__file__))'], { encoding: 'utf8' }).trim();
const builds = {
  [path.relative(repo, binary)]: binary,
  'js/dist (wasm)': path.join(repo, 'packages/3d-converter/js/dist/litegen_3d_converter_bg.wasm'),
  'the wheel in python/.venv314': installedWheel,
};
for (const [what, file] of Object.entries(builds)) {
  let built;
  try {
    built = statSync(file).mtimeMs;
  } catch {
    refuse(`${file} does not exist (this script never builds it)`);
  }
  if (built < source) {
    const why = `${what} (${new Date(built).toISOString()}) is older than litegen-core/src/mesh or convert.rs (${new Date(source).toISOString()})`;
    // PARITY_STALE_OK="<how you know the newer edits change no code>" runs
    // anyway, for edits that are comments only; the reason is printed.
    if (!process.env.PARITY_STALE_OK) refuse(why);
    console.error(`parity: running although ${why}, because: ${process.env.PARITY_STALE_OK}`);
  }
}
try {
  execFileSync('grep', ['-rqn', 'NEGCTL', path.join(repo, 'litegen-core/src'), path.join(repo, 'scripts/mesh-verify')]);
  refuse('a NEGCTL mutant is in the tree');
} catch (e) {
  if (e.status !== 1) throw e;
}

const port = await new Promise((resolve) => {
  const s = createServer().listen(0, '127.0.0.1', () => {
    const p = s.address().port;
    s.close(() => resolve(p));
  });
});
const key = `parity-${randomBytes(12).toString('hex')}`;
const cwd = mkdtempSync(path.join(tmpdir(), 'converter-parity-'));
const server = spawn(binary, [], {
  cwd,
  env: {
    ...process.env,
    LITEGEN__SERVER__HOST: '127.0.0.1',
    LITEGEN__SERVER__PORT: String(port),
    LITEGEN__DATABASE_URL: 'sqlite://:memory:',
    LITEGEN__MASTER_KEY: key,
    LITEGEN_MODELS_DIR: path.join(repo, 'models'),
    LITEGEN__PROVIDERS__MOCK__API_KEY: '',
  },
  stdio: ['ignore', 'ignore', 'pipe'],
});
let stderr = '';
server.stderr.on('data', (d) => {
  stderr += d;
});
const base = `http://127.0.0.1:${port}`;
for (let i = 0; ; i++) {
  try {
    // Up once it answers; /health takes the key in some builds.
    if ((await fetch(`${base}/health`, { headers: { authorization: `Bearer ${key}` } })).status < 500) break;
  } catch {}
  if (i > 150) {
    server.kill();
    refuse(`litegen did not start: ${stderr.slice(-800)}`);
  }
  await new Promise((r) => setTimeout(r, 200));
}

const sha = (bytes) => createHash('sha256').update(bytes).digest('hex');
const samples = JSON.parse(readFileSync(path.join(samplesDir, 'samples.json'), 'utf8'));
const targets = formats().map((f) => f.format);
const jobs = [];
for (const s of samples) {
  const main = s.files[0];
  const file = path.join(samplesDir, s.id, main);
  for (const to of targets) for (const pkg of ['file', 'zip']) jobs.push({ sample: s.id, path: file, from: s.format, to, package: pkg });
}

const results = [];
try {
  for (const j of jobs) {
    const input = new Uint8Array(readFileSync(j.path));
    const response = await fetch(`${base}/v1/utils/models3d/convert?from=${j.from}&to=${j.to}&package=${j.package}`, {
      method: 'POST',
      headers: { authorization: `Bearer ${key}`, 'content-type': 'application/octet-stream' },
      body: input,
    });
    const http = new Uint8Array(await response.arrayBuffer());
    let wasm;
    try {
      wasm = convert(input, { from: j.from, to: j.to, package: j.package }).files[0].data;
    } catch (e) {
      wasm = null;
      j.wasmError = `${e.code}: ${e.message}`;
    }
    results.push({ ...j, status: response.status, http, wasm });
  }
} finally {
  server.kill();
}

const native = JSON.parse(execFileSync(python, ['-I', path.join(here, 'parity_py.py')], { input: JSON.stringify(jobs), maxBuffer: 64 << 20, encoding: 'utf8' }));

const firstDiff = (a, b) => {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) if (a[i] !== b[i]) return i;
  return a.length === b.length ? -1 : n;
};
const tally = { wasm: { same: 0, differ: [] }, python: { same: 0, differ: [] } };
let httpFailures = 0;
results.forEach((r, i) => {
  const label = `${r.sample} ${r.from}->${r.to} (${r.package})`;
  if (r.status !== 200) {
    httpFailures++;
    console.log(`HTTP ${r.status} for ${label}: ${new TextDecoder().decode(r.http).slice(0, 200)}`);
    return;
  }
  const h = sha(r.http);
  if (r.wasm && sha(r.wasm) === h) tally.wasm.same++;
  else tally.wasm.differ.push(`${label} first differing byte ${r.wasm ? firstDiff(r.http, r.wasm) : 'n/a'}${r.wasmError ? ` (${r.wasmError})` : ''} of ${r.http.length}`);
  if (native[i].sha256 === h) tally.python.same++;
  else tally.python.differ.push(`${label}: python ${native[i].error ?? native[i].size + ' bytes'} vs http ${r.http.length} bytes`);
});
console.log(JSON.stringify({ compared: results.length, httpFailures, wasm: { identical: tally.wasm.same, differ: tally.wasm.differ.length }, python: { identical: tally.python.same, differ: tally.python.differ.length } }));
for (const d of tally.wasm.differ) console.log(`wasm differs: ${d}`);
for (const d of tally.python.differ) console.log(`python differs: ${d}`);
