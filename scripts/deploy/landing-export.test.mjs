// deploy.js ships the landing export its own build wrote, or stops.
//
// Runs a copy of the real deploy.js with child_process.execSync intercepted:
// no npm, no Next build, no tar, no wrangler and no SSH. The intercepted build
// writes a landing export where `next build` would put a static export for the
// NEXT_DIST_DIR it is given (Next 15: with output 'export', a distDir other
// than '.next' IS the export folder — node_modules/next/dist/export/utils.js
// hasCustomExportOutput, build/index.js `configOutDir = config.distDir`). That
// export holds en.html and, as every real one does, the API reference page
// with its SDK samples. The intercepted `wrangler pages deploy` records what
// the shipped folder holds.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, utimesSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const DEPLOY = join(dirname(fileURLToPath(import.meta.url)), '..', '..', 'deploy.js');

// The SDK samples (apps/landing/src/config/site.ts SDK_TS_CODE, SDK_PY_CODE)
// show http://localhost:4000 on purpose. These are excerpts of a real export
// (2026-11-11), which renders them highlighted in en/ and es/reference.html,
// and as plain strings in the reference page's JS chunk.
const REFERENCE_HTML = `<pre class="CodeBlock_pre__3xNpN"><code class="hljs"><span class="hljs-keyword">const</span> client = <span class="hljs-keyword">new</span> <span class="hljs-title class_">LiteGenClient</span>({
  <span class="hljs-attr">baseUrl</span>: process.<span class="hljs-property">env</span>.<span class="hljs-property">LITEGEN_BASE_URL</span> ?? <span class="hljs-string">&#x27;http://localhost:4000&#x27;</span>,
  <span class="hljs-attr">apiKey</span>: process.<span class="hljs-property">env</span>.<span class="hljs-property">LITEGEN_API_KEY</span>,
});</code></pre>`;
const REFERENCE_CHUNK = String.raw`(0,a.jsx)(o.CodeBlock,{code:"ts"===t?"const client = new LiteGenClient({\n  baseUrl: process.env.LITEGEN_BASE_URL ?? 'http://localhost:4000',\n  apiKey: process.env.LITEGEN_API_KEY,\n});":'client = LiteGenClient(\n    base_url="http://localhost:4000",\n    api_key="lg-...",\n)'})`;
const LANDING_EXPORT = {
  'en.html': 'fresh export',
  'en/reference.html': REFERENCE_HTML,
  'es/reference.html': REFERENCE_HTML,
  '_next/static/chunks/app/[locale]/reference/page-e91f1575c48155f7.js': REFERENCE_CHUNK,
};

// A dashboard bundle holds the SDK's own `??` fallback, which the app always
// overrides, and the API base its build wires in (dashboard/vite.config.ts:
// VITE_API_URL, else http://localhost:4000). Excerpts of a real Vite build.
const dashboardBundle = (apiBase) =>
  'this.baseUrl=(e.baseUrl??`http://localhost:4000`).replace(/\\/$/,``);var mB=`' + apiBase + '`,hB=mB;';

const INTERCEPT = `
const cp = require('child_process');
const fs = require('fs');
const path = require('path');
const realExecSync = cp.execSync;
const log = process.env.INTERCEPT_LOG;
cp.execSync = (cmd, opts = {}) => {
  if (/^grep /.test(cmd)) return realExecSync(cmd, opts);
  const env = opts.env || process.env;
  const cwd = opts.cwd || process.cwd();
  const entry = { cmd, NEXT_DIST_DIR: env.NEXT_DIST_DIR ?? null };
  const prefixed = cmd.match(/^npm --prefix (\\S+) run build$/);
  const app = prefixed ? path.join(cwd, prefixed[1]) : cmd === 'npm run build' ? cwd : null;
  if (app && path.basename(app) === 'dashboard') {
    fs.mkdirSync(path.join(app, 'dist', 'assets'), { recursive: true });
    fs.writeFileSync(path.join(app, 'dist', 'index.html'), 'dashboard');
    fs.writeFileSync(path.join(app, 'dist', 'assets', 'index-MpfJOURt.js'), process.env.INTERCEPT_DASHBOARD);
  }
  if (app && path.basename(app) === 'landing' && process.env.INTERCEPT_BUILD !== 'nothing') {
    const dist = env.NEXT_DIST_DIR || '.next';
    const exportDir = path.join(app, dist === '.next' ? 'out' : dist);
    // A real build takes seconds; Linux's file clock lags Date.now() by up to one tick.
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 50);
    for (const [file, body] of Object.entries(JSON.parse(process.env.INTERCEPT_EXPORT))) {
      fs.mkdirSync(path.dirname(path.join(exportDir, file)), { recursive: true });
      fs.writeFileSync(path.join(exportDir, file), body);
    }
  }
  const d = cmd.match(/wrangler pages deploy (\\S+)/);
  if (d) {
    entry.shipped = fs.readFileSync(path.join(cwd, d[1], 'en.html'), 'utf8');
    const reference = path.join(cwd, d[1], 'en', 'reference.html');
    entry.reference = fs.existsSync(reference) ? fs.readFileSync(reference, 'utf8') : null;
  }
  fs.appendFileSync(log, JSON.stringify(entry) + '\\n');
  return '';
};
`;

/**
 * Run a copy of deploy.js with `target` in a throwaway root that holds an
 * apps/landing/out left over from an earlier build, an hour old. The
 * intercepted landing build writes LANDING_EXPORT; `build` 'nothing' makes it
 * write no export at all. The intercepted dashboard build wires `dashboardApi`.
 */
