// Runs the real deploy.js with fakes only at its external edges (see
// helpers/fake-edge.cjs) and asserts the exact remote commands it sends.
// Run: npm run test:deploy
import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync,
} from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(here, '..', '..');
const FAKE_EDGE = join(here, 'helpers', 'fake-edge.cjs');

// Inside the repo's gitignored .artifacts/, never the OS temp directory.
mkdirSync(join(REPO, '.artifacts'), { recursive: true });
const SANDBOX = mkdtempSync(join(REPO, '.artifacts', 'deploy-commands-'));
after(() => rmSync(SANDBOX, { recursive: true, force: true }));

const SHA = '0123456789abcdef0123456789abcdef01234567';
const IMAGE = `ghcr.io/example-owner/litegen:${SHA}`;
const LATEST = 'ghcr.io/example-owner/litegen:latest';
const HOST = 'host.invalid';

const BASE_ENV = {
  DO_API_KEY: 'fake-do-token',
  DO_DROPLET_IP: HOST,
  GHCR_USER: 'example-owner',
  GHCR_TOKEN: 'fake-ghcr-token',
  LITEGEN__MASTER_KEY: 'fake-master-key',
  POSTGRES_PASSWORD: 'fake-db-password',
  PROXY_BUILD: 'ghcr',
  APP_URL: 'https://app.example.com',
};

let runs = 0;

// Each run gets its own copy of the deploy files, so build output and env files
// never touch the working tree.
function deploy(args, { envFile = {}, flagFile = false, scenario = {}, caddyfile = null } = {}) {
  runs += 1;
  const root = join(SANDBOX, `run-${runs}`);
  mkdirSync(join(root, 'dashboard'), { recursive: true });
  mkdirSync(join(root, 'apps', 'landing'), { recursive: true });
  cpSync(join(REPO, 'deploy.js'), join(root, 'deploy.js'));
  cpSync(join(REPO, 'deploy'), join(root, 'deploy'), { recursive: true, filter: (p) => !p.includes('/deploy/test') });

  if (caddyfile) writeFileSync(join(root, 'deploy', 'Caddyfile.example'), caddyfile);

  const name = flagFile ? '.env.deploy.example' : '.env.deploy';
  const body = Object.entries({ ...BASE_ENV, ...(caddyfile ? { LITEGEN_CADDYFILE: 'Caddyfile.example' } : {}), ...envFile }).map(([k, v]) => `${k}=${v}`).join('\n');
  writeFileSync(join(root, name), `${body}\n`);

  const log = join(root, 'edge.jsonl');
  const argv = [...args, ...(flagFile ? [`--deploy-env=${name}`] : [])];
  const res = spawnSync(process.execPath, ['--require', FAKE_EDGE, join(root, 'deploy.js'), ...argv], {
    encoding: 'utf8',
    cwd: root,
    // A clean environment: no real credential can reach the run.
    env: {
      PATH: process.env.PATH,
      HOME: root,
      __LITEGEN_REEXEC: '1',
      FAKE_EDGE_LOG: log,
      FAKE_EDGE_SCENARIO: JSON.stringify(scenario),
    },
  });
  const entries = existsSync(log)
    ? readFileSync(log, 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l))
    : [];
  return {
    res,
    root,
    entries,
    remote: entries.filter((e) => e.type === 'exec').map((e) => e.cmd),
    puts: entries.filter((e) => e.type === 'put').map((e) => e.remote),
    local: entries.filter((e) => e.type === 'local').map((e) => e.cmd),
  };
}

const composeBody = readFileSync(join(REPO, 'deploy', 'docker-compose.prod.yml'), 'utf8');
const overlayBody = readFileSync(join(REPO, 'deploy', 'docker-compose.shared.yml'), 'utf8');

const ENSURE_DOCKER = `command -v docker >/dev/null 2>&1 || (curl -fsSL https://get.docker.com | sh)
       systemctl enable --now docker 2>/dev/null || true
       docker compose version >/dev/null 2>&1 || (apt-get update -qq && apt-get install -y -qq docker-compose-plugin)`;
