import type { ModelInfo, ModelSchema, ParamSpec, ImageGenerationRequest, ReferenceImage } from '@litegen/sdk';
import type { Availability, MergedExtra, MergedParam, MergedRole, RefEntry, SharedFormState } from './types';

const MOCK_PROVIDER = 'mock';

/** Loosely-typed spec access — ParamSpec is a kind-tagged union whose nested
 *  size/mode shape is awkward to narrow; we read fields defensively (same style
 *  as the existing Playground). */
type AnySpec = { kind: string; [k: string]: unknown };

/** The slice of a served `ModelSchema` the Playground reads, all optional:
 *  schemas arrive as unvalidated JSON, so every field is checked before use. */
interface LooseSchema {
  params?: Record<string, AnySpec>;
  prompt?: { required?: boolean; max_length?: number | null };
  ref_inputs?: {
    max_total?: number;
    roles?: Record<string, { required?: boolean; min_count?: number; max_count?: number }>;
  } | null;
  extra_allowlist?: string[];
}

const loose = (schema: ModelSchema | undefined): LooseSchema => (schema ?? {}) as unknown as LooseSchema;

export function availabilityOf(m: ModelInfo): Availability {
  if (m.provider === MOCK_PROVIDER) return 'mock';
  return m.is_available ? 'live' : 'setup';
}

const RANK: Record<Availability, number> = { live: 0, mock: 1, setup: 2 };

/** Sort live → mock → setup, then alphabetical by id. */
export function sortModels(models: ModelInfo[]): ModelInfo[] {
  return [...models].sort((a, b) => {
    const r = RANK[availabilityOf(a)] - RANK[availabilityOf(b)];
    return r !== 0 ? r : a.id.localeCompare(b.id);
  });
}

/** ["WxH", ...] options for a size param of mode "enum"; [] otherwise. */
export function sizeEnumOptions(spec: ParamSpec): string[] {
  const s = spec as unknown as AnySpec;
  if (s.kind === 'size' && s.mode === 'enum' && Array.isArray(s.values)) {
    return (s.values as Array<[number, number]>).map(v => `${v[0]}x${v[1]}`);
  }
  return [];
}

/** A non-empty trimmed string, else null. Spec fields come from an
 *  unvalidated schema payload, so a non-string is treated as absent. */
function nonEmpty(v: unknown): string | null {
  if (typeof v !== 'string') return null;
  const t = v.trim();
  return t === '' ? null : t;
}

function stringList(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
}

/**
 * Human-facing label for a param row: the schema's declared `label` (every
 * ParamSpec variant may carry one so clients need not show raw API keys),
 * else the key humanised — `target_polycount` → `Target polycount`.
 */
