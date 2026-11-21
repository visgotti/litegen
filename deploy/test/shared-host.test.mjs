// Shared-host helpers: config, Caddyfile rendering, and the generated shell scripts
// run for real under sh and dash against a fake docker CLI.
// Run: npm run test:deploy
import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import {
  chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync,
} from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const {
  resolveSharedConfig, composeCommand, noPublishedPortsCheck, healthCommand,
  imageCleanupCommand, renderSiteCaddyfile, sitePublishScript,
} = require('../shared-host.js');

const here = dirname(fileURLToPath(import.meta.url));
const DEPLOY_DIR = resolve(here, '..');
const REPO = resolve(DEPLOY_DIR, '..');
const FAKE_DOCKER = join(here, 'helpers', 'fake-docker.cjs');

// Inside the repo's gitignored .artifacts/, never the OS temp directory.
mkdirSync(join(REPO, '.artifacts'), { recursive: true });
const SANDBOX = mkdtempSync(join(REPO, '.artifacts', 'deploy-shared-'));
after(() => rmSync(SANDBOX, { recursive: true, force: true }));

const SHELLS = ['/bin/sh', '/bin/dash'];
const shared = (extra = {}) => resolveSharedConfig({ LITEGEN_SHARED_STACK: 'example-stack', ...extra });

// ─── resolveSharedConfig ─────────────────────────────────────────────────────

test('an absent or blank setting means single-host mode', () => {
  assert.equal(resolveSharedConfig({}), null);
  assert.equal(resolveSharedConfig({ LITEGEN_SHARED_STACK: '' }), null);
  assert.equal(resolveSharedConfig({ LITEGEN_SHARED_STACK: '   ' }), null);
});

test('a stack name derives every path, project and alias', () => {
  assert.deepEqual(shared(), {
    stack: 'example-stack',
    remoteDir: '/opt/example-stack',
    siteDir: '/opt/sites/example-stack',
    edgeSiteDir: '/sites/example-stack',
    apiAlias: 'example-stack-api',
    dnsTokenEnv: null,
  });
  assert.equal(shared({ LITEGEN_SHARED_DNS_TOKEN_ENV: 'CF_TOKEN_A' }).dnsTokenEnv, 'CF_TOKEN_A');
});

test('unsafe stack names are refused', () => {
  for (const bad of ['Upper', '-lead', 'trail-', 'a_b', 'a/b', '../etc', 'a b', 'a;b', 'x'.repeat(60)]) {
    assert.throws(() => resolveSharedConfig({ LITEGEN_SHARED_STACK: bad }), /not a valid stack name/, bad);
  }
  assert.equal(resolveSharedConfig({ LITEGEN_SHARED_STACK: 'x'.repeat(59) }).apiAlias, `${'x'.repeat(59)}-api`);
});

test('stack names that collide with host directories are refused', () => {
  for (const reserved of ['edge', 'sites', 'litegen']) {
    assert.throws(() => resolveSharedConfig({ LITEGEN_SHARED_STACK: reserved }), /is reserved/, reserved);
  }
});

test('a DNS token setting must be a variable name, never a value', () => {
  for (const bad of ['abc123', 'A-B', 'A B', '{env.A}']) {
    assert.throws(() => shared({ LITEGEN_SHARED_DNS_TOKEN_ENV: bad }), /not an environment variable name/, bad);
  }
});

// ─── Commands ────────────────────────────────────────────────────────────────

test('compose, health and port-guard commands are exact', () => {
  const compose = 'docker compose -p example-stack --env-file .env -f docker-compose.prod.yml -f docker-compose.shared.yml';
  assert.equal(composeCommand(shared()), compose);
  assert.equal(
    healthCommand(shared()),
    `cd /opt/example-stack && ${compose} exec -T litegen wget -qO- http://127.0.0.1:4000/health/live >/dev/null`,
  );
  assert.equal(noPublishedPortsCheck(shared()), [
    'cd /opt/example-stack',
    `cfg=$(${compose} config --no-interpolate --format json) || exit 1`,
    `case "$cfg" in *'"published"'*) echo 'shared stack example-stack would publish a host port; refusing to start it' >&2; exit 1;; esac`,
  ].join('\n'));
});

test('the compose overlay clears ports and joins edge under the alias', () => {
  const lines = readFileSync(join(DEPLOY_DIR, 'docker-compose.shared.yml'), 'utf8')
    .split('\n')
    .filter((l) => l.trim() && !l.trim().startsWith('#'));
  assert.deepEqual(lines, [
    'services:',
    '  litegen:',
    '    ports: !reset []',
    '    networks:',
    '      default:',
    '      edge:',
    '        aliases:',
    '          - ${LITEGEN_EDGE_ALIAS:?LITEGEN_EDGE_ALIAS must be set}',
    'networks:',
    '  edge:',
    '    external: true',
  ]);
});