const GHCR_LOGIN = 'echo fake-ghcr-token | docker login ghcr.io -u example-owner --password-stdin';

const SHARED_ENV = { LITEGEN_SHARED_STACK: 'example-stack' };
const COMPOSE = 'docker compose -p example-stack --env-file .env -f docker-compose.prod.yml -f docker-compose.shared.yml';

const CLEANUP = `(
repo='ghcr.io/example-owner/litegen'
cutoff='2026-01-08T12:00:00.000Z'
containers=$(docker ps -aq) || exit 1
in_use=$(printf '%s\\n' "$containers" | xargs -r docker inspect --format '{{.Image}}') || exit 1
docker image ls --no-trunc --format '{{.ID}}' "$repo" | sort -u |
  xargs -r docker image inspect --format '{{.Id}} {{.Created}}' |
  awk -v cutoff="$cutoff" '$2 < cutoff { print $1 }' |
  while read -r id; do
    printf '%s\\n' "$in_use" | grep -qxF "$id" && continue
    docker image inspect --format '{{range .RepoTags}}{{println .}}{{end}}{{range .RepoDigests}}{{println .}}{{end}}' "$id" |
      awk -v repo="$repo" 'index($0, repo ":") == 1 || index($0, repo "@") == 1' |
      xargs -r docker image rm
  done
) >/dev/null 2>&1 || true`;

const PUBLISH = `set -eu
cd /opt/sites/example-stack
rm -rf .staging .previous
mkdir -p .staging/app .staging/landing
tar -C .staging/app --strip-components=1 -xzf /opt/example-stack/litegen-dashboard-dist.tgz
tar -C .staging/landing --strip-components=1 -xzf /opt/example-stack/litegen-landing-dist.tgz
rm -f Caddyfile.prev
if [ -f Caddyfile ]; then cp -p Caddyfile Caddyfile.prev; fi
published=0
restore() {
  if [ "$published" = 1 ]; then return 0; fi
  if [ -f Caddyfile.prev ]; then mv -f Caddyfile.prev Caddyfile; else rm -f Caddyfile; fi
}
trap restore EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
mv -f Caddyfile.next Caddyfile
docker exec edge-caddy-1 caddy validate --adapter caddyfile --config /etc/caddy/Caddyfile
docker exec edge-caddy-1 caddy reload --adapter caddyfile --config /etc/caddy/Caddyfile
published=1
mkdir .previous
for d in app landing; do
  if [ -d "$d" ]; then mv "$d" ".previous/$d"; fi
  mv ".staging/$d" "$d"
done
rm -rf .staging .previous`;

// The prod compose file is shipped unchanged, and its comments and base `ports:`
// mention the single-host layout. Everything else must be free of it.
const withoutComposeBody = (remote) => remote.map((c) => c.replace(composeBody, '<prod compose body>'));

function assertNoSingleHostResidue(rawRemote, puts) {
  const remote = withoutComposeBody(rawRemote);
  const all = [...remote, ...puts];
  assert.deepEqual(all.filter((c) => c.includes('/opt/litegen')), []);
  assert.deepEqual(all.filter((c) => c.includes('docker image prune') || c.includes('docker system prune')), []);
  assert.deepEqual(all.filter((c) => c.includes('docker-compose.web.yml')), []);
  assert.deepEqual(remote.filter((c) => c.includes('curl -sf http://127.0.0.1:4000')), []);
  assert.deepEqual(remote.filter((c) => /\b(4000|80|443):(4000|80|443)\b/.test(c)), []);
}

// ─── Single-host mode: the commands existing droplets depend on ──────────────