export function paramLabel(name: string, spec: ParamSpec): string {
  const declared = nonEmpty((spec as unknown as AnySpec).label);
  if (declared) return declared;
  const words = name.split('_').filter(Boolean).join(' ').toLowerCase();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** The schema's declared help text for a param, or null when it has none. */
export function paramDescription(spec: ParamSpec): string | null {
  return nonEmpty((spec as unknown as AnySpec).description);
}

/**
 * What a control shows before the user touches it: the schema's declared
 * default where there is one, else "unset" ('' — rendered as a placeholder or
 * a "Model default" option). Display only — an untouched param is never sent
 * (see `SharedFormState.params`), so nothing here can override a model's own
 * default. That is why there is no fallback to an int's minimum or an enum's
 * first member: the old fallback put `target_polycount: 100` on every 3D
 * request and `steps` at its minimum on image ones.
 */
export function defaultForSpec(spec: ParamSpec): unknown {
  const s = spec as unknown as AnySpec;
  switch (s.kind) {
    case 'bool': return typeof s.default === 'boolean' ? s.default : false;
    case 'int':
    case 'float': return typeof s.default === 'number' ? s.default : '';
    case 'string': return typeof s.default === 'string' ? s.default : '';
    case 'string_array': {
      // The spec's `default` is already an array here. Without one the server
      // resolves to glb (every 3D model can emit it), so that is what the
      // control shows; failing that, the first declared value — never all of
      // them: each extra container is a file the caller pays for and a format
      // the generation can now fail on.
      const d = stringList(s.default);
      if (d.length) return d;
      const ev = stringList(s.enum_values);
      if (ev.includes('glb')) return ['glb'];
      return ev.length ? [ev[0]] : [];
    }
    case 'aspect_ratio': return typeof s.default === 'string' ? s.default : '';
    case 'size': return '';
    case 'seed': return ''; // empty = random
    default: return '';
  }
}

// ─── output_formats: the provider-job budget ─────────────────────────────────
//
// `max_items` on a set param counts PROVIDER JOBS, not deliverables. litegen
// derives containers locally from any one it can read, and the server lists
// exactly those in `derivable_values` (served on GET /v1/models/{id}): any
// subset of them costs ONE job. A format litegen cannot convert would be a job
// of its own. Since litegen converts between all ten of its containers
// (docs/superpowers/3d-format-conversion.md) every served format is derivable
// and the cap cannot bind — `allShareOneJob` — but the rule stands for a model
// that offers one it cannot. Which formats are derivable is the server's
// knowledge; the client only applies the rule it is handed.

/** The formats the server says share one provider job; [] when it says none. */
export function derivableValues(spec: ParamSpec | AnySpec): string[] {
  return stringList((spec as AnySpec).derivable_values);
}

/** How many provider jobs a set of formats costs. */
export function vendorJobsNeeded(selected: string[], derivable: string[]): number {
  const shared = new Set(derivable);
  const ownJob = selected.filter(f => !shared.has(f)).length;
  return ownJob + (selected.some(f => shared.has(f)) ? 1 : 0);
}

/**
 * Whether every format on offer shares the one provider job — so no selection
 * can exceed the budget. Since litegen converts between all its containers
 * locally, that is the common case: the cap is worth stating only when it can
 * actually bind.
 */
export function allShareOneJob(spec: ParamSpec | AnySpec): boolean {
  const ev = stringList((spec as AnySpec).enum_values);
  const shared = new Set(derivableValues(spec));
  return ev.length > 0 && ev.every(f => shared.has(f));
}

/** Whether adding `candidate` to `chosen` stays inside the spec's job budget. */
export function canAddToSet(spec: ParamSpec | AnySpec, chosen: string[], candidate: string): boolean {
  const max = (spec as AnySpec).max_items;
  if (typeof max !== 'number') return true;
  return vendorJobsNeeded([...chosen, candidate], derivableValues(spec)) <= max;
}

function intersect(a: string[], b: string[]): string[] {
  const set = new Set(b);
  return a.filter(x => set.has(x));
}
function maxDefined(a?: number, b?: number): number | undefined {
  if (a == null) return b;
  if (b == null) return a;
  return Math.max(a, b);
}
function minDefined(a?: number, b?: number): number | undefined {
  if (a == null) return b;
  if (b == null) return a;
  return Math.min(a, b);
}

/** Merge two specs of the SAME kind into the most-restrictive single spec. */
function mergeSpec(a: AnySpec, b: AnySpec): AnySpec {
  const out: AnySpec = { ...a };
  if ('min' in a || 'min' in b) out.min = maxDefined(a.min as number, b.min as number);
  if ('max' in a || 'max' in b) out.max = minDefined(a.max as number, b.max as number);
  if (Array.isArray(a.enum_values) && Array.isArray(b.enum_values)) {
    out.enum_values = intersect(a.enum_values as string[], b.enum_values as string[]);
  }
  if (Array.isArray(a.allowed) && Array.isArray(b.allowed)) {
    out.allowed = intersect(a.allowed as string[], b.allowed as string[]);
  }
  if (a.kind === 'string_array' && b.kind === 'string_array') {
    // Most-restrictive wins, same as numeric bounds: offering a cap one of the
    // selected models cannot honour would let a Compare run be rejected at
    // submit with `param_too_many`.
    out.max_items = minDefined(a.max_items as number, b.max_items as number);
    // A format shares a job only if it does so on EVERY selected model; a
    // model that declares no derivable set counts each format as its own job.
    if (Array.isArray(a.derivable_values) && Array.isArray(b.derivable_values)) {
      out.derivable_values = intersect(derivableValues(a), derivableValues(b));
    } else {
      delete out.derivable_values;
    }
    // `enum_values` was intersected above, so a default carried over from `a`
    // can now name a container no longer on offer.
    if (Array.isArray(out.default) && Array.isArray(out.enum_values)) {
      out.default = intersect(out.default as string[], out.enum_values as string[]);
    }
  }
  if (a.kind === 'size' && b.kind === 'size' && a.mode === 'enum' && b.mode === 'enum') {
    const ao = (a.values as Array<[number, number]>).map(v => `${v[0]}x${v[1]}`);
    const bo = new Set((b.values as Array<[number, number]>).map(v => `${v[0]}x${v[1]}`));
    out.values = (a.values as Array<[number, number]>).filter((_, i) => bo.has(ao[i]));
  }
  return out;
}

/** Params with dedicated controls of their own, never rendered as a merged row. */
const DEDICATED = new Set(['seed']);

/** Union of params across selected schemas. One MergedParam per distinct name;
 *  numeric bounds intersected, enum/allowed intersected, applicability tracked.
 *  `skip` names params the caller renders its own control for. */
export function mergeParams(
  schemasById: Record<string, ModelSchema>,
  skip: ReadonlySet<string> = DEDICATED,
): MergedParam[] {
  const byName = new Map<string, { spec: AnySpec; models: string[] }>();
  for (const [modelId, schema] of Object.entries(schemasById)) {
    const params = loose(schema).params;
    if (!params) continue;
    for (const [name, rawSpec] of Object.entries(params)) {
      if (skip.has(name)) continue;
      const spec = rawSpec as AnySpec;
      const existing = byName.get(name);
      if (!existing) {
        byName.set(name, { spec: { ...spec }, models: [modelId] });
      } else {
        if (existing.spec.kind === spec.kind) existing.spec = mergeSpec(existing.spec, spec);
        existing.models.push(modelId);
      }
    }
  }
  // Stable order: prompt-adjacent common params first, then alpha.
  const PRIORITY = ['negative_prompt', 'output_formats', 'size', 'aspect_ratio', 'style', 'quality', 'steps', 'guidance_scale', 'strength'];
  return [...byName.entries()]
    .map(([name, v]) => ({ name, spec: v.spec as unknown as ParamSpec, models: v.models }))
    .sort((a, b) => {
      const ia = PRIORITY.indexOf(a.name);
      const ib = PRIORITY.indexOf(b.name);
      if (ia !== -1 || ib !== -1) return (ia === -1 ? 99 : ia) - (ib === -1 ? 99 : ib);
      return a.name.localeCompare(b.name);
    });
}

/** Clamp a numeric value to a spec's min/max; passthrough otherwise. */
export function clampToSpec(spec: AnySpec, value: unknown): unknown {
  if ((spec.kind === 'int' || spec.kind === 'float') && typeof value === 'number') {
    let v = value;
    if (typeof spec.min === 'number') v = Math.max(spec.min, v);
    if (typeof spec.max === 'number') v = Math.min(spec.max, v);
    return v;
  }
  // Compare mode shares one value across models that declare different sets, so
  // a `string_array` has to be narrowed to each model's own — otherwise picking
  // `stl` for one model makes every other model in the run fail validation with
  // `param_enum_mismatch` before anything is generated. Then the job budget:
  // formats are kept in order while they still fit, so a set one model could
  // serve from a single job is not cut down for it.
  if (spec.kind === 'string_array' && Array.isArray(value)) {
    const ev = stringList(spec.enum_values);
    const out: string[] = [];
    for (const f of value as string[]) {
      if (ev.includes(f) && canAddToSet(spec, out, f)) out.push(f);
    }
    return out;
  }
  return value;
}

// ─── Prompt ──────────────────────────────────────────────────────────────────

/** Whether the model rejects an empty prompt. Absent means required — the
 *  server's own default for `PromptSpec.required`. */
export function promptRequired(schema: ModelSchema | undefined): boolean {
  return loose(schema).prompt?.required !== false;
}

/**
 * What the prompt means to this model: `required`; `optional` (text OR image
 * in, like Rodin); or `unused` — the model takes no prompt at all (TRELLIS,
 * Hunyuan3D v2 generate from the input image alone). The server says so with
 * `prompt.max_length == 0`, and treats a non-empty prompt there as an
 * unsupported param: a 400 in strict mode, dropped in lax. So an `unused`
 * prompt is never sent (see `buildRequestForModel`).
 */
export function promptUsage(schema: ModelSchema | undefined): 'required' | 'optional' | 'unused' {
  if (loose(schema).prompt?.max_length === 0) return 'unused';
  return promptRequired(schema) ? 'required' : 'optional';
}

/** The model's prompt limit in characters, or null for none (or no prompt at all). */
export function promptMaxLength(schema: ModelSchema | undefined): number | null {
  const max = loose(schema).prompt?.max_length;
  return typeof max === 'number' && max > 0 ? max : null;
}

/** A prompt's length as the server counts it: characters (Unicode scalar
 *  values), not UTF-16 code units — an emoji is one, not two. */
export function promptLength(prompt: string): number {
  return [...prompt].length;
}

/** The prompt one model is sent: none at all when it takes no prompt, and
 *  none when blank (the field is optional on the wire). */
export function promptForModel(schema: ModelSchema | undefined, prompt: string): string | undefined {
  if (promptUsage(schema) === 'unused' || prompt.trim() === '') return undefined;
  return prompt;
}

// ─── Reference images ────────────────────────────────────────────────────────

/** Display order: the image-to-3D/image-to-image input first, then views in a
 *  walk around the subject, then anything else alphabetically. */
const ROLE_ORDER = ['init', 'view-front', 'view-left', 'view-back', 'view-right', 'view-top', 'view-bottom'];

function roleRank(role: string): number {
  const i = ROLE_ORDER.indexOf(role);
  return i === -1 ? ROLE_ORDER.length : i;
}

/** Human label for a role key: `init` → "Input image", `view-front` → "Front view". */
export function roleLabel(role: string): string {
  if (role === 'init') return 'Input image';
  if (role.startsWith('view-')) {
    const side = role.slice('view-'.length).replace(/[-_]/g, ' ');
    return `${side.charAt(0).toUpperCase()}${side.slice(1)} view`;
  }
  const words = role.replace(/[-_]/g, ' ');
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** Union of reference-image roles across the selected models (see `MergedRole`). */
export function mergeRefRoles(schemasById: Record<string, ModelSchema>): MergedRole[] {
  const byRole = new Map<string, MergedRole>();
  for (const [modelId, schema] of Object.entries(schemasById)) {
    const roles = loose(schema).ref_inputs?.roles;
    if (!roles) continue;
    for (const [role, spec] of Object.entries(roles)) {
      const max = typeof spec.max_count === 'number' ? spec.max_count : 0;
      const min = typeof spec.min_count === 'number' ? spec.min_count : 0;
      const requires = spec.required === true && min > 0;
      const entry = byRole.get(role);
      if (!entry) {
        byRole.set(role, {
          role,
          maxCount: max,
          minCount: requires ? min : 0,
          models: [modelId],
          requiredBy: requires ? [modelId] : [],
        });
      } else {
        entry.maxCount = Math.min(entry.maxCount, max);
        entry.models.push(modelId);
        if (requires) {
          entry.minCount = Math.max(entry.minCount, min);
          entry.requiredBy.push(modelId);
        }
      }
    }
  }
  return [...byRole.values()].sort((a, b) => roleRank(a.role) - roleRank(b.role) || a.role.localeCompare(b.role));
}

/** The most reference images any one request may carry across the selection. */
export function maxTotalRefs(schemasById: Record<string, ModelSchema>): number | undefined {
  let out: number | undefined;
  for (const schema of Object.values(schemasById)) {
    const t = loose(schema).ref_inputs?.max_total;
    if (typeof t === 'number') out = out == null ? t : Math.min(out, t);
  }
  return out;
}

/** The reference images one model is sent: only roles it declares, each capped
 *  at the model's own per-role and total limits, in the order they were added. */
export function refsForModel(schema: ModelSchema | undefined, refs: RefEntry[]): ReferenceImage[] {
  const ri = loose(schema).ref_inputs;
  if (!ri?.roles) return [];
  const perRole = new Map<string, number>();
  const out: ReferenceImage[] = [];
  for (const r of refs) {
    const spec = ri.roles[r.role];
    if (!spec) continue;
    const n = perRole.get(r.role) ?? 0;
    if (typeof spec.max_count === 'number' && n >= spec.max_count) continue;
    if (typeof ri.max_total === 'number' && out.length >= ri.max_total) break;
    perRole.set(r.role, n + 1);
    out.push({ type: r.type, value: r.value, role: r.role });
  }
  return out;
}

// ─── Provider passthrough (`extra`) ──────────────────────────────────────────

/** Union of `extra_allowlist` keys across the selected models. */
export function mergeExtras(schemasById: Record<string, ModelSchema>): MergedExtra[] {
  const byKey = new Map<string, string[]>();
  for (const [modelId, schema] of Object.entries(schemasById)) {
    for (const key of stringList(loose(schema).extra_allowlist)) {
      const models = byKey.get(key);
      if (models) models.push(modelId);
      else byKey.set(key, [modelId]);
    }
  }
  return [...byKey.entries()]
    .map(([key, models]) => ({ key, models }))
    .sort((a, b) => a.key.localeCompare(b.key));
}

/**
 * A passthrough value as typed: JSON when it parses (`512`, `true`,
 * `{"a":1}`, `"512"` for a string that looks like a number), the raw text
 * otherwise, so `high` needs no quotes. Blank is "not set".
 */
export function parseExtraValue(raw: string): unknown {
  const t = raw.trim();
  if (t === '') return undefined;
  try {
    return JSON.parse(t);
  } catch {
    return t;
  }
}

/** The `extra` object one model is sent: only its allowlisted keys, blanks
 *  omitted. Undefined when nothing applies, so the field stays off the wire. */
export function extraForModel(
  schema: ModelSchema | undefined,
  extra: Record<string, string>,
): Record<string, unknown> | undefined {
  const allowed = new Set(stringList(loose(schema).extra_allowlist));
  const out: Record<string, unknown> = {};
  for (const [key, raw] of Object.entries(extra)) {
    if (!allowed.has(key)) continue;
    const v = parseExtraValue(raw);
    if (v !== undefined) out[key] = v;
  }
  return Object.keys(out).length ? out : undefined;
}

// ─── Request building ────────────────────────────────────────────────────────

/**
 * Why this model cannot be run with the current inputs, in words a user can act
 * on — empty when it can. Mirrors the server's own checks (check_prompt,
 * check_refs) so a run that would be rejected is blocked before it is sent.
 */
export function requestProblems(
  schema: ModelSchema | undefined,
  shared: Pick<SharedFormState, 'prompt' | 'refs'>,
): string[] {
  const problems: string[] = [];
  const sent = refsForModel(schema, shared.refs);
  const roles = loose(schema).ref_inputs?.roles ?? {};
  const minOf = (spec: { min_count?: number }) => (typeof spec.min_count === 'number' ? spec.min_count : 0);
  const requiresARole = Object.values(roles).some(spec => spec.required === true && minOf(spec) > 0);
  const usage = promptUsage(schema);
  // An `unused` prompt is simply not sent to this model, so it cannot be wrong.
  if (usage !== 'unused') {
    if (!shared.prompt.trim()) {
      if (usage === 'required') {
        problems.push('needs a prompt');
      } else if (sent.length === 0 && Object.keys(roles).length > 0 && !requiresARole) {
        // A prompt-optional model that takes images still needs SOMETHING to
        // generate from: with neither, the server reads the request as
        // text-to-3D and rejects it with prompt_required.
        problems.push('needs a prompt or a reference image');
      }
    }
    const max = promptMaxLength(schema);
    if (max != null && promptLength(shared.prompt) > max) {
      problems.push(`takes a prompt of at most ${max.toLocaleString()} characters`);
    }
  }
  for (const [role, spec] of Object.entries(roles)) {
    const min = minOf(spec);
    const count = sent.filter(r => r.role === role).length;
    // The server's rule exactly: an optional role may be left empty, but once
    // used it must reach its minimum; a required one must always.
    if (count < min && (spec.required === true || count > 0)) {
      problems.push(role === 'init'
        ? (min === 1 ? 'needs an input image' : `needs ${min} input images`)
        : `needs ${min} image${min === 1 ? '' : 's'} for the ${roleLabel(role).toLowerCase()}`);
    }
  }
  return problems;
}

/** Build a schema-valid request for one model: shared values, minus params the
 *  model doesn't declare, clamped to that model's own bounds. `mediaType`
 *  decides the family-specific fields (`n` is image-only: one 3D job is one
 *  result, billed flat). */
export function buildRequestForModel(
  modelId: string,
  schema: ModelSchema,
  shared: SharedFormState,
  mediaType: 'image' | 'model3d' = 'image',
): ImageGenerationRequest {
  const params = loose(schema).params ?? {};
  const body: Record<string, unknown> = { model: modelId };
  // One shared prompt box, but a model that takes no prompt is never sent it:
  // in strict mode that is a 400, which would sink its tile in a mixed run.
  const prompt = promptForModel(schema, shared.prompt);
  if (prompt !== undefined) body.prompt = prompt;
  body.strict = shared.strict;
  if (mediaType === 'image') body.n = shared.n;
  if (shared.seed !== '' && 'seed' in params) body.seed = parseInt(shared.seed, 10);
  return { ...body, ...schemaInputs(schema, shared) } as unknown as ImageGenerationRequest;
}

/**
 * The schema-driven half of a request — the params the user set, the
 * reference images and the passthrough `extra` — narrowed to what this one
 * model declares. Single Mode adds it to the fields it has dedicated controls
 * for; Compare Mode builds every request from it.
 */
export function schemaInputs(
  schema: ModelSchema | undefined,
  shared: Pick<SharedFormState, 'params'> & Partial<Pick<SharedFormState, 'refs' | 'extra'>>,
): Record<string, unknown> {
  const params = loose(schema).params ?? {};
  const body: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(shared.params)) {
    if (!(name in params)) continue;          // drop params this model doesn't support
    if (value === '' || value == null) continue;
    let v: unknown = value;
    if ((params[name].kind === 'int' || params[name].kind === 'float') && typeof value === 'string') {
      v = Number(value);
      if (!Number.isFinite(v)) continue;
    }
    v = clampToSpec(params[name], v);
    // An empty set means "unset", not "no formats": the API rejects an
    // explicitly empty list, and omitting the field is what makes the model's
    // own default apply. Narrowing above can empty it out for a model whose
    // declared set does not overlap the shared selection.
    if (Array.isArray(v) && v.length === 0) continue;
    body[name] = v;
  }
  const refs = refsForModel(schema, shared.refs ?? []);
  if (refs.length) body.reference_images = refs;
  const extra = extraForModel(schema, shared.extra ?? {});
  if (extra) body.extra = extra;
  return body;
}

// ─── Restoring a stored request (Playground history) ─────────────────────────

/** Request fields that are not schema params: the base request, plus the
 *  image family's fixed fields. */
const NON_PARAM_FIELDS = new Set([
  'model', 'prompt', 'negative_prompt', 'seed', 'size', 'n', 'strict',
  'reference_images', 'extra', 'metadata', 'response_format',
]);

/** The schema params a stored request body set, ready for `SharedFormState.params`. */
export function paramsFromBody(body: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(body)) {
    if (!NON_PARAM_FIELDS.has(k) && v != null) out[k] = v;
  }
  return out;
}