test('the image cleanup never runs a prune and names only its own repository', () => {
  const cmd = imageCleanupCommand('ghcr.io/example-owner/litegen', Date.parse('2026-01-15T12:00:00.000Z'));
  assert.equal(cmd.includes('prune'), false);
  assert.equal(cmd.split('\n')[1], "repo='ghcr.io/example-owner/litegen'");
  assert.equal(cmd.split('\n')[2], "cutoff='2026-01-08T12:00:00.000Z'");
  assert.equal(cmd.split('\n').at(-1), ') >/dev/null 2>&1 || true');
  assert.throws(() => imageCleanupCommand("x'; rm -rf /", 0), /odd repository/);
});

// ─── renderSiteCaddyfile ─────────────────────────────────────────────────────

const PUBLIC_SHAPE = `{
    email ops@example.com
}

www.example.com {
    redir https://example.com{uri} permanent
}

example.com {
    root * /srv/landing
    encode gzip
    # a comment that mentions /srv/landing
    try_files {path} {path}.html {path}/index.html /en.html
    file_server
}

app.example.com {
    handle_path /api/* {
        reverse_proxy litegen:4000
    }
    handle {
        root * /srv/app
        try_files {path} /index.html
        file_server
        encode gzip
    }
}
`;

const PRIVATE_SHAPE = `# Single-domain file.
{
    email ops@example.com
}

private.example.com {
    handle_path /api/* {
        reverse_proxy litegen:4000
    }
    handle {
        root * /srv/app
        try_files {path} /index.html
        file_server
        encode gzip
    }
}
`;

test('the landing + app + www shape renders exactly', () => {
  assert.equal(renderSiteCaddyfile(PUBLIC_SHAPE, shared()), `# Generated by deploy.js for shared stack example-stack. Do not edit on the host.
www.example.com {
    redir https://example.com{uri} permanent
}

example.com {
    root * /sites/example-stack/landing
    encode gzip
    try_files {path} {path}.html {path}/index.html /en.html
    file_server
}

app.example.com {
    handle_path /api/* {
        reverse_proxy example-stack-api:4000
    }
    handle {
        root * /sites/example-stack/app
        try_files {path} /index.html
        file_server
        encode gzip
    }
}
`);
});

test('the single-domain shape renders exactly, with DNS-01 when a token variable is set', () => {
  assert.equal(
    renderSiteCaddyfile(PRIVATE_SHAPE, shared({ LITEGEN_SHARED_DNS_TOKEN_ENV: 'CF_TOKEN_A' })),
    `# Generated by deploy.js for shared stack example-stack. Do not edit on the host.
private.example.com {
    tls {
        dns cloudflare {env.CF_TOKEN_A}
    }
    handle_path /api/* {
        reverse_proxy example-stack-api:4000
    }
    handle {
        root * /sites/example-stack/app
        try_files {path} /index.html
        file_server
        encode gzip
    }
}
`,
  );
});