test('without the setting, `proxy` (ghcr) sends the exact single-host commands', () => {
  const { res, remote, puts } = deploy(['proxy']);
  assert.equal(res.status, 0, res.stderr);
  assert.deepEqual(puts, []);
  assert.deepEqual(remote, [
    ENSURE_DOCKER,
    GHCR_LOGIN,
    'mkdir -p /opt/litegen',
    `cat > /opt/litegen/docker-compose.prod.yml << 'COMPOSEEOF'\n${composeBody}\nCOMPOSEEOF`,
    `cat > /opt/litegen/.env << 'ENVEOF'\nLITEGEN_IMAGE=${IMAGE}\nLITEGEN__MASTER_KEY=fake-master-key\nPOSTGRES_PASSWORD=fake-db-password\nENVEOF\nchmod 600 /opt/litegen/.env`,
    'cd /opt/litegen && docker compose --env-file .env -f docker-compose.prod.yml pull && docker compose --env-file .env -f docker-compose.prod.yml up -d',
    'curl -sf http://127.0.0.1:4000/health/live',
    'docker image prune -af --filter "until=168h" >/dev/null 2>&1 || true',
  ]);
});

test('without the setting, `web` sends the exact single-host commands', () => {
  const { res, remote, puts } = deploy(['web']);
  assert.equal(res.status, 0, res.stderr);
  assert.deepEqual(puts, [
    '/opt/litegen/litegen-dashboard-dist.tgz',
    '/opt/litegen/litegen-landing-dist.tgz',
    '/opt/litegen/Caddyfile',
    '/opt/litegen/docker-compose.web.yml',
  ]);
  assert.deepEqual(remote, [
    'mkdir -p /opt/litegen',
    'cd /opt/litegen &&        mkdir -p dashboard-dist landing-dist &&        find dashboard-dist -mindepth 1 -delete &&        tar -C dashboard-dist --strip-components=1 -xzf litegen-dashboard-dist.tgz &&        find landing-dist -mindepth 1 -delete &&        tar -C landing-dist --strip-components=1 -xzf litegen-landing-dist.tgz &&        docker compose -f docker-compose.prod.yml -f docker-compose.web.yml --env-file .env up -d',
    'cd /opt/litegen && docker compose -f docker-compose.prod.yml -f docker-compose.web.yml ps',
  ]);
});

// ─── Shared mode ─────────────────────────────────────────────────────────────

test('shared `proxy` (ghcr) sends the exact commands and publishes nothing', () => {
  const { res, remote, puts } = deploy(['proxy'], { envFile: SHARED_ENV, flagFile: true });
  assert.equal(res.status, 0, res.stderr);
  assert.deepEqual(puts, []);
  assert.deepEqual(remote, [
    ENSURE_DOCKER,
    GHCR_LOGIN,
    'mkdir -p /opt/example-stack',
    `cat > /opt/example-stack/docker-compose.prod.yml << 'COMPOSEEOF'\n${composeBody}\nCOMPOSEEOF`,
    `cat > /opt/example-stack/docker-compose.shared.yml << 'COMPOSEEOF'\n${overlayBody}\nCOMPOSEEOF`,
    `cat > /opt/example-stack/.env << 'ENVEOF'\nLITEGEN_IMAGE=${IMAGE}\nLITEGEN__MASTER_KEY=fake-master-key\nPOSTGRES_PASSWORD=fake-db-password\nLITEGEN_EDGE_ALIAS=example-stack-api\nENVEOF\nchmod 600 /opt/example-stack/.env`,
    `cd /opt/example-stack\ncfg=$(${COMPOSE} config --no-interpolate --format json) || exit 1\ncase "$cfg" in *'"published"'*) echo 'shared stack example-stack would publish a host port; refusing to start it' >&2; exit 1;; esac`,
    `cd /opt/example-stack && ${COMPOSE} pull && ${COMPOSE} up -d`,
    `cd /opt/example-stack && ${COMPOSE} exec -T litegen wget -qO- http://127.0.0.1:4000/health/live >/dev/null`,
    CLEANUP,
  ]);
  assertNoSingleHostResidue(remote, puts);
});