function deploy(target, { build = 'export', dashboardApi = 'https://app.litegen.ai/api' } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'deploy-landing-'));
  copyFileSync(DEPLOY, join(root, 'deploy.js'));
  writeFileSync(join(root, 'intercept.cjs'), INTERCEPT);
  mkdirSync(join(root, 'node_modules', 'node-ssh'), { recursive: true });
  writeFileSync(join(root, 'node_modules', 'node-ssh', 'index.js'), "exports.NodeSSH = class { async connect() { throw new Error('stub ssh: not connecting'); } };");
  mkdirSync(join(root, 'apps', 'landing', 'out'), { recursive: true });
  writeFileSync(join(root, 'apps', 'landing', 'out', 'en.html'), 'stale export');
  const hourAgo = new Date(Date.now() - 3_600_000);
  utimesSync(join(root, 'apps', 'landing', 'out', 'en.html'), hourAgo, hourAgo);
  const log = join(root, 'commands.jsonl');
  const r = spawnSync(process.execPath, ['--require', join(root, 'intercept.cjs'), join(root, 'deploy.js'), target], {
    cwd: root,
    encoding: 'utf8',
    // __LITEGEN_REEXEC skips deploy.js's self re-exec, which would drop --require. No .env.deploy exists in root.
    env: {
      PATH: process.env.PATH,
      HOME: root,
      __LITEGEN_REEXEC: '1',
      INTERCEPT_LOG: log,
      INTERCEPT_BUILD: build,
      INTERCEPT_EXPORT: JSON.stringify(LANDING_EXPORT),
      INTERCEPT_DASHBOARD: dashboardBundle(dashboardApi),
      CLOUDFLARE_API_TOKEN: 'not-a-real-token',
      CLOUDFLARE_ACCOUNT_ID: 'not-a-real-account',
      DO_DROPLET_IP: '192.0.2.1',
    },
  });
  const commands = existsSync(log) ? readFileSync(log, 'utf8').trim().split('\n').map((l) => JSON.parse(l)) : [];
  return { status: r.status, out: r.stdout + r.stderr, commands };
}

test('deploy.js landing ships the export its own build just wrote, never an older apps/landing/out', () => {
  const r = deploy('landing');
  const shipped = r.commands.filter((c) => c.shipped !== undefined);
  assert.ok(shipped.every((c) => c.shipped === 'fresh export'), `wrangler shipped ${JSON.stringify(shipped.map((c) => c.shipped))}:\n${r.out}`);
  assert.equal(r.status, 0, r.out);
  assert.equal(shipped.length, 1, r.out);
});

// Every real landing export shows http://localhost:4000 in its SDK samples, on
// purpose, so deploy.js landing runs no localhost guard on it, as deploy.js web
// has not since 2026-06-04 (6826d1e). The guard is for the dashboard, whose
// wired API base must never be localhost. With it, deploy.js landing refused
// every real export ("Refusing to deploy a broken bundle").
test('deploy.js landing ships an export whose SDK samples show http://localhost:4000, as every landing export does', () => {
  const r = deploy('landing');
  const shipped = r.commands.filter((c) => c.shipped !== undefined);
  assert.equal(r.status, 0, r.out);
  assert.equal(shipped.length, 1, r.out);
  assert.equal(shipped[0].shipped, 'fresh export', r.out);
  assert.equal(shipped[0].reference, REFERENCE_HTML, r.out);
});

test('deploy.js landing stops, shipping nothing, when its build leaves apps/landing/out as it was', () => {
  const r = deploy('landing', { build: 'nothing' });
  assert.deepEqual(r.commands.filter((c) => /wrangler pages deploy/.test(c.cmd)).map((c) => c.shipped), [], r.out);
  assert.notEqual(r.status, 0, r.out);
  // The refusal itself: the stale en.html's own text is "stale export" too.
  assert.match(r.out, /Refusing to ship a stale export/);
});

// deploy.js web shares the freshness check: both cases pass before and after
// it moved into a helper, and fail if web ever ships an export its build did
// not write, or refuses one it did.
test('deploy.js web packages the landing export its build just wrote', () => {
  const r = deploy('web');
  assert.ok(r.commands.some((c) => /tar .*litegen-landing-dist\.tgz/.test(c.cmd)), r.out);
  assert.match(r.out, /stub ssh: not connecting/);
});

test('deploy.js web stops before packaging when its landing build leaves apps/landing/out as it was', () => {
  const r = deploy('web', { build: 'nothing' });
  assert.ok(!r.commands.some((c) => /litegen-landing-dist\.tgz/.test(c.cmd)), r.out);
  assert.notEqual(r.status, 0, r.out);
  assert.match(r.out, /Refusing to ship a stale export/);
});

// The localhost guard stays on the dashboard, the build it is for. A dashboard
// built without VITE_API_URL wires http://localhost:4000 as its API base, and
// deploy.js web refuses it before packaging anything. (The SDK's `??` fallback
// is in every dashboard bundle; the packaging test above ships one.)
test('deploy.js web refuses a dashboard build that wires http://localhost:4000 as its API base', () => {
  const r = deploy('web', { dashboardApi: 'http://localhost:4000' });
  assert.notEqual(r.status, 0, r.out);
  assert.match(r.out, /dashboard: build output references http:\/\/localhost:<port> as a wired endpoint/);
  assert.ok(!r.commands.some((c) => / tar /.test(c.cmd)), r.out);
});
