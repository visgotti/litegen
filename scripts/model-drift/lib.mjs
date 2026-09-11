// Core of the provider model-drift check: compare what each vendor publishes
// against the models we carry in models/*.yaml. Pure functions plus one
// orchestrator (`run`) whose network and filesystem edges are injectable, so
// the whole pipeline is testable offline. See scripts/model-drift/README.md.
import { readdirSync, readFileSync, writeFileSync, mkdirSync, existsSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

const SKIP_PROVIDERS = new Set(['mock']);

const USER_AGENT = 'litegen-model-drift/1 (+https://github.com/visgotti/litegen; weekly docs check)';

// ─── Registry ────────────────────────────────────────────────────────────────

const unquote = (s) => s.replace(/^["']|["']$/g, '');

/**
 * Read models/*.yaml and group model ids by their `provider:` field, skipping
 * the mock provider. A line parser (no YAML dependency) is enough for the
 * registry's block style, and it refuses a file whose `- id:` count disagrees
 * with its `media_type:` count rather than silently dropping a model.
 * @returns {Map<string, string[]>} provider → model ids, in file order
 */
export function loadRegistry(modelsDir) {
  const byProvider = new Map();
  const files = readdirSync(modelsDir).filter((f) => f.endsWith('.yaml')).sort();
  for (const file of files) {
    const models = [];
    let current = null;
    let mediaTypes = 0;
    for (const line of readFileSync(join(modelsDir, file), 'utf8').split('\n')) {
      const id = line.match(/^\s*-\s+id:\s*(\S+)\s*$/);
      if (id) {
        current = { id: unquote(id[1]), provider: null };
        models.push(current);
        continue;
      }
      const provider = line.match(/^\s+provider:\s*(\S+)\s*$/);
      if (provider && current && !current.provider) current.provider = unquote(provider[1]);
      if (/^\s+media_type:\s*\S/.test(line)) mediaTypes++;
    }
    if (models.length !== mediaTypes) {
      throw new Error(
        `${file}: parsed ${models.length} ids but found ${mediaTypes} models (media_type lines); ` +
          'the line parser cannot read this file — keep one `- id:` line per model',
      );
    }
    for (const m of models) {
      const provider = m.provider ?? m.id.split('/')[0];
      if (SKIP_PROVIDERS.has(provider)) continue;
      if (!byProvider.has(provider)) byProvider.set(provider, []);
      byProvider.get(provider).push(m.id);
    }
  }
  return new Map([...byProvider].sort(([a], [b]) => a.localeCompare(b)));
}

// ─── One source ──────────────────────────────────────────────────────────────

/** Lower-case, trim trailing separators (regex over prose catches "sora-2-"), dedupe, sort. */
function normalizeIds(raw) {
  const out = new Set();
  for (const v of raw) {
    if (typeof v !== 'string') continue;
    const id = v.trim().toLowerCase().replace(/[-_.:/]+$/, '');
    if (id) out.add(id);
  }
  return [...out].sort();
}

const looksLikeHtml = (body) => /^\s*(<!doctype html|<html)/i.test(body);

/**
 * Turn one fetched response into the ids a source lists. Anything that is not
 * a trustworthy list — an HTTP error, an HTML page where a spec or markdown
 * was expected (moved page / JS app shell), a parse or extractor error, or an
 * empty result — comes back `ok: false` with the reason, never as zero ids.
 *
 * @param source {{key, expect: 'json'|'text'|'html', extract: RegExp | (parsed, body) => string[], snapshot?: (parsed, body, ctx) => string}}
 * @param response {{status?, contentType?, body?, error?}}
 * @param ctx {{ours?: string[]}} our carried vendor ids, for snapshots scoped to them
 */
export function evaluateSource(source, response, ctx = { ours: [] }) {
  const fail = (error) => ({ key: source.key, ok: false, error });
  if (response.error) return fail(response.error);
  if (response.status >= 400) return fail(`HTTP ${response.status}`);
  const body = response.body ?? '';
  if (source.expect !== 'html' && looksLikeHtml(body)) {
    return fail(`expected ${source.expect}, got an HTML page (the URL moved or now serves a JS app shell)`);
  }
  let parsed = body;
  if (source.expect === 'json') {
    try {
      parsed = JSON.parse(body);
    } catch (e) {
      return fail(`expected json, body did not parse (${e.message})`);
    }
  }
  let raw;
  try {
    if (source.extract instanceof RegExp) {
      const re = source.extract.global ? source.extract : new RegExp(source.extract.source, source.extract.flags + 'g');
      raw = [...body.matchAll(re)].map((m) => m[1] ?? m[0]);
    } else {
      raw = source.extract(parsed, body);
    }
  } catch (e) {
    return fail(`extract threw: ${e.message}`);
  }
  const ids = normalizeIds(raw ?? []);
  if (ids.length === 0) return fail('no model ids extracted — the page structure or the vendor id naming changed');
  let snapshot;
  try {
    snapshot = source.snapshot ? String(source.snapshot(parsed, body, ctx)) : ids.join('\n');
  } catch (e) {
    return fail(`snapshot threw: ${e.message}`);
  }
  return { key: source.key, ok: true, index: Boolean(source.index), ids, snapshot: snapshot.replace(/\s+$/, '') + '\n' };
}

// ─── One provider ────────────────────────────────────────────────────────────

/**
 * Compare the union of a provider's source ids against the vendor ids our
 * models map to (definition.models: litegen id → vendor id | vendor id[]).
 * - new:     upstream ids we neither carry nor acknowledged
 * - missing: carried models none of whose vendor ids appear upstream — only
 *            when every source succeeded, since a failed source may hold them
 * - failed:  failed sources, plus a canary failure when sources returned ids
 *            but none of ours (the source is wrong, so new/missing would be noise)
 * Index sources (lists of docs pages or endpoints) only feed snapshots: a new
 * page surfaces as a changed snapshot, not as a model id.
 */
export function classify(def, results) {
  const lc = (s) => s.toLowerCase();
  const failed = results.filter((r) => !r.ok).map((r) => ({ source: r.key, error: r.error }));
  const succeeded = results.filter((r) => r.ok && !r.index);
  const upstream = new Set(succeeded.flatMap((r) => r.ids));
  const carried = Object.entries(def.models).map(([model, v]) => ({ model, vendorIds: [v].flat() }));
  const ours = new Set(carried.flatMap((c) => c.vendorIds.map(lc)));

  if (succeeded.length > 0 && carried.length > 0 && ![...ours].some((id) => upstream.has(id))) {
    failed.push({
      source: '(canary)',
      error: `none of our ${carried.length} models appear in any source — the source moved, restructured, or the extract pattern no longer matches`,
    });
    return { provider: def.provider, failed, new: [], missing: [] };
  }

  const acks = (def.acknowledged ?? []).map((a) =>
    a.pattern ? { test: (id) => new RegExp(a.pattern.source, a.pattern.flags.replace('g', '')).test(id) } : { test: (id) => id === lc(a.id) },
  );
  const fresh = [...upstream].filter((id) => !ours.has(id) && !acks.some((a) => a.test(id))).sort();
  const missing =
    failed.length > 0 ? [] : carried.filter((c) => !c.vendorIds.some((v) => upstream.has(lc(v))));
  return { provider: def.provider, failed, new: fresh, missing };
}

/** Registry ↔ definition consistency: every carried model mapped, no stale mappings. */
function validateDefinition(def, registryIds) {
  const file = `scripts/model-drift/providers/${def.provider}.mjs`;
  const mapped = new Set(Object.keys(def.models));
  const inRegistry = new Set(registryIds);
  return [
    ...registryIds
      .filter((id) => !mapped.has(id))
      .map((id) => ({ source: '(config)', error: `${id} is in models/*.yaml but not mapped in ${file}` })),
    ...[...mapped]
      .filter((id) => !inRegistry.has(id))
      .map((id) => ({ source: '(config)', error: `${id} is mapped in ${file} but not in models/*.yaml` })),
  ];
}

// ─── Network ─────────────────────────────────────────────────────────────────

/** Default fetch: one retry on network errors, 429 and 5xx. Returns {status, contentType, body}. */
export async function httpFetch(url, { headers = {}, timeoutMs = 30_000 } = {}) {
  let lastErr;
  for (let attempt = 0; attempt < 2; attempt++) {
    if (attempt > 0) await new Promise((r) => setTimeout(r, 2_000));
    try {
      const res = await fetch(url, {
        headers: { 'user-agent': USER_AGENT, ...headers },
        redirect: 'follow',
        signal: AbortSignal.timeout(timeoutMs),
      });
      const out = { status: res.status, contentType: res.headers.get('content-type') ?? '', body: await res.text() };
      const transient = res.status === 429 || res.status >= 500;
      if (transient && attempt === 0) continue;
      return out;
    } catch (e) {
      lastErr = e;
    }
  }
  throw lastErr;
}

async function fetchSource(source, fetchImpl) {
  try {
    return await fetchImpl(source.url, { headers: source.headers });
  } catch (e) {
    const cause = e.cause ? ` (${e.cause.code ?? e.cause.message})` : '';
    return { error: `fetch failed: ${e.message}${cause}` };
  }
}

/**
 * A source's fetch URL. `discover: { url, pattern }` finds a content-hashed
 * asset (a docs bundle renamed on every deploy) by matching the page that
 * links it, so the source survives redeploys but still fails loudly — with
 * the reason — when the page stops linking anything like it.
 */
async function resolveUrl(source, fetchImpl) {
  if (!source.discover) return { url: source.url };
  const { url, pattern } = source.discover;
  const page = await fetchSource({ url, headers: source.headers }, fetchImpl);
  if (page.error) return { error: `discover ${url}: ${page.error}` };
  if (page.status >= 400) return { error: `discover ${url}: HTTP ${page.status}` };
  const m = (page.body ?? '').match(pattern);
  if (!m) return { error: `discover ${url}: no match for ${pattern} — the page no longer links the asset` };
  return { url: new URL(m[1] ?? m[0], url).href };
}

async function mapLimit(items, limit, fn) {
  const out = new Array(items.length);
  let next = 0;
  const worker = async () => {
    while (next < items.length) {
      const i = next++;
      out[i] = await fn(items[i]);
    }
  };
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
  return out;
}

// ─── Orchestrator ────────────────────────────────────────────────────────────

/**
 * Check every provider. Exit code: 0 clean · 1 drift (new / missing / changed
 * snapshot) · 2 something needs investigating (a source failed, or the drift
 * definitions are out of sync with models/*.yaml) — 2 wins over 1.
 *
 * Snapshots (the typings-relevant excerpt of each source, or its id list) are
 * written to `snapshotsDir/<provider>/<key>.txt` unless `write` is false, so
 * after a run `git diff` on that directory shows exactly what changed upstream.
 */
export async function run({ modelsDir, definitions, snapshotsDir, fetchImpl = httpFetch, write = true, only, concurrency = 4 }) {
  const registry = loadRegistry(modelsDir);
  const defs = new Map(definitions.map((d) => [d.provider, d]));
  const known = [...new Set([...registry.keys(), ...defs.keys()])].sort();
  const unknown = (only ?? []).filter((p) => !known.includes(p));
  if (unknown.length) throw new Error(`unknown provider(s): ${unknown.join(', ')} — known: ${known.join(', ')}`);
  const names = known.filter((p) => !only || only.includes(p));

  const problems = validateDefinitions(registry, definitions);

  const providers = await mapLimit(names, concurrency, async (provider) => {
    const config = problems.filter((p) => p.provider === provider).map(({ source, error }) => ({ source, error }));
    const def = defs.get(provider);
    if (!def) return { provider, failed: config, new: [], missing: [], changed: [], sources: [] };
    const ours = Object.values(def.models).flat();
    const results = [];
    const urls = new Map();
    for (const source of def.sources) {
      const target = await resolveUrl(source, fetchImpl);
      urls.set(source.key, target.url ?? source.discover?.url);
      const at = { ...source, url: target.url };
      const response = target.error ? { error: target.error } : await fetchSource(at, fetchImpl);
      let result = evaluateSource(source, response, { ours });
      // A 2xx that fails extraction is sometimes a CDN serving a stray shell;
      // one re-fetch keeps that from paging anyone. HTTP errors were already
      // retried by the fetcher, so they are not.
      if (!result.ok && !response.error && response.status < 400) {
        result = evaluateSource(source, await fetchSource(at, fetchImpl), { ours });
      }
      results.push(result);
    }
    const verdict = classify(def, results);

    const dir = join(snapshotsDir, provider);
    const changed = [];
    for (const r of results.filter((x) => x.ok)) {
      const file = join(dir, `${r.key}.txt`);
      const previous = existsSync(file) ? readFileSync(file, 'utf8') : null;
      if (previous !== null && previous !== r.snapshot) changed.push(r.key);
      if (write) {
        mkdirSync(dir, { recursive: true });
        writeFileSync(file, r.snapshot);
      }
    }
    // A renamed or removed source would otherwise leave a stale baseline behind.
    if (write && existsSync(dir)) {
      const current = new Set(def.sources.map((s) => `${s.key}.txt`));
      for (const f of readdirSync(dir)) if (f.endsWith('.txt') && !current.has(f)) rmSync(join(dir, f));
    }
    return {
      ...verdict,
      failed: [...config, ...verdict.failed],
      changed,
      sources: results.map((r) => ({ key: r.key, url: urls.get(r.key), ok: r.ok, ids: r.ids?.length ?? 0, error: r.error })),
    };
  });

  const needsInvestigation = providers.some((p) => p.failed.length > 0);
  const drift = providers.some((p) => p.new.length || p.missing.length || p.changed.length);
  return { exitCode: needsInvestigation ? 2 : drift ? 1 : 0, providers };
}

// ─── Definitions ─────────────────────────────────────────────────────────────

const EXPECT_KINDS = ['json', 'text', 'html'];

/**
 * Import every `<provider>.mjs` in `dir` and check its shape up front, so a
 * malformed definition fails the run by name instead of mid-fetch.
 */
export async function loadDefinitions(dir) {
  const defs = [];
  for (const file of readdirSync(dir).filter((f) => f.endsWith('.mjs')).sort()) {
    const def = (await import(pathToFileURL(join(dir, file)).href)).default;
    const expected = file.replace(/\.mjs$/, '');
    const bad = (msg) => new Error(`providers/${file}: ${msg}`);
    if (!def || def.provider !== expected) throw bad(`default export's provider is "${def?.provider}", expected "${expected}"`);
    if (!def.models || typeof def.models !== 'object') throw bad('needs a models map (litegen id → vendor id)');
    if (!Array.isArray(def.sources) || def.sources.length === 0) throw bad('needs at least one entry in sources');
    const keys = new Set();
    for (const s of def.sources) {
      if (keys.has(s.key)) throw bad(`duplicate source key "${s.key}"`);
      keys.add(s.key);
      if (!/^[a-z0-9][a-z0-9_-]*$/.test(s.key ?? '')) throw bad(`source key "${s.key}" must be a lower-case slug (it names the snapshot file)`);
      const discoverable = s.discover && /^https:\/\//.test(s.discover.url ?? '') && s.discover.pattern instanceof RegExp;
      if (!/^https:\/\//.test(s.url ?? '') && !discoverable) {
        throw bad(`source "${s.key}" needs an https url, or discover: { url, pattern } for a content-hashed asset`);
      }
      if (!EXPECT_KINDS.includes(s.expect)) throw bad(`source "${s.key}" has expect "${s.expect}" — use one of ${EXPECT_KINDS.join(', ')}`);
      if (!(s.extract instanceof RegExp) && typeof s.extract !== 'function') throw bad(`source "${s.key}" needs extract (RegExp or function)`);
    }
    defs.push(def);
  }
  return defs.sort((a, b) => a.provider.localeCompare(b.provider));
}

/** Every registry provider has a definition, and every definition is in sync with the registry. */
export function validateDefinitions(registry, definitions) {
  const defined = new Set(definitions.map((d) => d.provider));
  const problems = [...registry.keys()]
    .filter((p) => !defined.has(p))
    .map((provider) => ({ provider, source: '(config)', error: `no drift definition — add scripts/model-drift/providers/${provider}.mjs` }));
  for (const def of definitions) {
    for (const p of validateDefinition(def, registry.get(def.provider) ?? [])) problems.push({ provider: def.provider, ...p });
  }
  return problems;
}

// ─── Report ──────────────────────────────────────────────────────────────────

const EXIT_MEANING = {
  0: 'clean',
  1: 'drift — review the items below',
  2: 'needs investigation — a source or definition is broken (see FAILED)',
};

/** Human-readable report. The skill and scripts read `--json` instead. */
export function formatReport({ exitCode, providers }) {
  const lines = [`Model drift · ${providers.length} providers · exit ${exitCode} (${EXIT_MEANING[exitCode]})`, ''];
  const clean = [];
  for (const p of providers) {
    const problems = [];
    for (const f of p.failed) problems.push(`  FAILED ${f.source}: ${f.error}`);
    if (p.new.length) problems.push(`  new upstream (not carried, not acknowledged): ${p.new.join(', ')}`);
    for (const m of p.missing) problems.push(`  missing upstream: ${m.model} (looked for ${m.vendorIds.join(' | ')})`);
    for (const key of p.changed) problems.push(`  changed: ${key} → git diff scripts/model-drift/snapshots/${p.provider}/${key}.txt`);
    if (problems.length) lines.push(p.provider, ...problems);
    else clean.push(p.provider);
  }
  if (clean.length) lines.push('', `clean: ${clean.join(', ')}`);
  return lines.join('\n') + '\n';
}