test('shared `proxy` (ondroplet) builds inside the stack directory', () => {
  const { res, remote, puts, local } = deploy(['proxy'], {
    envFile: { ...SHARED_ENV, PROXY_BUILD: 'ondroplet' },
    flagFile: true,
  });
  assert.equal(res.status, 0, res.stderr);
  assert.deepEqual(puts, ['/opt/example-stack/ctx.tgz']);
  assert.equal(local.some((c) => c.includes('docker buildx')), false);
  assert.deepEqual(remote.slice(1, 9), [
    `docker image inspect ${IMAGE} >/dev/null 2>&1 && echo yes || echo no`,
    `pgrep -f "[d]ocker build -t ${IMAGE}" >/dev/null 2>&1 && echo yes || echo no`,
    "[ -f /swapfile ] || (fallocate -l 2G /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile && echo '/swapfile none swap sw 0 0' >> /etc/fstab)",
    'rm -rf /opt/example-stack/build && mkdir -p /opt/example-stack/build',
    'tar xzf /opt/example-stack/ctx.tgz -C /opt/example-stack/build',
    `cd /opt/example-stack/build && rm -f build.status build.log &&      setsid sh -c 'docker build -t ${IMAGE} -t ${LATEST} . > build.log 2>&1; echo $? > build.status' < /dev/null > /dev/null 2>&1 &`,
    'cat /opt/example-stack/build/build.status 2>/dev/null',
    'mkdir -p /opt/example-stack',
  ]);
  assert.equal(remote.includes(`cd /opt/example-stack && ${COMPOSE} up -d`), true);
  assert.deepEqual(withoutComposeBody(remote).filter((c) => c.includes(' pull')), []);
  assertNoSingleHostResidue(remote, puts);
});

test('shared `proxy` aborts with the stack logs when the health check fails', () => {
  const { res, remote } = deploy(['proxy'], { envFile: SHARED_ENV, flagFile: true, scenario: { healthFails: true } });
  assert.equal(res.status, 1);
  assert.match(res.stderr, /proxy failed its \/health\/live check/);
  assert.equal(remote.filter((c) => c.endsWith('/health/live >/dev/null')).length, 40);
  assert.equal(remote.at(-1), `cd /opt/example-stack && ${COMPOSE} logs --tail 80`);
});

test('shared `web` publishes to the site directory with no compose and no Caddy service', () => {
  const { res, remote, puts } = deploy(['web'], {
    envFile: { ...SHARED_ENV, LITEGEN_SHARED_DNS_TOKEN_ENV: 'CF_TOKEN_A' },
    flagFile: true,
    caddyfile: `{
    email ops@example.com
}

app.example.com {
    handle_path /api/* {
        reverse_proxy litegen:4000
    }
    handle {
        root * /srv/app
        try_files {path} /index.html
        file_server
    }
}

example.com {
    root * /srv/landing
    file_server
}
`,
  });
  assert.equal(res.status, 0, res.stderr);
  assert.deepEqual(puts, [
    '/opt/example-stack/litegen-dashboard-dist.tgz',
    '/opt/example-stack/litegen-landing-dist.tgz',
  ]);
  assert.deepEqual(remote, [
    'mkdir -p /opt/example-stack /opt/sites/example-stack',
    `cat > /opt/sites/example-stack/Caddyfile.next << 'SITEEOF'
# Generated by deploy.js for shared stack example-stack. Do not edit on the host.
app.example.com {
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
    }
}

example.com {
    tls {
        dns cloudflare {env.CF_TOKEN_A}
    }
    root * /sites/example-stack/landing
    file_server
}
SITEEOF`,
    PUBLISH,
  ]);
  assertNoSingleHostResidue(remote, puts);
});