/** A stored body's URL reference images as picker entries. Inline ones were
 *  never stored (see `withoutInlineRefs`). */
export function refsFromBody(body: Record<string, unknown>): RefEntry[] {
  const refs = Array.isArray(body.reference_images) ? body.reference_images : [];
  return refs.flatMap((r, i) => {
    const ref = r as { type?: unknown; value?: unknown; role?: unknown } | null;
    if (ref?.type !== 'url' || typeof ref.value !== 'string' || typeof ref.role !== 'string') return [];
    return [{ id: `restored-${i}`, role: ref.role, type: 'url' as const, value: ref.value }];
  });
}

/** A passthrough value back to the text that `parseExtraValue` turns into it:
 *  a string that would re-parse as JSON (`"512"`) keeps its quotes. */
export function extraToRaw(value: unknown): string {
  if (typeof value === 'string') return parseExtraValue(value) === value ? value : JSON.stringify(value);
  return JSON.stringify(value);
}

/** A stored body's `extra` as the picker's raw text per key. */
export function extraFromBody(body: Record<string, unknown>): Record<string, string> {
  const extra = body.extra;
  if (typeof extra !== 'object' || extra === null || Array.isArray(extra)) return {};
  return Object.fromEntries(Object.entries(extra).map(([k, v]) => [k, extraToRaw(v)]));
}

/** The model's vendor API reference (`docs_url`), when it is a real https link. */
export function docsUrlOf(schema: ModelSchema | undefined): string | null {
  const url = (schema as unknown as { docs_url?: unknown } | undefined)?.docs_url;
  if (typeof url !== 'string') return null;
  try {
    return new URL(url).protocol === 'https:' ? url : null;
  } catch {
    return null;
  }
}
