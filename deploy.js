#!/usr/bin/env node
'use strict';

/**
 * LiteGen hosted deploy.
 *
 *   node deploy.js provision   Create/reuse the DigitalOcean droplet (proxy + db).
 *   node deploy.js proxy       Build amd64 image -> push GHCR -> droplet pulls + runs.
 *   node deploy.js web         Build dashboard + landing -> ship dist + Caddyfile -> compose up Caddy web tier.
 *   node deploy.js landing     Static-export the Next.js site -> Cloudflare Pages.
 *   node deploy.js all         provision -> proxy -> web -> landing  (full redeploy).
 *
 * Flags:  --dry-run   Print intended actions; make no real API/SSH/registry calls.
 *
 * Config comes from .env.deploy (gitignored). See .env.deploy.template.
 */

const { execSync, spawnSync } = require('child_process');
const { NodeSSH } = require('node-ssh');
const path = require('path');
const fs = require('fs');
const net = require('net');

const ROOT = __dirname;
const ENV_FILE = path.resolve(ROOT, '.env.deploy');
const DRY_RUN = process.argv.includes('--dry-run');

// This machine trusts a system/corporate CA that Node's bundled store lacks,
// so global fetch() (the DigitalOcean API) can fail with "fetch failed".
// Re-exec ourselves with --use-system-ca so fetch trusts the OS trust store.
// (The wrangler call below needs the same flag — see deployLanding.)
if (!process.execArgv.includes('--use-system-ca') && !process.env.__LITEGEN_REEXEC) {
  const r = spawnSync(process.execPath, ['--use-system-ca', __filename, ...process.argv.slice(2)], {
    stdio: 'inherit',
    env: { ...process.env, __LITEGEN_REEXEC: '1' },
  });
  process.exit(r.status === null ? 1 : r.status);
}

