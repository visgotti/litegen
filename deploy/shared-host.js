'use strict';

/**
 * Shared-host mode for deploy.js: several isolated LiteGen stacks on one server
 * behind one "edge" Caddy that imports /sites/<name>/Caddyfile for each tenant.
 *
 * Switched on by LITEGEN_SHARED_STACK=<stack> in the deploy env file. Everything
 * here is a pure function that returns a remote command or file body, so deploy.js
 * stays the only thing that talks to the server, and with the setting absent none
 * of this code runs.
 */

// A stack name becomes a directory, a compose project and a DNS label
// (`<stack>-api`), so it takes the strictest of the three rule sets.
const STACK_RE = /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/;
const STACK_MAX = 59; // leaves room for "-api" inside a 63-character DNS label
// /opt/edge and /opt/sites belong to the host. /opt/litegen is the single-host
// layout, and adopting it would make a shared deploy take over those containers.
const RESERVED_STACKS = new Set(['edge', 'sites', 'litegen']);
const ENV_NAME_RE = /^[A-Z_][A-Z0-9_]*$/;
const IMAGE_REPO_RE = /^[a-z0-9][a-z0-9._/-]*$/;

const EDGE_CONTAINER = 'edge-caddy-1';
const EDGE_CONFIG = '/etc/caddy/Caddyfile';
const IMAGE_MAX_AGE_MS = 168 * 60 * 60 * 1000; // the 7 days single-host mode prunes at

function resolveSharedConfig(env) {
  const stack = (env.LITEGEN_SHARED_STACK || '').trim();
  if (!stack) return null;
  if (!STACK_RE.test(stack) || stack.length > STACK_MAX) {
    throw new Error(
      `LITEGEN_SHARED_STACK="${stack}" is not a valid stack name: use lowercase letters, ` +
        `digits and inner hyphens, at most ${STACK_MAX} characters.`,
    );
  }
  if (RESERVED_STACKS.has(stack)) {
    throw new Error(`LITEGEN_SHARED_STACK="${stack}" is reserved: /opt/${stack} is not a stack directory.`);
  }
  const dnsTokenEnv = (env.LITEGEN_SHARED_DNS_TOKEN_ENV || '').trim() || null;
  if (dnsTokenEnv && !ENV_NAME_RE.test(dnsTokenEnv)) {
    throw new Error(`LITEGEN_SHARED_DNS_TOKEN_ENV="${dnsTokenEnv}" is not an environment variable name.`);
  }
  return {
    stack,
    remoteDir: `/opt/${stack}`,
    siteDir: `/opt/sites/${stack}`,
    edgeSiteDir: `/sites/${stack}`,
    apiAlias: `${stack}-api`,
    dnsTokenEnv,
  };
}

function composeCommand(shared) {
  return `docker compose -p ${shared.stack} --env-file .env -f docker-compose.prod.yml -f docker-compose.shared.yml`;
}

// `ports: !reset []` needs a recent Compose, and an older one could ignore the tag
// and keep the base file's 4000:4000. So the merged config is checked before `up`.
// --no-interpolate keeps the secrets in .env out of the rendered config.
function noPublishedPortsCheck(shared) {
  return [
    `cd ${shared.remoteDir}`,
    `cfg=$(${composeCommand(shared)} config --no-interpolate --format json) || exit 1`,
    `case "$cfg" in *'"published"'*) echo 'shared stack ${shared.stack} would publish a host port; refusing to start it' >&2; exit 1;; esac`,
  ].join('\n');
}

// No port is published, so the probe runs inside the container (the image ships wget).
function healthCommand(shared) {
  return `cd ${shared.remoteDir} && ${composeCommand(shared)} exec -T litegen wget -qO- http://127.0.0.1:4000/health/live >/dev/null`;
}

/**
 * Remove this repository's images older than 7 days, and nothing else.
 *
 * Not `docker image prune`: it has no repository filter. Containers of EVERY stack
 * count as in use, because `docker image rm <tag>` on an image with a second tag
 * only untags it, even under a running container, and the stacks share this
 * repository. Images go by their own tags and digests, never by ID, because
 * removing by ID would also take a tag some other repository holds on that image.
 */
function imageCleanupCommand(imageRepo, nowMs) {
  if (!IMAGE_REPO_RE.test(imageRepo)) throw new Error(`Refusing image cleanup for odd repository "${imageRepo}".`);
  const cutoff = new Date(nowMs - IMAGE_MAX_AGE_MS).toISOString();
  return [
    '(',
    `repo='${imageRepo}'`,
    `cutoff='${cutoff}'`,
    'containers=$(docker ps -aq) || exit 1',
    `in_use=$(printf '%s\\n' "$containers" | xargs -r docker inspect --format '{{.Image}}') || exit 1`,
    `docker image ls --no-trunc --format '{{.ID}}' "$repo" | sort -u |`,
    `  xargs -r docker image inspect --format '{{.Id}} {{.Created}}' |`,
    `  awk -v cutoff="$cutoff" '$2 < cutoff { print $1 }' |`,
    '  while read -r id; do',
    `    printf '%s\\n' "$in_use" | grep -qxF "$id" && continue`,
    `    docker image inspect --format '{{range .RepoTags}}{{println .}}{{end}}{{range .RepoDigests}}{{println .}}{{end}}' "$id" |`,
    `      awk -v repo="$repo" 'index($0, repo ":") == 1 || index($0, repo "@") == 1' |`,
    '      xargs -r docker image rm',
    '  done',
    ') >/dev/null 2>&1 || true',
  ].join('\n');
}