test('two stacks on one host share no directory, project or alias', () => {
  const a = deploy(['proxy'], { envFile: { LITEGEN_SHARED_STACK: 'stack-a' }, flagFile: true });
  const b = deploy(['proxy'], { envFile: { LITEGEN_SHARED_STACK: 'stack-b' }, flagFile: true });
  assert.equal(a.res.status, 0, a.res.stderr);
  assert.equal(b.res.status, 0, b.res.stderr);
  const up = (s) => {
    const compose = `docker compose -p ${s} --env-file .env -f docker-compose.prod.yml -f docker-compose.shared.yml`;
    return `cd /opt/${s} && ${compose} pull && ${compose} up -d`;
  };
  assert.equal(a.remote.filter((c) => c === up('stack-a')).length, 1);
  assert.equal(b.remote.filter((c) => c === up('stack-b')).length, 1);
  // Everything except the image cleanup and generic host setup names only its stack.
  const stackLines = (r) => r.remote.filter((c) => c.includes('/opt/'));
  assert.deepEqual(stackLines(a).filter((c) => c.includes('stack-b')), []);
  assert.deepEqual(stackLines(b).filter((c) => c.includes('stack-a')), []);
  assert.equal(a.remote.some((c) => c.includes('LITEGEN_EDGE_ALIAS=stack-a-api')), true);
  assert.equal(b.remote.some((c) => c.includes('LITEGEN_EDGE_ALIAS=stack-b-api')), true);
});

test('shared mode refuses `provision`, which would create and bill a new droplet', () => {
  const { res, entries } = deploy(['provision'], { envFile: SHARED_ENV, flagFile: true });
  assert.equal(res.status, 1);
  assert.match(res.stderr, /shared stack example-stack runs on an existing host/);
  assert.deepEqual(entries, []);
});

test('shared `all` skips provisioning and still runs proxy, web and landing', () => {
  const { res, entries, remote, local } = deploy(['all'], {
    envFile: { ...SHARED_ENV, CLOUDFLARE_API_TOKEN: 'fake-cf-token', CLOUDFLARE_ACCOUNT_ID: 'fake-cf-account' },
    flagFile: true,
  });
  assert.equal(res.status, 0, res.stderr);
  assert.equal(res.stdout.includes('Provisioning DigitalOcean droplet'), false);
  assert.deepEqual([...new Set(entries.filter((e) => e.type === 'connect').map((e) => e.host))], [HOST]);
  assert.equal(remote[0], ENSURE_DOCKER);
  assert.equal(remote.at(-1), PUBLISH);
  assert.equal(
    local.at(-1),
    'npx --yes wrangler pages deploy apps/landing/out --project-name=litegen-landing --branch=main --commit-dirty=true',
  );
});

test('an invalid stack name stops the deploy before anything runs', () => {
  const { res, entries } = deploy(['proxy'], { envFile: { LITEGEN_SHARED_STACK: 'Bad_Name' }, flagFile: true });
  assert.equal(res.status, 1);
  assert.match(res.stderr, /not a valid stack name/);
  assert.deepEqual(entries, []);
});

test('a --deploy-env file that does not exist stops the deploy', () => {
  const root = join(SANDBOX, 'missing-env');
  mkdirSync(root, { recursive: true });
  cpSync(join(REPO, 'deploy.js'), join(root, 'deploy.js'));
  cpSync(join(REPO, 'deploy'), join(root, 'deploy'), { recursive: true, filter: (p) => !p.includes('/deploy/test') });
  const log = join(root, 'edge.jsonl');
  const res = spawnSync(process.execPath, ['--require', FAKE_EDGE, join(root, 'deploy.js'), 'proxy', '--deploy-env=.env.deploy.nope'], {
    encoding: 'utf8',
    env: { PATH: process.env.PATH, HOME: root, __LITEGEN_REEXEC: '1', FAKE_EDGE_LOG: log },
  });
  assert.equal(res.status, 1);
  assert.match(res.stderr, /Deploy env file not found: .*\.env\.deploy\.nope/);
  assert.equal(existsSync(log), false);
});