// ── Load .env.deploy (real env always wins) ──────────────────────────────
if (fs.existsSync(ENV_FILE)) {
  for (const line of fs.readFileSync(ENV_FILE, 'utf8').split('\n')) {
    const t = line.trim();
    if (!t || t.startsWith('#')) continue;
    const eq = t.indexOf('=');
    if (eq === -1) continue;
    const key = t.slice(0, eq).trim();
    const val = t.slice(eq + 1).trim().replace(/^["']|["']$/g, '');
    if (!process.env[key]) process.env[key] = val;
  }
}

// ── Helpers ───────────────────────────────────────────────────────────────
function env(key) { return process.env[key] || ''; }

function required(key) {
  const v = env(key);
  if (!v) { console.error(`✖ Missing required env var: ${key} (set it in .env.deploy)`); process.exit(1); }
  return v;
}

function resolvePath(p) {
  if (!p) return p;
  if (p.startsWith('~')) return path.join(process.env.HOME, p.slice(1));
  return path.resolve(p);
}

const NODE_BIN_DIR = path.dirname(process.execPath);

function run(cmd, opts = {}) {
  console.log(`  $ ${cmd}`);
  if (DRY_RUN) return '';
  const mergedEnv = { ...process.env, ...(opts.env || {}) };
  mergedEnv.PATH = `${NODE_BIN_DIR}:${mergedEnv.PATH || ''}`;
  return execSync(cmd, { cwd: ROOT, stdio: 'inherit', ...opts, env: mergedEnv });
}

function runCapture(cmd) {
  return execSync(cmd, { cwd: ROOT, encoding: 'utf8' }).trim();
}

// Log in to a registry WITHOUT printing the token (run() would echo it to logs).
function dockerLogin(registry, user, token) {
  console.log(`  $ docker login ${registry} -u ${user} --password-stdin  (token hidden)`);
  if (DRY_RUN) return;
  execSync(`docker login ${registry} -u ${user} --password-stdin`, {
    input: token,
    stdio: ['pipe', 'inherit', 'inherit'],
    env: { ...process.env, PATH: `${NODE_BIN_DIR}:${process.env.PATH || ''}` },
  });
}

// Persist (update-or-append) a single KEY=value line in .env.deploy so re-runs
// are stable (droplet IP, generated secrets).
function persistEnvVar(key, value) {
  process.env[key] = value;
  let lines = fs.existsSync(ENV_FILE) ? fs.readFileSync(ENV_FILE, 'utf8').split('\n') : [];
  let found = false;
  lines = lines.map((line) => {
    const t = line.trim();
    if (t.startsWith('#') || !t.includes('=')) return line;
    if (t.slice(0, t.indexOf('=')).trim() === key) { found = true; return `${key}=${value}`; }
    return line;
  });
  if (!found) {
    if (lines.length && lines[lines.length - 1] === '') lines.splice(lines.length - 1, 1);
    lines.push(`${key}=${value}`);
  }
  fs.writeFileSync(ENV_FILE, lines.join('\n').replace(/\n*$/, '\n'), { mode: 0o600 });
}

let _keychainLoaded = false;
async function sshConnect(host) {
  const ssh = new NodeSSH();
  const keyPath = resolvePath(env('DO_SSH_KEY_PATH') || path.join(process.env.HOME, '.ssh', 'id_ed25519'));
  const cfg = {
    host,
    username: process.env.DO_SSH_USER || 'root',
    keepaliveInterval: 20_000,
    keepaliveCountMax: 30,
  };
  // node-ssh can't parse a passphrase-encrypted key without the passphrase.
  // Prefer the ssh-agent; on macOS, load the key from the keychain into the
  // agent first (idempotent; no-op off macOS or if already loaded).
  if (process.env.SSH_AUTH_SOCK) {
    if (process.platform === 'darwin' && !_keychainLoaded) {
      try { execSync('ssh-add --apple-load-keychain 2>/dev/null'); } catch { /* ignore */ }
      _keychainLoaded = true;
    }
    cfg.agent = process.env.SSH_AUTH_SOCK;
  }
  if (env('DO_SSH_PASSPHRASE')) {
    cfg.privateKeyPath = keyPath;
    cfg.passphrase = env('DO_SSH_PASSPHRASE');
  } else if (!process.env.SSH_AUTH_SOCK) {
    cfg.privateKeyPath = keyPath; // assume unencrypted when no agent/passphrase
  }
  await ssh.connect(cfg);
  return ssh;
}

async function sshExec(ssh, cmd, label) {
  console.log(`  [ssh] ${label || cmd.split('\n')[0]}`);
  if (DRY_RUN) return { code: 0, stdout: '', stderr: '' };
  const result = await ssh.execCommand(cmd);
  if (result.stdout) console.log(result.stdout);
  if (result.stderr) console.error(result.stderr);
  if (result.code !== 0 && result.code !== null) {
    throw new Error(`SSH command failed (exit ${result.code}): ${label || cmd}`);
  }
  return result;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ── Fail-loud guard: no localhost dev URLs leaked into a built bundle ─────
function assertNoLocalhostInBundle(dir, label) {
  console.log(`\n=== Guard: scanning ${label} build output for localhost dev URLs ===`);
  const abs = path.resolve(ROOT, dir);
  if (!fs.existsSync(abs)) {
    throw new Error(`${label}: build output ${dir} does not exist — the build did not produce it. Refusing to deploy.`);
  }
  // Capture each localhost occurrence with leading context. We FAIL on a real
  // wired dev URL, but ALLOW a `??`/`||` fallback default (e.g. the vendored SDK's
  // `baseUrl ?? "http://localhost:4000"`) that the app always overrides — those
  // never become the effective endpoint and would otherwise block every deploy.
  let raw = '';
  try {
    raw = execSync(`grep -rIohE '.{0,24}localhost:[0-9]+' ${JSON.stringify(abs)}`, { encoding: 'utf8' });
  } catch (err) {
    if (err.status === 1) raw = '';
    else throw new Error(`${label}: localhost guard grep failed (exit ${err.status}): ${err.message}`);
  }
  const offenders = raw
    .split('\n')
    .map((s) => s.trim())
    .filter(Boolean)
    .filter((occ) => {
      const prefix = occ.slice(0, occ.indexOf('localhost'));
      // Allowed iff it's a fallback default (preceded by `??` or `||`).
      return !/\?\?|\|\|/.test(prefix);
    });
  if (offenders.length) {
    console.error(`\n✖ ${label}: build leaked a localhost dev URL:\n  ${offenders.join('\n  ')}`);
    throw new Error(`${label}: build output references http://localhost:<port> as a wired endpoint. Refusing to deploy a broken bundle.`);
  }
  console.log(`  OK — no wired localhost dev URLs in ${label} output (fallback defaults ignored).`);
}

// ── DigitalOcean API ──────────────────────────────────────────────────────
const DO_API = 'https://api.digitalocean.com/v2';

async function doApi(method, endpoint, body) {
  const token = required('DO_API_KEY');
  if (DRY_RUN) { console.log(`  [DO ${method}] ${endpoint}${body ? ' ' + JSON.stringify(body) : ''}`); return {}; }
  const res = await fetch(`${DO_API}${endpoint}`, {
    method,
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  const json = text ? JSON.parse(text) : {};
  if (!res.ok) throw new Error(`DO ${method} ${endpoint} -> ${res.status}: ${json.message || text}`);
  return json;
}

// Upload the local public key to the DO account if absent; return its fingerprint.
async function ensureSshKey() {
  const keyPath = resolvePath(env('DO_SSH_KEY_PATH') || path.join(process.env.HOME, '.ssh', 'id_ed25519'));
  const pubPath = `${keyPath}.pub`;
  if (!fs.existsSync(pubPath)) throw new Error(`Public key not found: ${pubPath}`);
  const pub = fs.readFileSync(pubPath, 'utf8').trim();
  const pubCore = pub.split(/\s+/).slice(0, 2).join(' '); // type + base64 (ignore comment)

  if (DRY_RUN) { console.log(`  [DO] ensure SSH key from ${pubPath}`); return 'dry-run-fingerprint'; }

  const { ssh_keys = [] } = await doApi('GET', '/account/keys?per_page=200');
  const existing = ssh_keys.find((k) => (k.public_key || '').split(/\s+/).slice(0, 2).join(' ') === pubCore);
  if (existing) { console.log(`  SSH key already in DO account (fingerprint ${existing.fingerprint}).`); return existing.fingerprint; }

  const created = await doApi('POST', '/account/keys', { name: `litegen-deploy-${Date.now()}`, public_key: pub });
  console.log(`  Uploaded SSH key to DO account (fingerprint ${created.ssh_key.fingerprint}).`);
  return created.ssh_key.fingerprint;
}

function publicIpv4(droplet) {
  const v4 = (droplet.networks && droplet.networks.v4) || [];
  const pub = v4.find((n) => n.type === 'public');
  return pub ? pub.ip_address : '';
}

async function waitForSsh(host, tries = 40) {
  process.stdout.write('  Waiting for SSH (port 22)');
  for (let i = 0; i < tries; i++) {
    const ok = await new Promise((resolve) => {
      const sock = net.connect({ host, port: 22, timeout: 4000 });
      sock.on('connect', () => { sock.destroy(); resolve(true); });
      sock.on('error', () => resolve(false));
      sock.on('timeout', () => { sock.destroy(); resolve(false); });
    });
    if (ok) { process.stdout.write(' up!\n'); return; }
    process.stdout.write('.');
    await sleep(5000);
  }
  process.stdout.write('\n');
  throw new Error(`SSH never came up on ${host}:22`);
}

// ── Target: provision ──────────────────────────────────────────────────────
const DROPLET_NAME = 'litegen-proxy';
const DROPLET_TAG = 'litegen';

async function provision() {
  required('DO_API_KEY');
  console.log('\n=== Provisioning DigitalOcean droplet ===');

  // Idempotent: reuse an existing tagged droplet instead of creating a second.
  if (!DRY_RUN) {
    const { droplets = [] } = await doApi('GET', `/droplets?tag_name=${DROPLET_TAG}&per_page=200`);
    const existing = droplets.find((d) => d.name === DROPLET_NAME);
    if (existing) {
      const ip = publicIpv4(existing);
      console.log(`  Reusing existing droplet ${existing.id} (${ip || 'no public IP yet'}).`);
      if (ip) { persistEnvVar('DO_DROPLET_IP', ip); await waitForSsh(ip); }
      return ip;
    }
  }

  const fingerprint = await ensureSshKey();
  const region = env('DO_REGION') || 'nyc3';
  const size = env('DO_SIZE') || 's-1vcpu-2gb';
  const image = env('DO_IMAGE') || 'ubuntu-24-04-x64';
  console.log(`  Creating ${size} / ${region} / ${image} ...`);

  const { droplet } = await doApi('POST', '/droplets', {
    name: DROPLET_NAME,
    region, size, image,
    ssh_keys: [fingerprint],
    tags: [DROPLET_TAG],
    monitoring: true,
    ipv6: false,
  });
  if (DRY_RUN) { console.log('  [dry-run] would poll for active droplet + IP.'); return ''; }

  // Poll until active with a public IP.
  let ip = '';
  for (let i = 0; i < 60; i++) {
    const { droplet: d } = await doApi('GET', `/droplets/${droplet.id}`);
    ip = publicIpv4(d);
    if (d.status === 'active' && ip) break;
    process.stdout.write(i === 0 ? '  Waiting for droplet to become active.' : '.');
    await sleep(5000);
  }
  process.stdout.write('\n');
  if (!ip) throw new Error('Droplet never reported a public IPv4 address.');
  console.log(`  Droplet active at ${ip}.`);
  persistEnvVar('DO_DROPLET_IP', ip);
  await waitForSsh(ip);
  return ip;
}

// ── Target: proxy (build image, push GHCR, deploy compose on droplet) ──────
function ghcrImageRef() {
  const repoOwner = (env('GHCR_USER') || runCapture('git remote get-url origin').match(/github\.com[:/]([^/]+)/)[1]).toLowerCase();
  return `ghcr.io/${repoOwner}/litegen`;
}

async function deployProxy() {
  const host = required('DO_DROPLET_IP');
  required('LITEGEN__MASTER_KEY');
  required('POSTGRES_PASSWORD');

  const sha = runCapture('git rev-parse HEAD');
  const image = ghcrImageRef();
  const imageSha = `${image}:${sha}`;
  const imageLatest = `${image}:latest`;
  const ghUser = env('GHCR_USER') || runCapture('git remote get-url origin').match(/github\.com[:/]([^/]+)/)[1];
  // 'ghcr'      → build+push locally, droplet pulls. Needs clean TLS to registries
  //               + crates.io (fails behind a TLS-intercepting corporate proxy).
  // 'ondroplet' → build natively on the droplet (clean network, native amd64).
  const mode = env('PROXY_BUILD') || 'ghcr';

  if (mode === 'ghcr') {
    const ghToken = required('GHCR_TOKEN');
    console.log(`\n=== Building + pushing proxy image ${imageSha} (mode: ghcr) ===`);
    // Cross-build for the amd64 droplet (Mac is arm64). docker-container driver
    // is required for --push.
    run(`docker buildx inspect litegen-builder >/dev/null 2>&1 || docker buildx create --name litegen-builder --driver docker-container --bootstrap`);
    dockerLogin('ghcr.io', ghUser, ghToken);
    run(`docker buildx build --builder litegen-builder --platform linux/amd64 --push -t ${imageSha} -t ${imageLatest} .`);
  }

  console.log(`\n=== Deploying proxy + db to ${host} (build mode: ${mode}) ===`);
  let ssh = DRY_RUN ? null : await sshConnect(host);
  try {
    await sshExec(ssh,
      `command -v docker >/dev/null 2>&1 || (curl -fsSL https://get.docker.com | sh)
       systemctl enable --now docker 2>/dev/null || true
       docker compose version >/dev/null 2>&1 || (apt-get update -qq && apt-get install -y -qq docker-compose-plugin)`,
      'ensure docker + compose');

    if (mode === 'ondroplet') {
      await buildOnDroplet(ssh, host, imageSha, imageLatest);
      // The detached build outlives the SSH channel; reconnect fresh for compose.
      if (!DRY_RUN) { try { ssh.dispose(); } catch { /* ignore */ } ssh = await sshConnect(host); }
    } else {
      // Droplet must authenticate to pull the (private) GHCR image.
      const ghToken = required('GHCR_TOKEN');
      await sshExec(ssh, `echo ${ghToken} | docker login ghcr.io -u ${ghUser} --password-stdin`, 'ghcr login (droplet)');
    }

    await sshExec(ssh, 'mkdir -p /opt/litegen', 'mkdir /opt/litegen');

    // Compose file (quoted heredoc — no shell expansion of ${...}).
    const composeBody = fs.readFileSync(path.join(ROOT, 'deploy', 'docker-compose.prod.yml'), 'utf8');
    await sshExec(ssh, `cat > /opt/litegen/docker-compose.prod.yml << 'COMPOSEEOF'\n${composeBody}\nCOMPOSEEOF`, 'write compose');

    // Substitution .env beside the compose. Provider keys passed through if set.
    const envLines = [
      `LITEGEN_IMAGE=${imageSha}`,
      `LITEGEN__MASTER_KEY=${env('LITEGEN__MASTER_KEY')}`,
      `POSTGRES_PASSWORD=${env('POSTGRES_PASSWORD')}`,
    ];
    for (const k of [
      'LITEGEN_CORS_ORIGINS',
      // Hosted multi-tenant mode: open signup creates orgs, master key is platform-admin,
      // SECRETS_KEY enables per-app BYO provider-credential encryption. COOKIE_INSECURE_DEV
      // lets session cookies work over plain http until TLS is fronted.
      'LITEGEN__MODE', 'LITEGEN__SECRETS_KEY', 'LITEGEN__COOKIE_INSECURE_DEV',
      // OAuth-only auth (Google + GitHub; no password). CALLBACK_BASE includes the
      // /api prefix so the constructed redirect_uri is https://app.litegen.ai/api/auth/redirect.
      'LITEGEN__AUTH__ALLOW_PASSWORD', 'LITEGEN__OAUTH__CALLBACK_BASE',
      'LITEGEN__OAUTH__GOOGLE__CLIENT_ID', 'LITEGEN__OAUTH__GOOGLE__CLIENT_SECRET',
      'LITEGEN__OAUTH__GITHUB__CLIENT_ID', 'LITEGEN__OAUTH__GITHUB__CLIENT_SECRET',
      // Object storage: Scaleway S3 (S3-compatible) for generated-image artifacts.
      'LITEGEN__IMAGE_STORAGE__BACKEND', 'LITEGEN_S3_BUCKET', 'LITEGEN_S3_REGION',
      'LITEGEN_S3_ENDPOINT_URL', 'LITEGEN_S3_ACCESS_KEY_ID', 'LITEGEN_S3_SECRET_ACCESS_KEY',
      'OPENAI_API_KEY', 'REPLICATE_API_TOKEN', 'GOOGLE_API_KEY', 'FAL_KEY',
    ]) {
      if (env(k)) envLines.push(`${k}=${env(k)}`);
    }
    await sshExec(ssh,
      `cat > /opt/litegen/.env << 'ENVEOF'\n${envLines.join('\n')}\nENVEOF\nchmod 600 /opt/litegen/.env`,
      'write droplet .env');

    // ondroplet: image is already local — skip the registry pull.
    const pullStep = mode === 'ghcr'
      ? 'docker compose --env-file .env -f docker-compose.prod.yml pull && '
      : '';
    await sshExec(ssh,
      `cd /opt/litegen && ${pullStep}docker compose --env-file .env -f docker-compose.prod.yml up -d`,
      'compose up');

    // Health check.
    console.log('\n  Waiting for proxy health (/health/live)...');
    let healthy = false;
    if (!DRY_RUN) {
      for (let i = 0; i < 40; i++) {
        const res = await ssh.execCommand('curl -sf http://127.0.0.1:4000/health/live');
        if (res.code === 0) { healthy = true; break; }
        await sleep(3000);
      }
      if (!healthy) {
        console.warn('  Health check did not pass within 120s — dumping logs:');
        await sshExec(ssh, 'cd /opt/litegen && docker compose -f docker-compose.prod.yml logs --tail 80', 'compose logs');
      } else {
        console.log('  Proxy is healthy.');
      }
    }

    await sshExec(ssh, 'docker image prune -af --filter "until=168h" >/dev/null 2>&1 || true', 'prune old images');
    console.log(`\n  Proxy live at http://${host}:4000  (Authorization: Bearer <LITEGEN__MASTER_KEY>)`);
  } finally {
    if (ssh) ssh.dispose();
  }
}

// Read a remote file via a FRESH short-lived SSH connection (resilient to the
// long-build connection dropping). Returns the contents, or null if the file
// is absent or the connection fails transiently.
async function readRemoteFile(host, remotePath) {
  let ssh;
  try {
    ssh = await sshConnect(host);
    const res = await ssh.execCommand(`cat ${remotePath} 2>/dev/null`);
    return res.code === 0 ? res.stdout : null;
  } catch {
    return null; // transient failure — caller keeps polling
  } finally {
    if (ssh) { try { ssh.dispose(); } catch { /* ignore */ } }
  }
}

// Build the image natively on the droplet (clean network, native amd64). Used
// when local egress to registries/crates.io is intercepted by a corporate TLS
// proxy, or to skip slow QEMU cross-builds.
//
// The compile can run 10-20 min on a small box — far longer than a flaky
// SSH channel reliably survives. So we (a) skip entirely if the image already
// exists, and (b) run the build DETACHED (setsid + status file) and poll with
// fresh connections, so a dropped channel can't kill or stall the build.
async function buildOnDroplet(ssh, host, imageSha, imageLatest) {
  // Idempotent: a prior (even crashed) run may have finished the image.
  if (!DRY_RUN) {
    const have = await ssh.execCommand(`docker image inspect ${imageSha} >/dev/null 2>&1 && echo yes || echo no`);
    if (have.stdout.trim() === 'yes') { console.log('  Image already built on droplet — skipping build.'); return; }
    // If a build is already in progress (e.g. an orphan from a dropped run),
    // adopt it: poll for the image instead of starting a competing build.
    // The `[d]ocker` char-class keeps this pattern from matching the shell that
    // runs the pgrep itself (whose argv literally contains the pattern string),
    // which would otherwise always report a phantom "build already running".
    const running = await ssh.execCommand(`pgrep -f "[d]ocker build -t ${imageSha}" >/dev/null 2>&1 && echo yes || echo no`);
    if (running.stdout.trim() === 'yes') {
      console.log('  A build for this image is already running on the droplet — waiting for it.');
      return await waitForImage(host, ssh, imageSha);
    }
  }

  console.log('  Building image on the droplet (native amd64)...');
  // 2G swap so the Rust release build doesn't OOM the 2GB box.
  await sshExec(ssh,
    `[ -f /swapfile ] || (fallocate -l 2G /swapfile && chmod 600 /swapfile && mkswap /swapfile && swapon /swapfile && echo '/swapfile none swap sw 0 0' >> /etc/fstab)`,
    'ensure 2G swap');

  // Pack only what the Dockerfile COPYs (never litegen-core/target/).
  const ctx = '/tmp/litegen-ctx.tgz';
  run(`COPYFILE_DISABLE=1 tar czf ${ctx} -C ${JSON.stringify(ROOT)} Dockerfile litegen-core/Cargo.toml litegen-core/Cargo.lock litegen-core/src litegen-core/migrations models`);
  await sshExec(ssh, 'rm -rf /opt/litegen/build && mkdir -p /opt/litegen/build', 'reset build dir');
  console.log('  [scp] uploading build context...');
  if (!DRY_RUN) { await ssh.putFile(ctx, '/opt/litegen/ctx.tgz'); fs.unlinkSync(ctx); }
  await sshExec(ssh, 'tar xzf /opt/litegen/ctx.tgz -C /opt/litegen/build', 'extract build context');

  // Detached build (survives SSH drops); status + log land in the build dir.
  await sshExec(ssh,
    `cd /opt/litegen/build && rm -f build.status build.log && \
     setsid sh -c 'docker build -t ${imageSha} -t ${imageLatest} . > build.log 2>&1; echo $? > build.status' < /dev/null > /dev/null 2>&1 &`,
    'start detached build');
  if (DRY_RUN) return;
  await waitForImage(host, ssh, imageSha);
}

// True if the image exists on the droplet (checked over a fresh connection).
async function remoteImageExists(host, imageSha) {
  let ssh;
  try {
    ssh = await sshConnect(host);
    const r = await ssh.execCommand(`docker image inspect ${imageSha} >/dev/null 2>&1 && echo yes || echo no`);
    return r.stdout.trim() === 'yes';
  } catch {
    return false;
  } finally {
    if (ssh) { try { ssh.dispose(); } catch { /* ignore */ } }
  }
}

// Poll (with fresh connections) until the detached build's status file reports
// success, or the image otherwise appears (covers an adopted orphan build that
// has no status file). Throws with the build-log tail on failure/timeout.
async function waitForImage(host, ssh, imageSha) {
  console.log('  Building on droplet (polling every 15s, up to ~45m)...');
  for (let i = 0; i < 180; i++) {
    await sleep(15000);
    const status = await readRemoteFile(host, '/opt/litegen/build/build.status');
    if (status !== null && status.trim() !== '') {
      if (status.trim() === '0') { console.log('  On-droplet build complete.'); return; }
      const log = (await readRemoteFile(host, '/opt/litegen/build/build.log')) || '';
      throw new Error(`On-droplet build failed (exit ${status.trim()}). Last lines:\n` + log.split('\n').slice(-40).join('\n'));
    }
    if (await remoteImageExists(host, imageSha)) { console.log('  Image is present on droplet.'); return; }
    if (i % 4 === 0) process.stdout.write(`  ...still building (${(i + 1) * 15}s elapsed)\n`);
  }
  throw new Error('On-droplet build timed out after ~45 minutes.');
}

// ── Target: web (build dashboard + landing -> ship dist + Caddyfile -> compose up web tier) ─
//
// Caddy (the `web` service) terminates TLS via automatic Let's Encrypt and serves
//   litegen.ai      → the static Next.js landing export   (/srv/landing)
//   app.litegen.ai  → the dashboard SPA (/srv/app) + the API reverse-proxied under /api
async function deployWeb() {
  const host = required('DO_DROPLET_IP');
  const appUrl = env('APP_URL') || 'https://app.litegen.ai';
  const apiBase = `${appUrl}/api`;

  const dashboardDir = path.join(ROOT, 'dashboard');
  const dashboardTgz = '/tmp/litegen-dashboard-dist.tgz';
  const remoteDashboardTgz = '/opt/litegen/litegen-dashboard-dist.tgz';

  const landingDir = path.join(ROOT, 'apps', 'landing');
  const landingOut = path.join(landingDir, 'out');
  const landingFallbackDir = '/tmp/litegen-landing-fallback';
  const landingTgz = '/tmp/litegen-landing-dist.tgz';
  const remoteLandingTgz = '/opt/litegen/litegen-landing-dist.tgz';

  // ── Build dashboard (Vite SPA → dashboard/dist) ──
  console.log(`\n=== Building dashboard (VITE_API_URL=${apiBase}) ===`);
  if (!DRY_RUN) {
    run(`npm run build`, {
      cwd: dashboardDir,
      env: { ...process.env, VITE_API_URL: apiBase },
    });
    assertNoLocalhostInBundle('dashboard/dist', 'dashboard');
  } else {
    console.log('  [dry-run] skipping dashboard build.');
  }

  // ── Build landing (Next.js static export → apps/landing/out) ──
  // If the landing build fails (e.g. due to in-progress edits to apps/landing),
  // don't abort the whole web deploy — ship a minimal valid placeholder so
  // litegen.ai still returns valid HTML.
  let landingSrcDir = landingOut; // what we tar up; flips to the fallback on failure
  console.log('\n=== Building landing (Next.js static export → apps/landing/out) ===');
  if (!DRY_RUN) {
    try {
      run(`npm run build`, {
        cwd: landingDir,
        env: { ...process.env, NODE_ENV: 'production' },
      });
      if (!fs.existsSync(landingOut)) {
        throw new Error(`landing build produced no ${landingOut} directory`);
      }
      assertNoLocalhostInBundle('apps/landing/out', 'landing');
      console.log('  Landing build OK.');
    } catch (err) {
      console.warn(`\n  ⚠ Landing build FAILED: ${err.message}`);
      console.warn('  ⚠ Shipping a minimal placeholder index.html so litegen.ai still serves valid HTML.');
      fs.rmSync(landingFallbackDir, { recursive: true, force: true });
      fs.mkdirSync(landingFallbackDir, { recursive: true });
      fs.writeFileSync(
        path.join(landingFallbackDir, 'index.html'),
        '<!doctype html><html><head><meta charset=utf-8><title>LiteGen</title></head><body><h1>LiteGen</h1><p>Coming soon.</p></body></html>\n',
      );
      landingSrcDir = landingFallbackDir;
    }
  } else {
    console.log('  [dry-run] skipping landing build.');
  }

  // ── Package both dists separately ──
  console.log('\n=== Packaging dashboard dist ===');
  if (!DRY_RUN) {
    run(`COPYFILE_DISABLE=1 tar -C ${JSON.stringify(dashboardDir)} -czf ${dashboardTgz} dist`);
  }

  console.log('\n=== Packaging landing dist ===');
  if (!DRY_RUN) {
    // Tar the chosen source dir's contents under a single top-level component so
    // the remote `--strip-components=1` extraction lands the files at the root.
    const landingBase = path.basename(landingSrcDir);
    run(`COPYFILE_DISABLE=1 tar -C ${JSON.stringify(path.dirname(landingSrcDir))} -czf ${landingTgz} ${JSON.stringify(landingBase)}`);
  }

  console.log(`\n=== Shipping dashboard-dist + landing-dist + Caddyfile + web compose to ${host} ===`);
  let ssh = DRY_RUN ? null : await sshConnect(host);
  try {
    await sshExec(ssh, 'mkdir -p /opt/litegen', 'mkdir /opt/litegen');

    console.log(`  [scp] ${dashboardTgz} -> ${remoteDashboardTgz}`);
    if (!DRY_RUN) { await ssh.putFile(dashboardTgz, remoteDashboardTgz); }

    console.log(`  [scp] ${landingTgz} -> ${remoteLandingTgz}`);
    if (!DRY_RUN) { await ssh.putFile(landingTgz, remoteLandingTgz); }

    const caddyfile = path.join(ROOT, 'deploy', 'Caddyfile');
    console.log(`  [scp] ${caddyfile} -> /opt/litegen/Caddyfile`);
    if (!DRY_RUN) { await ssh.putFile(caddyfile, '/opt/litegen/Caddyfile'); }

    const webCompose = path.join(ROOT, 'deploy', 'docker-compose.web.yml');
    console.log(`  [scp] ${webCompose} -> /opt/litegen/docker-compose.web.yml`);
    if (!DRY_RUN) { await ssh.putFile(webCompose, '/opt/litegen/docker-compose.web.yml'); }

    await sshExec(ssh,
      `cd /opt/litegen && \
       rm -rf dashboard-dist && mkdir dashboard-dist && \
       tar -C dashboard-dist --strip-components=1 -xzf litegen-dashboard-dist.tgz && \
       rm -rf landing-dist && mkdir landing-dist && \
       tar -C landing-dist --strip-components=1 -xzf litegen-landing-dist.tgz && \
       docker compose -f docker-compose.prod.yml -f docker-compose.web.yml --env-file .env up -d`,
      'unpack dashboard + landing dists + compose up web (Caddy)');

    if (!DRY_RUN) {
      await sshExec(ssh,
        'cd /opt/litegen && docker compose -f docker-compose.prod.yml -f docker-compose.web.yml ps',
        'compose ps');
    }

    console.log(`\n  Landing live at https://litegen.ai/`);
    console.log(`  Dashboard live at ${appUrl}/  (API at ${apiBase})`);
  } finally {
    if (ssh) { try { ssh.dispose(); } catch { /* ignore */ } }
    if (!DRY_RUN) {
      try { fs.unlinkSync(dashboardTgz); } catch { /* ignore */ }
      try { fs.unlinkSync(landingTgz); } catch { /* ignore */ }
      try { fs.rmSync(landingFallbackDir, { recursive: true, force: true }); } catch { /* ignore */ }
    }
  }
}

// ── Target: landing (static export -> Cloudflare Pages) ────────────────────
function deployLanding() {
  required('CLOUDFLARE_API_TOKEN');
  required('CLOUDFLARE_ACCOUNT_ID');
  const project = env('CF_PAGES_PROJECT') || 'litegen-landing';
  const outDir = 'apps/landing/out';
  const cfEnv = {
    ...process.env,
    CLOUDFLARE_API_TOKEN: env('CLOUDFLARE_API_TOKEN'),
    CLOUDFLARE_ACCOUNT_ID: env('CLOUDFLARE_ACCOUNT_ID'),
    // wrangler talks to the CF API over Node fetch — trust the OS CA store.
    NODE_OPTIONS: `${process.env.NODE_OPTIONS || ''} --use-system-ca`.trim(),
  };

  console.log('\n=== Building landing (static export) ===');
  run('npm --prefix apps/landing install', { stdio: 'inherit' });
  run('npm --prefix apps/landing run build', { env: { ...process.env, NODE_ENV: 'production' } });
  if (!DRY_RUN) assertNoLocalhostInBundle(outDir, 'landing');

  // `pages deploy` does not auto-create the project. Create it once (idempotent);
  // a re-run just errors with "already exists", which we ignore.
  console.log('\n=== Ensuring Cloudflare Pages project exists ===');
  try {
    run(`npx --yes wrangler pages project create ${project} --production-branch=main`, { env: cfEnv });
  } catch {
    console.log('  (create skipped — project already exists)');
  }

  console.log('\n=== Deploying landing to Cloudflare Pages ===');
  run(`npx --yes wrangler pages deploy ${outDir} --project-name=${project} --branch=main --commit-dirty=true`, { env: cfEnv });
  console.log(`\n  Landing deployed → https://${project}.pages.dev`);
}

// ── CLI ─────────────────────────────────────────────────────────────────
const TARGETS = {
  provision: { fn: provision,    desc: 'Create/reuse the DigitalOcean droplet (proxy + db)' },
  proxy:     { fn: deployProxy,  desc: 'Build amd64 image → push GHCR → droplet pulls + runs' },
  web:       { fn: deployWeb,    desc: 'Build dashboard + landing → ship dist + Caddyfile → compose up Caddy web tier' },
  landing:   { fn: deployLanding, desc: 'Static-export the Next.js site → Cloudflare Pages' },
};

async function deployAll() {
  await provision();
  await deployProxy();
  await deployWeb();
  await deployLanding();
}

async function main() {
  const target = process.argv[2];
  if (!target || target === '-h' || target === '--help') {
    console.log(`
  Usage: node deploy.js <target> [--dry-run]

  Targets:
    provision   ${TARGETS.provision.desc}
    proxy       ${TARGETS.proxy.desc}
    web         ${TARGETS.web.desc}
    landing     ${TARGETS.landing.desc}
    all         provision → proxy → web → landing  (full redeploy)

  Config: .env.deploy (see .env.deploy.template)
`);
    process.exit(0);
  }
  if (DRY_RUN) console.log('▷ DRY RUN — no real API/SSH/registry calls will be made.\n');

  if (target === 'all') { await deployAll(); return; }
  const entry = TARGETS[target];
  if (!entry) { console.error(`Unknown target "${target}". Use: provision, proxy, web, landing, all`); process.exit(1); }
  console.log(`Deploying: ${target} → ${entry.desc}`);
  await entry.fn();
}

main().catch((err) => { console.error('\n✖ Deploy failed:', err.message); process.exit(1); });
