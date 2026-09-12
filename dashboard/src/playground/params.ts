import type { ModelInfo, ModelSchema, ParamSpec, ImageGenerationRequest } from '@litegen/sdk';
import type { Availability, MergedParam, SharedFormState } from './types';

const MOCK_PROVIDER = 'mock';

/** Loosely-typed spec access — ParamSpec is a kind-tagged union whose nested
 *  size/mode shape is awkward to narrow; we read fields defensively (same style
 *  as the existing Playground). */
type AnySpec = { kind: string; [k: string]: unknown };

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

/** Default UI value for a control given its spec. */
export function defaultForSpec(spec: ParamSpec): unknown {
  const s = spec as unknown as AnySpec;
  switch (s.kind) {
    case 'bool': return (s.default as boolean) ?? false;
    case 'int':
    case 'float': return (s.default as number) ?? (s.min as number) ?? '';
    case 'string': {
      const ev = (s.enum_values as string[]) ?? [];
      return (s.default as string) ?? (ev.length ? ev[0] : '');
    }
    case 'string_array': {
      // The spec's `default` is already an array here. Fall back to the first
      // declared value rather than to everything: each extra container is a
      // file the caller pays for and a format the generation can now fail on,
      // so ticking them all by default would opt everyone into both.
      const d = s.default as string[] | undefined;
      if (Array.isArray(d) && d.length) return d;
      const ev = (s.enum_values as string[]) ?? [];
      return ev.length ? [ev[0]] : [];
    }
    case 'aspect_ratio': {
      const allowed = (s.allowed as string[]) ?? [];
      return (s.default as string) ?? (allowed[0] ?? '');
    }
    case 'size': {
      const opts = sizeEnumOptions(spec);
      if (opts.length) return opts[0];
      return s.min_width != null ? `${s.min_width}x${s.min_height}` : '';
    }
    case 'seed': return ''; // empty = random
    default: return '';
  }
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

/** Union of params across selected schemas. One MergedParam per distinct name;
 *  numeric bounds intersected, enum/allowed intersected, applicability tracked. */
export function mergeParams(schemasById: Record<string, ModelSchema>): MergedParam[] {
  const byName = new Map<string, { spec: AnySpec; models: string[] }>();
  for (const [modelId, schema] of Object.entries(schemasById)) {
    const params = (schema as Record<string, unknown>).params as Record<string, AnySpec> | undefined;
    if (!params) continue;
    for (const [name, rawSpec] of Object.entries(params)) {
      if (name === 'seed') continue;   // seed has a dedicated control in the panel
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
  const PRIORITY = ['negative_prompt', 'size', 'aspect_ratio', 'style', 'quality', 'steps', 'guidance_scale', 'strength'];
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
  // `param_enum_mismatch` before anything is generated.
  if (spec.kind === 'string_array' && Array.isArray(value)) {
    const ev = (spec.enum_values as string[]) ?? [];
    let v = (value as string[]).filter(x => ev.includes(x));
    const max = spec.max_items as number | undefined;
    if (max != null && v.length > max) v = v.slice(0, max);
    return v;
  }
  return value;
}

/** Build a schema-valid request for one model: shared values, minus params the
 *  model doesn't declare, clamped to that model's own bounds. */
export function buildRequestForModel(
  modelId: string,
  schema: ModelSchema,
  shared: SharedFormState,
): ImageGenerationRequest {
  const params = ((schema as Record<string, unknown>).params as Record<string, AnySpec>) ?? {};
  const body: Record<string, unknown> = {
    model: modelId,
    prompt: shared.prompt,
    n: shared.n,
    strict: shared.strict,
  };
  if (shared.seed !== '' && 'seed' in params) body.seed = parseInt(shared.seed, 10);
  for (const [name, value] of Object.entries(shared.params)) {
    if (!(name in params)) continue;          // drop params this model doesn't support
    if (value === '' || value == null) continue;
    let v: unknown = value;
    if ((params[name].kind === 'int' || params[name].kind === 'float') && typeof value === 'string') {
      v = Number(value);
    }
    v = clampToSpec(params[name], v);
    // An empty set means "unset", not "no formats": the API rejects an
    // explicitly empty list, and omitting the field is what makes the model's
    // own default apply. Narrowing above can empty it out for a model whose
    // declared set does not overlap the shared selection.
    if (Array.isArray(v) && v.length === 0) continue;
    body[name] = v;
  }
  return body as ImageGenerationRequest;
}