/**
 * Turn a single-host Caddyfile into a site file the edge Caddy can import.
 *
 * The site blocks are kept as written, so the routing cannot drift from the
 * single-host file. Only what the shared host changes is rewritten: the static
 * roots and the API upstream. The global options block is dropped because an
 * imported file cannot have one (the ACME email lives in the edge Caddyfile), and
 * any global option other than `email` is an error rather than a silent loss.
 */
function renderSiteCaddyfile(source, shared) {
  const out = [`# Generated by deploy.js for shared stack ${shared.stack}. Do not edit on the host.`];
  let depth = 0;
  let inGlobal = false;
  let sites = 0;

  for (const raw of source.split('\n')) {
    const line = raw.trimEnd();
    const trimmed = line.trim();
    if (trimmed.startsWith('#')) continue;
    if (!trimmed) {
      if (depth > 0 && !inGlobal) out.push('');
      continue;
    }

    const opens = (trimmed.match(/\{/g) || []).length;
    const closes = (trimmed.match(/\}/g) || []).length;

    if (depth === 0) {
      if (trimmed === '{') {
        if (sites > 0) throw new Error('Caddyfile: a global options block must come first.');
        inGlobal = true;
        depth = 1;
        continue;
      }
      if (!trimmed.endsWith('{') || trimmed.startsWith('(') || opens !== closes + 1) {
        throw new Error(`Caddyfile: unsupported top-level line for shared mode: "${trimmed}"`);
      }
      sites += 1;
      if (sites > 1) out.push('');
      out.push(line);
      if (shared.dnsTokenEnv) {
        out.push('    tls {', `        dns cloudflare {env.${shared.dnsTokenEnv}}`, '    }');
      }
      depth = 1;
      continue;
    }

    depth += opens - closes;
    if (depth < 0) throw new Error('Caddyfile: unbalanced braces.');

    if (inGlobal) {
      if (depth === 0) { inGlobal = false; continue; }
      if (!/^email\s/.test(trimmed)) {
        throw new Error(`Caddyfile: global option "${trimmed}" cannot move into a shared site file.`);
      }
      continue;
    }

    out.push(
      line
        .replace(/\/srv\/(landing|app)(?![\w-])/g, `${shared.edgeSiteDir}/$1`)
        .replace(/(^|\s)litegen:4000(?=\s|$)/g, `$1${shared.apiAlias}:4000`),
    );
  }

  if (depth !== 0) throw new Error('Caddyfile: unbalanced braces.');
  if (sites === 0) throw new Error('Caddyfile: no site blocks found.');
  const rendered = `${out.join('\n')}\n`;
  const leftover = rendered.match(/\/srv\/\S*|(?:^|\s)litegen:4000\b/m);
  if (leftover) throw new Error(`Caddyfile: "${leftover[0].trim()}" has no shared-host equivalent.`);
  return rendered;
}

/**
 * Publish a site on the edge Caddy. Expects <siteDir>/Caddyfile.next to be written.
 *
 * Every site on the host shares one config, so a bad file here would break the next
 * reload for all of them. The candidate is validated in place through the real
 * import, and the EXIT trap puts the previous file back on any failure, including a
 * dropped SSH session. Static files are unpacked to a staging dir first and only
 * swapped in after the reload, so a failure leaves the old site fully intact.
 */
function sitePublishScript(shared, { dashboardTgz, landingTgz }) {
  const caddy = (verb) => `docker exec ${EDGE_CONTAINER} caddy ${verb} --adapter caddyfile --config ${EDGE_CONFIG}`;
  return [
    'set -eu',
    `cd ${shared.siteDir}`,
    'rm -rf .staging .previous',
    'mkdir -p .staging/app .staging/landing',
    `tar -C .staging/app --strip-components=1 -xzf ${dashboardTgz}`,
    `tar -C .staging/landing --strip-components=1 -xzf ${landingTgz}`,
    'rm -f Caddyfile.prev',
    'if [ -f Caddyfile ]; then cp -p Caddyfile Caddyfile.prev; fi',
    'published=0',
    'restore() {',
    '  if [ "$published" = 1 ]; then return 0; fi',
    '  if [ -f Caddyfile.prev ]; then mv -f Caddyfile.prev Caddyfile; else rm -f Caddyfile; fi',
    '}',
    // Armed only now: earlier, "no .prev" would wrongly mean "delete the live file".
    'trap restore EXIT',
    "trap 'exit 129' HUP",
    "trap 'exit 130' INT",
    "trap 'exit 143' TERM",
    'mv -f Caddyfile.next Caddyfile',
    caddy('validate'),
    caddy('reload'),
    'published=1',
    'mkdir .previous',
    'for d in app landing; do',
    '  if [ -d "$d" ]; then mv "$d" ".previous/$d"; fi',
    '  mv ".staging/$d" "$d"',
    'done',
    'rm -rf .staging .previous',
  ].join('\n');
}

module.exports = {
  resolveSharedConfig,
  composeCommand,
  noPublishedPortsCheck,
  healthCommand,
  imageCleanupCommand,
  renderSiteCaddyfile,
  sitePublishScript,
};