test('every Caddyfile in the repo renders with its site addresses intact', () => {
  const files = readdirSync(DEPLOY_DIR).filter((f) => f.startsWith('Caddyfile'));
  assert.equal(files.length, 2);
  const addresses = (text) => text.split('\n').filter((l) => /^\S.*\{$/.test(l) && l !== '{');
  for (const file of files) {
    const source = readFileSync(join(DEPLOY_DIR, file), 'utf8');
    const rendered = renderSiteCaddyfile(source, shared());
    assert.deepEqual(addresses(rendered), addresses(source), file);
    assert.equal(rendered.includes('email'), false, file);
    assert.equal(rendered.includes('/srv/'), false, file);
    assert.equal(/\slitegen:4000/.test(rendered), false, file);
    assert.equal(
      rendered.split('\n').filter((l) => l.trim() === 'reverse_proxy example-stack-api:4000').length,
      source.split('\n').filter((l) => l.trim() === 'reverse_proxy litegen:4000').length,
      file,
    );
  }
});

test('anything the edge could not carry is refused, not dropped', () => {
  const render = (src) => () => renderSiteCaddyfile(src, shared());
  assert.throws(render('{\n    debug\n}\na.example.com {\n    file_server\n}\n'), /global option "debug"/);
  assert.throws(render('a.example.com {\n    root * /srv/other\n}\n'), /"\/srv\/other" has no shared-host equivalent/);
  assert.throws(render('(snip) {\n    encode gzip\n}\n'), /unsupported top-level line/);
  assert.throws(render('import other\n'), /unsupported top-level line/);
  assert.throws(render('a.example.com {\n    file_server\n'), /unbalanced braces/);
  assert.throws(render('# nothing\n'), /no site blocks/);
  assert.throws(render('a.example.com {\n    file_server\n}\n{\n    email a@example.com\n}\n'), /must come first/);
});

// ─── Generated scripts, run for real ─────────────────────────────────────────

function fakeBin(dir) {
  const bin = join(dir, 'bin');
  mkdirSync(bin, { recursive: true });
  const docker = join(bin, 'docker');
  writeFileSync(docker, `#!/bin/sh\nexec "${process.execPath}" "${FAKE_DOCKER}" "$@"\n`);
  chmodSync(docker, 0o755);
  return bin;
}

function makeTarball(dir, name, fileName, body) {
  const src = join(dir, `${name}-src`, 'dist');
  mkdirSync(src, { recursive: true });
  writeFileSync(join(src, fileName), body);
  const tgz = join(dir, `${name}.tgz`);
  execFileSync('tar', ['-C', join(dir, `${name}-src`), '-czf', tgz, 'dist']);
  return tgz;
}

function siteFixture(label, shell, { previous, failOn } = {}) {
  const dir = mkdtempSync(join(SANDBOX, `${label}-`));
  const siteDir = join(dir, 'sites', 'example-stack');
  mkdirSync(siteDir, { recursive: true });
  if (previous) {
    writeFileSync(join(siteDir, 'Caddyfile'), previous);
    for (const d of ['app', 'landing']) {
      mkdirSync(join(siteDir, d));
      writeFileSync(join(siteDir, d, 'old.html'), `old ${d}`);
    }
  }
  writeFileSync(join(siteDir, 'Caddyfile.next'), 'NEW SITE\n');
  const dashboardTgz = makeTarball(dir, 'dash', 'index.html', 'new app');
  const landingTgz = makeTarball(dir, 'land', 'en.html', 'new landing');
  const stateFile = join(dir, 'state.json');
  writeFileSync(stateFile, JSON.stringify({ calls: [], seen: [], containers: [], images: [], failOn: failOn || null }));
  const script = sitePublishScript({ ...shared(), siteDir }, { dashboardTgz, landingTgz });
  const res = spawnSync(shell, ['-c', script], {
    encoding: 'utf8',
    env: {
      PATH: `${fakeBin(dir)}:${process.env.PATH}`,
      FAKE_DOCKER_STATE: stateFile,
      FAKE_SITE_FILE: join(siteDir, 'Caddyfile'),
    },
  });
  return { res, siteDir, state: JSON.parse(readFileSync(stateFile, 'utf8')) };
}

const CADDY_ARGS = ['--adapter', 'caddyfile', '--config', '/etc/caddy/Caddyfile'];
const read = (...p) => readFileSync(join(...p), 'utf8');

for (const shell of SHELLS) {
  test(`${shell}: publish validates, then reloads, then swaps static files`, () => {
    const { res, siteDir, state } = siteFixture('ok', shell, { previous: 'OLD SITE\n' });
    assert.equal(res.status, 0, res.stderr);
    assert.deepEqual(state.calls, [
      ['exec', 'edge-caddy-1', 'caddy', 'validate', ...CADDY_ARGS],
      ['exec', 'edge-caddy-1', 'caddy', 'reload', ...CADDY_ARGS],
    ]);
    assert.deepEqual(state.seen, [
      { verb: 'validate', siteFile: 'NEW SITE\n' },
      { verb: 'reload', siteFile: 'NEW SITE\n' },
    ]);
    assert.equal(read(siteDir, 'Caddyfile'), 'NEW SITE\n');
    assert.equal(read(siteDir, 'Caddyfile.prev'), 'OLD SITE\n');
    assert.deepEqual(readdirSync(join(siteDir, 'app')), ['index.html']);
    assert.deepEqual(readdirSync(join(siteDir, 'landing')), ['en.html']);
    assert.equal(read(siteDir, 'app', 'index.html'), 'new app');
    assert.equal(read(siteDir, 'landing', 'en.html'), 'new landing');
    assert.deepEqual(readdirSync(siteDir).sort(), ['Caddyfile', 'Caddyfile.prev', 'app', 'landing']);
  });

  test(`${shell}: a failed validation restores the old site and never reloads`, () => {
    const { res, siteDir, state } = siteFixture('badvalidate', shell, { previous: 'OLD SITE\n', failOn: 'validate' });
    assert.equal(res.status, 1);
    assert.deepEqual(state.calls, [['exec', 'edge-caddy-1', 'caddy', 'validate', ...CADDY_ARGS]]);
    assert.equal(read(siteDir, 'Caddyfile'), 'OLD SITE\n');
    assert.equal(existsSync(join(siteDir, 'Caddyfile.prev')), false);
    assert.deepEqual(readdirSync(join(siteDir, 'app')), ['old.html']);
    assert.equal(read(siteDir, 'landing', 'old.html'), 'old landing');
  });

  test(`${shell}: a failed reload restores the old site and keeps its static files`, () => {
    const { res, siteDir, state } = siteFixture('badreload', shell, { previous: 'OLD SITE\n', failOn: 'reload' });
    assert.equal(res.status, 1);
    assert.equal(state.calls.length, 2);
    assert.equal(read(siteDir, 'Caddyfile'), 'OLD SITE\n');
    assert.deepEqual(readdirSync(join(siteDir, 'app')), ['old.html']);
    assert.deepEqual(readdirSync(join(siteDir, 'landing')), ['old.html']);
  });

  test(`${shell}: a failed first publish leaves no site file to break other tenants`, () => {
    const { res, siteDir } = siteFixture('badfirst', shell, { failOn: 'validate' });
    assert.equal(res.status, 1);
    assert.equal(existsSync(join(siteDir, 'Caddyfile')), false);
    assert.equal(existsSync(join(siteDir, 'app')), false);
    assert.equal(existsSync(join(siteDir, 'landing')), false);
  });
}

const REPO_NAME = 'ghcr.io/example-owner/litegen';
const OLD = '2026-01-01T00:00:00.123456789Z';
const NEW = '2026-01-14T00:00:00.123456789Z';

function cleanupRun(shell, label) {
  const dir = mkdtempSync(join(SANDBOX, `${label}-`));
  const img = (id, created, tags, digests = []) => ({ id: `sha256:${id}`, created, tags, digests });
  const initial = {
    calls: [],
    seen: [],
    failOn: null,
    containers: [
      { id: 'c-running-other-stack', image: 'sha256:inuse' },
      { id: 'c-stopped', image: 'sha256:stopped' },
      { id: 'c-unrelated', image: 'sha256:foreign' },
    ],
    images: [
      img('stale', OLD, [`${REPO_NAME}:stale`], [`${REPO_NAME}@sha256:d1`]),
      img('inuse', OLD, [`${REPO_NAME}:inuse`, `${REPO_NAME}:latest`]),
      img('stopped', OLD, [`${REPO_NAME}:stopped`]),
      img('fresh', NEW, [`${REPO_NAME}:fresh`]),
      img('cotagged', OLD, [`${REPO_NAME}:cotagged`, 'other/app:keep']),
      img('foreign', OLD, ['postgres:16-alpine']),
      img('foreignold', OLD, ['other/app:old']),
      img('prefix', OLD, [`${REPO_NAME}-extra:old`]),
    ],
  };
  const stateFile = join(dir, 'state.json');
  writeFileSync(stateFile, JSON.stringify(initial));
  const res = spawnSync(shell, ['-c', imageCleanupCommand(REPO_NAME, Date.parse('2026-01-15T12:00:00.000Z'))], {
    encoding: 'utf8',
    env: { PATH: `${fakeBin(dir)}:${process.env.PATH}`, FAKE_DOCKER_STATE: stateFile },
  });
  return { res, state: JSON.parse(readFileSync(stateFile, 'utf8')) };
}

for (const shell of SHELLS) {
  test(`${shell}: cleanup removes only this repository's old, unused images`, () => {
    const { res, state } = cleanupRun(shell, 'cleanup');
    assert.equal(res.status, 0, res.stderr);
    const left = Object.fromEntries(state.images.map((i) => [i.id.replace('sha256:', ''), [...i.tags, ...i.digests]]));
    assert.deepEqual(left, {
      inuse: [`${REPO_NAME}:inuse`, `${REPO_NAME}:latest`],
      stopped: [`${REPO_NAME}:stopped`],
      fresh: [`${REPO_NAME}:fresh`],
      cotagged: ['other/app:keep'],
      foreign: ['postgres:16-alpine'],
      foreignold: ['other/app:old'],
      prefix: [`${REPO_NAME}-extra:old`],
    });
    const removals = state.calls.filter((c) => c[0] === 'image' && c[1] === 'rm').map((c) => c.slice(2));
    assert.deepEqual(removals.flat().sort(), [
      `${REPO_NAME}:cotagged`,
      `${REPO_NAME}:stale`,
      `${REPO_NAME}@sha256:d1`,
    ]);
  });
}
