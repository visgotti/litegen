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
const LIFECYCLE_KEYS = ['deprecated_at', 'retires_at'];

/**
 * Every model block in models/*.yaml, in file order: its id, its `provider:`
 * and the lifecycle dates at the model's own level (a nested key of the same
 * name, inside a param, say, is not the model's). A line parser (no YAML
 * dependency) is enough for the registry's block style, and it refuses a file
 * whose `- id:` count disagrees with its `media_type:` count rather than
 * silently dropping a model.
 * @returns {{ id: string, provider: string | null, deprecated_at: string | null, retires_at: string | null }[]}
 */
function parseModels(modelsDir) {
  const all = [];
  const files = readdirSync(modelsDir).filter((f) => f.endsWith('.yaml')).sort();
  for (const file of files) {
    const models = [];
    let current = null;
    let keyIndent = -1;
    let mediaTypes = 0;
    for (const line of readFileSync(join(modelsDir, file), 'utf8').split('\n')) {
      const id = line.match(/^(\s*-\s+)id:\s*(\S+)\s*$/);
      if (id) {
        current = { id: unquote(id[2]), provider: null, deprecated_at: null, retires_at: null };
        // The model's own keys line up with `id` after the `- `.
        keyIndent = id[1].length;
        models.push(current);
        continue;
      }
      const provider = line.match(/^\s+provider:\s*(\S+)\s*$/);
      if (provider && current && !current.provider) current.provider = unquote(provider[1]);
      const date = line.match(/^(\s+)(\w+):\s*([^\s#]+)\s*(?:#.*)?$/);
      if (date && current && date[1].length === keyIndent && LIFECYCLE_KEYS.includes(date[2])) {
        const value = unquote(date[3]);
        current[date[2]] = value === 'null' || value === '~' ? null : value;
      }
      if (/^\s+media_type:\s*\S/.test(line)) mediaTypes++;
    }
    if (models.length !== mediaTypes) {
      throw new Error(
        `${file}: parsed ${models.length} ids but found ${mediaTypes} models (media_type lines); ` +
          'the line parser cannot read this file — keep one `- id:` line per model',
      );
    }
    all.push(...models);
  }
  return all;
}

/**
 * Read models/*.yaml and group model ids by their `provider:` field, skipping
 * the mock provider.
 * @returns {Map<string, string[]>} provider → model ids, in file order
 */
export function loadRegistry(modelsDir) {
  const byProvider = new Map();
  for (const m of parseModels(modelsDir)) {
    const provider = m.provider ?? m.id.split('/')[0];
    if (SKIP_PROVIDERS.has(provider)) continue;
    if (!byProvider.has(provider)) byProvider.set(provider, []);
    byProvider.get(provider).push(m.id);
  }
  return new Map([...byProvider].sort(([a], [b]) => a.localeCompare(b)));
}

/**
 * The lifecycle dates models/*.yaml records per model (`YYYY-MM-DD`, or null).
 * The catalog counts a model deprecated once either is set
 * (litegen-core `ModelSchema::status_on`).
 * @returns {Map<string, { deprecated_at: string | null, retires_at: string | null }>} model id → dates
 */
export function loadLifecycle(modelsDir) {
  return new Map(parseModels(modelsDir).map((m) => [m.id, { deprecated_at: m.deprecated_at, retires_at: m.retires_at }]));
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
 * A source may also say which models it lists as deprecated but still served
 * (`deprecated`, such as fal's registry status): those come back normalized as
 * `deprecated`, apart from `ids`, and count as listed.
 *
 * @param source {{key, expect: 'json'|'text'|'html', extract: RegExp | (parsed, body) => string[], deprecated?: (parsed, body) => string[], snapshot?: (parsed, body, ctx) => string}}
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
  let deprecatedRaw = [];
  try {
    if (source.deprecated) deprecatedRaw = source.deprecated(parsed, body);
  } catch (e) {
    return fail(`deprecated threw: ${e.message}`);
  }
  const ids = normalizeIds(raw ?? []);
  const deprecated = normalizeIds(deprecatedRaw ?? []);
  // A source whose models are all deprecated still lists them: they are served.
  if (ids.length === 0 && deprecated.length === 0) return fail('no model ids extracted — the page structure or the vendor id naming changed');
  let snapshot;
  try {
    snapshot = source.snapshot ? String(source.snapshot(parsed, body, ctx)) : ids.join('\n');
  } catch (e) {
    return fail(`snapshot threw: ${e.message}`);
  }
  return { key: source.key, ok: true, index: Boolean(source.index), ids, deprecated, snapshot: snapshot.replace(/\s+$/, '') + '\n' };
}

// ─── One provider ────────────────────────────────────────────────────────────

/**
 * Compare the union of a provider's source ids against the vendor ids our
 * models map to (definition.models: litegen id → vendor id | vendor id[]).
 * - new:     upstream ids we neither carry nor acknowledged
 * - missing: carried models with a vendor id that no longer appears upstream
 *            (`absent`). A model maps to every id its adapter sends — one per
 *            endpoint it routes to — so one endpoint going away is a desync
 *            even while the others remain. Only when every source succeeded,
 *            since a failed source may hold them.
 * - deprecated: carried models with a vendor id a source lists as deprecated
 *            but still served, and none lists as active ({ model, vendorIds:
 *            those ids }). Such an id is not `absent`; run() keeps the entries
 *            models/*.yaml does not already record.
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
  // Deprecated and nowhere active: still served, so neither new nor gone.
  const upstreamDeprecated = new Set(succeeded.flatMap((r) => r.deprecated ?? []).filter((id) => !upstream.has(id)));
  const listed = (id) => upstream.has(id) || upstreamDeprecated.has(id);
  const carried = Object.entries(def.models).map(([model, v]) => ({ model, vendorIds: [v].flat() }));
  const ours = new Set(carried.flatMap((c) => c.vendorIds.map(lc)));

  if (succeeded.length > 0 && carried.length > 0 && ![...ours].some(listed)) {
    failed.push({
      source: '(canary)',
      error: `none of our ${carried.length} models appear in any source — the source moved, restructured, or the extract pattern no longer matches`,
    });
    return { provider: def.provider, failed, new: [], missing: [], deprecated: [] };
  }

  const acks = (def.acknowledged ?? []).map((a) =>
    a.pattern ? { test: (id) => new RegExp(a.pattern.source, a.pattern.flags.replace('g', '')).test(id) } : { test: (id) => id === lc(a.id) },
  );
  const fresh = [...upstream].filter((id) => !ours.has(id) && !acks.some((a) => a.test(id))).sort();
  const missing =
    failed.length > 0
      ? []
      : carried.flatMap((c) => {
          const absent = c.vendorIds.filter((v) => !listed(lc(v)));
          return absent.length ? [{ ...c, absent }] : [];
        });
  const deprecated = carried.flatMap((c) => {
    const vendorIds = c.vendorIds.filter((v) => upstreamDeprecated.has(lc(v)));
    return vendorIds.length ? [{ model: c.model, vendorIds }] : [];
  });
  return { provider: def.provider, failed, new: fresh, missing, deprecated };
}

const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
/** `id` as a whole token or path segment, not part of a longer id: gen4 ≠ gen4_turbo, but /v1/gen4/generate counts. */
const mentions = (text, id) => new RegExp(`(?<![\\w.-])${escapeRe(id)}(?![\\w-]|\\.\\w)`, 'i').test(text);

/**
 * A source whose snapshot counts as typings evidence: one with its own
 * `snapshot` that is not an index, unless it says `typings: false` (a lineup
 * table or an id list: it says a model exists, not what it accepts), or an
 * index that says `typings: true` (its ids are field names, say, but its
 * snapshot is a request schema).
 */
const isTypings = (s) => Boolean(s.snapshot) && s.typings !== false && (!s.index || s.typings === true);

/**
 * Typings coverage: every vendor id we carry must appear in at least one
 * typings snapshot (see `isTypings`), or the check never compares that
 * model's request schema with models/*.yaml, and a new enum value or bound on
 * it goes unseen. A model pinned in the definition's `typingsFrom` (litegen id
 * → source keys) takes its typings from those sources only, which need not
 * name its vendor id: for a free-form model field (Leonardo's `modelId`), or
 * a vendor id that another model sends to a different endpoint. A model whose
 * vendor publishes no request schema is listed in the definition's
 * `untyped`, with the reason.
 * Withheld when a typings source failed, since the id may live in that one,
 * and for a model already reported missing (`missing`: litegen ids).
 */
export function typingsGaps(def, results, missing = []) {
  const typedKeys = new Set(def.sources.filter(isTypings).map((s) => s.key));
  const typed = results.filter((r) => typedKeys.has(r.key));
  if (typed.some((r) => !r.ok)) return [];
  const exempt = def.untyped ?? {};
  const pinned = def.typingsFrom ?? {};
  return Object.entries(def.models)
    .filter(([model]) => !Object.hasOwn(exempt, model) && !missing.includes(model))
    .flatMap(([model, v]) => {
      if (Object.hasOwn(pinned, model)) {
        // A pin names where this model's request schema is: those sources, and only those.
        const keys = [pinned[model]].flat();
        const ok = keys.length > 0 && keys.every((k) => typed.some((r) => r.key === k && r.ok));
        return ok ? [] : [{ source: '(typings)', error: `${model}: its typingsFrom sources (${keys.join(', ')}) gave no typings snapshot` }];
      }
      const absent = [v].flat().filter((id) => !typed.some((r) => mentions(r.snapshot, id)));
      if (absent.length === 0) return [];
      return [{
        source: '(typings)',
        error:
          `${model}: ${absent.join(', ')} appears in no typings snapshot, so its request schema is never compared — ` +
          'add or re-scope a source that covers it, or list the model in `untyped` with the reason',
      }];
    });
}

/** Registry ↔ definition consistency: every carried model mapped, no stale mappings, exemptions or typings pins. */
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
    ...Object.entries(def.untyped ?? {})
      .filter(([id, reason]) => !mapped.has(id) || typeof reason !== 'string' || !reason.trim())
      .map(([id]) => ({
        source: '(config)',
        error: mapped.has(id) ? `untyped ${id} in ${file} needs a reason` : `untyped ${id} in ${file} is not a mapped model`,
      })),
    ...Object.entries(def.typingsFrom ?? {}).flatMap(([id, keys]) => {
      if (!mapped.has(id)) return [{ source: '(config)', error: `typingsFrom ${id} in ${file} is not a mapped model` }];
      return [keys]
        .flat()
        .filter((k) => !def.sources.some((s) => s.key === k && isTypings(s)))
        .map((k) => ({ source: '(config)', error: `typingsFrom ${id} in ${file} names "${k}", which is not a typings source of this definition` }));
    }),
  ];
}

// ─── Network ─────────────────────────────────────────────────────────────────

/**
 * Default fetch: up to three retries on network errors, 429 and 5xx, each
 * after the server's Retry-After (in seconds, at most 60) or else 2, 4 and
 * 8 s. fal's Platform API answers a burst of `expand=openapi-3.0` requests
 * with 429 (seen 2026-11-04), and one retry 2 s later wasn't enough. Returns
 * {status, contentType, body}; the last answer when every retry failed too.
 */
export async function httpFetch(url, { headers = {}, timeoutMs = 30_000, retries = 3 } = {}) {
  let lastErr;
  let wait = 0;
  for (let attempt = 0; attempt <= retries; attempt++) {
    if (attempt > 0) await new Promise((r) => setTimeout(r, wait));
    wait = 2_000 * 2 ** attempt;
    try {
      const res = await fetch(url, {
        headers: { 'user-agent': USER_AGENT, ...headers },
        redirect: 'follow',
        signal: AbortSignal.timeout(timeoutMs),
      });
      const out = { status: res.status, contentType: res.headers.get('content-type') ?? '', body: await res.text() };
      const transient = res.status === 429 || res.status >= 500;
      if (transient && attempt < retries) {
        const after = Number.parseFloat(res.headers.get('retry-after') ?? '');
        if (Number.isFinite(after) && after >= 0) wait = Math.min(after, 60) * 1_000;
        continue;
      }
      return out;
    } catch (e) {
      lastErr = e;
    }
  }
  throw lastErr;
}

async function fetchSource(source, fetchImpl) {
  try {
    // A source may give its own `timeoutMs` (a multi-megabyte spec on a slow
    // link outlasts the 30 s default).
    return await fetchImpl(source.url, { headers: source.headers, timeoutMs: source.timeoutMs });
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
 * Check every provider. Exit code: 0 clean · 1 drift (new / missing / deprecated
 * upstream and not recorded in models/*.yaml / changed snapshot / a snapshot
 * with no baseline) · 2 something needs investigating (a
 * source failed, or the drift definitions are out of sync with models/*.yaml)
 * — 2 wins over 1.
 *
 * Snapshots (the typings-relevant excerpt of each source, or its id list) are
 * written to `snapshotsDir/<provider>/<key>.txt` unless `write` is false, so
 * after a run `git diff` on that directory shows exactly what changed upstream.
 * A source with no snapshot there yet is reported as `noBaseline`: nothing was
 * compared, so its first snapshot needs a whole-file review.
 */
export async function run({ modelsDir, definitions, snapshotsDir, fetchImpl = httpFetch, write = true, only, concurrency = 4 }) {
  // An empty filter matches nothing and would report a clean run over zero providers.
  if (only && only.length === 0) throw new Error('empty provider filter: name one or more providers, or pass no filter to check every provider');
  const registry = loadRegistry(modelsDir);
  const lifecycle = loadLifecycle(modelsDir);
  const defs = new Map(definitions.map((d) => [d.provider, d]));
  const known = [...new Set([...registry.keys(), ...defs.keys()])].sort();
  const unknown = (only ?? []).filter((p) => !known.includes(p));
  if (unknown.length) throw new Error(`unknown provider(s): ${unknown.join(', ')} — known: ${known.join(', ')}`);
  const names = known.filter((p) => !only || only.includes(p));

  const problems = validateDefinitions(registry, definitions);

  const providers = await mapLimit(names, concurrency, async (provider) => {
    const config = problems.filter((p) => p.provider === provider).map(({ source, error }) => ({ source, error }));
    const def = defs.get(provider);
    if (!def) return { provider, failed: config, new: [], missing: [], deprecated: [], changed: [], noBaseline: [], sources: [] };
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
    // A deprecation models/*.yaml already records (either date) is not news.
    verdict.deprecated = verdict.deprecated.filter(({ model }) => {
      const dates = lifecycle.get(model);
      return !(dates?.deprecated_at || dates?.retires_at);
    });
    // A broken source would make every model it covers look untyped.
    if (verdict.failed.length === 0) verdict.failed.push(...typingsGaps(def, results, verdict.missing.map((m) => m.model)));

    const dir = join(snapshotsDir, provider);
    const changed = [];
    // No earlier snapshot to compare with: the first one must be read whole, so it is reported too.
    const noBaseline = [];
    for (const r of results.filter((x) => x.ok)) {
      const file = join(dir, `${r.key}.txt`);
      const previous = existsSync(file) ? readFileSync(file, 'utf8') : null;
      if (previous === null) noBaseline.push(r.key);
      else if (previous !== r.snapshot) changed.push(r.key);
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
      noBaseline,
      sources: results.map((r) => ({ key: r.key, url: urls.get(r.key), ok: r.ok, ids: r.ids?.length ?? 0, error: r.error })),
    };
  });

  const needsInvestigation = providers.some((p) => p.failed.length > 0);
  const drift = providers.some((p) => p.new.length || p.missing.length || p.deprecated.length || p.changed.length || p.noBaseline.length);
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
    const pins = def.typingsFrom;
    const pinList = (keys) => Array.isArray(keys) && keys.length > 0 && keys.every((k) => typeof k === 'string');
    if (pins !== undefined && (!pins || typeof pins !== 'object' || Array.isArray(pins) || !Object.values(pins).every(pinList))) {
      throw bad('typingsFrom must map litegen model ids to non-empty lists of source keys');
    }
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
      if (s.typings !== undefined && typeof s.typings !== 'boolean') throw bad(`source "${s.key}" has typings ${JSON.stringify(s.typings)} — use true or false`);
      if (s.deprecated !== undefined && typeof s.deprecated !== 'function') throw bad(`source "${s.key}" has a deprecated that is not a function (parsed, body) => ids`);
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
    for (const m of p.missing) problems.push(`  missing upstream: ${m.model} (not found: ${(m.absent ?? m.vendorIds).join(', ')})`);
    for (const d of p.deprecated ?? []) {
      problems.push(`  deprecated upstream: ${d.model} (${d.vendorIds.join(', ')}) → still served, but models/*.yaml has no deprecated_at or retires_at for it`);
    }
    for (const key of p.changed) problems.push(`  changed: ${key} → git diff scripts/model-drift/snapshots/${p.provider}/${key}.txt`);
    // The path is relative to the snapshots folder the run used (--snapshots, or snapshots/).
    for (const key of p.noBaseline ?? []) problems.push(`  no baseline: ${key} → no earlier snapshot to compare; read all of ${p.provider}/${key}.txt`);
    if (problems.length) lines.push(p.provider, ...problems);
    else clean.push(p.provider);
  }
  if (clean.length) lines.push('', `clean: ${clean.join(', ')}`);
  return lines.join('\n') + '\n';
}
