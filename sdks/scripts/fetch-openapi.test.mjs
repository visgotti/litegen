// fetch-openapi.sh writes sdks/openapi.json, which both SDKs and the website's
// API reference are generated from. Each case runs a copy of the real script in
// a throwaway tree (its REPO_ROOT is two levels up from the script), so the
// repo's own sdks/openapi.json is never touched, and no case runs cargo: the
// boot case puts a stub `cargo` first on PATH and a stub server binary where
// the debug build would be. Needs bash, curl and python3.
import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPT = join(dirname(fileURLToPath(import.meta.url)), 'fetch-openapi.sh');
const COMMITTED = JSON.stringify({ openapi: '3.1.0', info: { title: 'committed', version: '1' }, paths: { '/v1/models': {} } }, null, 4) + '\n';

function tree() {
  const root = mkdtempSync(join(tmpdir(), 'fetch-openapi-'));
  mkdirSync(join(root, 'sdks', 'scripts'), { recursive: true });
  copyFileSync(SCRIPT, join(root, 'sdks', 'scripts', 'fetch-openapi.sh'));
  writeFileSync(join(root, 'sdks', 'openapi.json'), COMMITTED);
  mkdirSync(join(root, 'models'));
  return root;
}

/** Run the copied script; resolves {status, stdout, stderr, spec} (spec: sdks/openapi.json afterwards). */
function fetchOpenapi(root, env) {
  return new Promise((resolve) => {
    // TMPDIR: the script's own temp folder goes where the suite's do.
    const child = spawn('bash', [join(root, 'sdks', 'scripts', 'fetch-openapi.sh')], { env: { HOME: root, PATH: process.env.PATH, TMPDIR: tmpdir(), ...env } });
    let stdout = '', stderr = '';
    child.stdout.on('data', (d) => (stdout += d));
    child.stderr.on('data', (d) => (stderr += d));
    child.on('close', (status) => resolve({ status, stdout, stderr, spec: readFileSync(join(root, 'sdks', 'openapi.json'), 'utf8') }));
  });
}

/** A stub gateway on a free 127.0.0.1 port answering GET /openapi.json with `body`. */
async function stubGateway(status, body, type = 'application/json') {
  const server = createServer((req, res) => {
    res.writeHead(req.url === '/health/live' ? 200 : status, { 'content-type': type });
    res.end(req.url === '/health/live' ? 'ok' : body);
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  after(() => server.close());
  return `http://127.0.0.1:${server.address().port}`;
}

/** A port nothing listens on (bound, then released). */
async function deadPort() {
  const s = createServer();
  await new Promise((r) => s.listen(0, '127.0.0.1', r));
  const { port } = s.address();
  await new Promise((r) => s.close(r));
  return port;
}

test('a gateway that does not answer leaves sdks/openapi.json as it was, and the script says why', async () => {
  const root = tree();
  const r = await fetchOpenapi(root, { LITEGEN_BASE_URL: `http://127.0.0.1:${await deadPort()}` });
  assert.notEqual(r.status, 0);
  assert.equal(r.spec, COMMITTED, 'sdks/openapi.json was overwritten by a failed fetch');
  assert.match(r.stderr, /openapi\.json/);
});

test('an HTML page or a JSON document that is not OpenAPI never replaces sdks/openapi.json', async () => {
  for (const [body, type] of [['<!doctype html><html>maintenance</html>', 'text/html'], ['{}', 'application/json'], ['', 'application/json']]) {
    const root = tree();
    const r = await fetchOpenapi(root, { LITEGEN_BASE_URL: await stubGateway(200, body, type) });
    assert.notEqual(r.status, 0, `${JSON.stringify(body)} was accepted`);
    assert.equal(r.spec, COMMITTED, `${JSON.stringify(body)} replaced sdks/openapi.json`);
  }
});

test('a gateway answering a spec writes it the way python3 -m json.tool prints it', async () => {
  const spec = { openapi: '3.1.0', info: { title: 'fresh', version: '2' }, paths: { '/v1/images/generations': { post: {} } } };
  const root = tree();
  const r = await fetchOpenapi(root, { LITEGEN_BASE_URL: await stubGateway(200, JSON.stringify(spec)) });
  assert.equal(r.status, 0, r.stderr);
  assert.equal(r.spec, JSON.stringify(spec, null, 4) + '\n');
});

test('a server that dies at startup fails at once with its own error, and leaves sdks/openapi.json as it was', async () => {
  const root = tree();
  mkdirSync(join(root, 'bin'));
  writeFileSync(join(root, 'bin', 'cargo'), '#!/bin/sh\nexit 0\n');
  mkdirSync(join(root, 'litegen-core', 'target', 'debug'), { recursive: true });
  writeFileSync(join(root, 'litegen-core', 'target', 'debug', 'litegen'), '#!/bin/sh\necho "stub litegen: missing field bucket_name" >&2\nexit 1\n');
  for (const f of ['bin/cargo', 'litegen-core/target/debug/litegen']) chmodSync(join(root, f), 0o755);
  const started = Date.now();
  const r = await fetchOpenapi(root, { PATH: `${join(root, 'bin')}:${process.env.PATH}`, LITEGEN_CODEGEN_PORT: String(await deadPort()) });
  assert.notEqual(r.status, 0);
  assert.equal(r.spec, COMMITTED, 'sdks/openapi.json was overwritten');
  assert.match(r.stderr, /missing field bucket_name/, "the server's own error is not shown");
  assert.ok(Date.now() - started < 15_000, `took ${Date.now() - started} ms: it waited out the startup loop for a server that had exited`);
});
