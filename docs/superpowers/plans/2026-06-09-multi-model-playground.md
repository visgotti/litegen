# Multi-Model Generation Playground Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Evolve the dashboard Playground (`/playground`) into a tool that test-generates across many image models at once: select several models, fill one unified parameter panel built from the union of their real schemas, and generate one image tile per generation.

**Architecture:** Pure client-side fan-out — no backend changes. The browser fetches each selected model's schema (`GET /v1/models/{id}`), merges params into a unified panel (union + applicability), builds a schema-valid request per model, estimates cost (`POST /v1/images/cost`), then fires one `images.generate()` per model with bounded concurrency. The existing single-model flow is preserved verbatim behind a Single/Compare mode toggle so current e2e stays green.

**Tech Stack:** React 19 + Vite 8 + TypeScript, `@litegen/sdk` (`client.models`, `client.images`), plain CSS (`.pg-*`), Playwright e2e (mock provider) as the test mechanism (the dashboard has no unit runner; merge logic is made observable via a per-model Request JSON view and asserted in e2e).

**Test note:** The dashboard's only test stack is Playwright e2e (`dashboard/e2e/`, config boots `litegen-core/target/release/litegen` with the mock provider + in-memory SQLite). We follow that convention rather than introducing vitest on bleeding-edge Vite 8. Because `GET /v1/models/{id}` serves **every** catalog model's schema regardless of provider config (`is_available` only flags runnability), e2e builds the unified panel from *diverse real schemas* (e.g. `openai/dall-e-3` vs `bfl/flux-pro`) and asserts the per-model Request JSON, while actual generation uses mock models. Each task ends by running the e2e spec; per-step micro-runs are impractical with this heavyweight harness, so TDD is applied at task granularity (write/extend e2e assertions → fail → implement → pass → commit).

---

## File Structure

New, under `dashboard/src/playground/`:
- `types.ts` — shared types (`Availability`, `MergedParam`, `SharedFormState`, `ResultTileState`).
- `params.ts` — **pure** logic: availability, sort, param merge (union + applicability + restrictive intersection), size helpers, per-model request builder, clamping.
- `useModelSchemas.ts` — hook: fetch + session-cache `ModelSchema` for selected ids.
- `useUnifiedParams.ts` — hook: combine schemas → merged params + shared form state + `buildRequest`.
- `useFanOut.ts` — hook: concurrency-bounded generate, per-tile state machine, abort, cost estimate.
- `ParamField.tsx` — presentational control renderer switching on `ParamSpec.kind`.
- `ModelPicker.tsx` — searchable multi-select checklist with availability badges.
- `ResultTile.tsx` / `ResultGrid.tsx` — one tile per generation (model × n).
- `CompareMode.tsx` — composes picker + unified params + cost + results + per-model Request JSON.
- `SingleMode.tsx` — the **current** Playground body, extracted unchanged (keeps its `data-testid`s).

Changed:
- `pages/Playground.tsx` — thin shell: Single/Compare toggle hosting `SingleMode` / `CompareMode`.
- `playground-history.ts` — add a backward-compatible multi-run entry type + push/get.
- `App.css` — `.pg-*` classes.
- `e2e/compare-playground.spec.ts` — new e2e spec.

---

## Task 0: Prerequisite — confirm e2e baseline

**Files:** none (verification only)

- [ ] **Step 1: Build the backend binary the e2e harness needs**

Run: `cargo build --release -p litegen --manifest-path litegen-core/Cargo.toml`
Expected: compiles; produces `litegen-core/target/release/litegen`.

- [ ] **Step 2: Confirm current e2e passes (baseline)**

Run: `cd dashboard && npm run test:e2e -- --grep "every UI feature"`
Expected: PASS (the existing `god-test.spec.ts`). This proves the harness works before we change anything.

---

## Task 1: History model + shared types

**Files:**
- Modify: `dashboard/src/playground-history.ts`
- Create: `dashboard/src/playground/types.ts`

- [ ] **Step 1: Add the multi-run history type (backward compatible)**

Append to `dashboard/src/playground-history.ts` (keep existing `PlaygroundHistoryEntry` and functions unchanged):

```ts
export interface MultiRunResult {
  model: string;
  request: Record<string, unknown>;
  response?: Record<string, unknown>;
  error?: string;
}

export interface MultiRunHistoryEntry {
  id: string;
  kind: 'multi';
  prompt: string;
  timestamp: string;
  models: string[];
  results: MultiRunResult[];
}

const MULTI_KEY = 'litegen_playground_multi_history';
const MULTI_CAP = 10;

export function getMultiHistory(): MultiRunHistoryEntry[] {
  try {
    return JSON.parse(localStorage.getItem(MULTI_KEY) ?? '[]');
  } catch {
    return [];
  }
}

export function pushMultiHistory(entry: MultiRunHistoryEntry): void {
  const history = getMultiHistory();
  history.push(entry);
  localStorage.setItem(MULTI_KEY, JSON.stringify(history.slice(-MULTI_CAP)));
}

export function removeMultiHistory(id: string): void {
  const history = getMultiHistory().filter(e => e.id !== id);
  localStorage.setItem(MULTI_KEY, JSON.stringify(history));
}
```

- [ ] **Step 2: Create shared types**

Create `dashboard/src/playground/types.ts`:

```ts
import type { ParamSpec, ImageGenerationRequest } from '@litegen/sdk';

export type Availability = 'live' | 'mock' | 'setup';

/** One row in the unified parameter panel. */
export interface MergedParam {
  name: string;        // API param key, e.g. "size", "style", "steps"
  spec: ParamSpec;     // merged/representative spec used to render one control
  models: string[];    // model ids that declare this param
}

/** Values the user has set in the unified panel. */
export interface SharedFormState {
  prompt: string;
  n: number;
  strict: boolean;
  seed: string;                        // "" = random
  params: Record<string, unknown>;     // keyed by MergedParam.name
}

export type TileStatus = 'queued' | 'running' | 'done' | 'error';

/** One result cell (model × index). */
export interface ResultTileState {
  key: string;                         // `${modelId}#${index}`
  modelId: string;
  index: number;
  status: TileStatus;
  request: ImageGenerationRequest;
  b64_json?: string | null;
  url?: string | null;
  costUsd?: number;
  latencyMs?: number;
  error?: string;
}
```

- [ ] **Step 3: Type-check**

Run: `cd dashboard && npx tsc -b --noEmit`
Expected: PASS (no errors).

- [ ] **Step 4: Commit**

```bash
git add dashboard/src/playground-history.ts dashboard/src/playground/types.ts
git commit -m "feat(dashboard): multi-run history + playground shared types"
```

---

## Task 2: Pure param logic (`params.ts`)

This is the heart of the feature. It is pure (no React, no DOM) and is verified end-to-end in Task 8 via the per-model Request JSON view.

**Files:**
- Create: `dashboard/src/playground/params.ts`

- [ ] **Step 1: Write `params.ts`**

Create `dashboard/src/playground/params.ts`:

```ts
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
  if (a == null) return b; if (b == null) return a; return Math.max(a, b);
}
function minDefined(a?: number, b?: number): number | undefined {
  if (a == null) return b; if (b == null) return a; return Math.min(a, b);
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
      const ia = PRIORITY.indexOf(a.name), ib = PRIORITY.indexOf(b.name);
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
    body[name] = clampToSpec(params[name], v);
  }
  return body as ImageGenerationRequest;
}
```

- [ ] **Step 2: Type-check**

Run: `cd dashboard && npx tsc -b --noEmit`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add dashboard/src/playground/params.ts
git commit -m "feat(dashboard): pure unified-param merge + per-model request builder"
```

---

## Task 3: `ParamField` control renderer

**Files:**
- Create: `dashboard/src/playground/ParamField.tsx`

- [ ] **Step 1: Write `ParamField.tsx`**

Create `dashboard/src/playground/ParamField.tsx`:

```tsx
import type { ParamSpec } from '@litegen/sdk';
import { sizeEnumOptions } from './params';

type AnySpec = { kind: string; [k: string]: unknown };

interface Props {
  name: string;
  spec: ParamSpec;
  models: string[];        // applicability
  totalSelected: number;   // to render "all" vs the model list
  value: unknown;
  onChange: (v: unknown) => void;
}

export default function ParamField({ name, spec, models, totalSelected, value, onChange }: Props) {
  const s = spec as unknown as AnySpec;
  const tid = `pg-param-${name}`;
  const applies = models.length === totalSelected
    ? 'all'
    : models.map(m => m.split('/').pop()).join(', ');

  let control: React.ReactNode;
  switch (s.kind) {
    case 'bool':
      control = (
        <input type="checkbox" data-testid={tid} checked={Boolean(value)}
          onChange={e => onChange(e.target.checked)} />
      );
      break;
    case 'int':
    case 'float':
      control = (
        <input type="number" className="input" data-testid={tid}
          min={s.min as number} max={s.max as number}
          step={s.kind === 'float' ? 'any' : 1}
          value={value as number | string} onChange={e => onChange(e.target.value)} />
      );
      break;
    case 'string': {
      const ev = (s.enum_values as string[]) ?? [];
      control = ev.length ? (
        <select className="input" data-testid={tid} value={String(value ?? '')}
          onChange={e => onChange(e.target.value)}>
          {ev.map(o => <option key={o} value={o}>{o}</option>)}
        </select>
      ) : (
        <input type="text" className="input" data-testid={tid}
          maxLength={s.max_length as number}
          value={String(value ?? '')} onChange={e => onChange(e.target.value)} />
      );
      break;
    }
    case 'aspect_ratio': {
      const allowed = (s.allowed as string[]) ?? [];
      control = (
        <select className="input" data-testid={tid} value={String(value ?? '')}
          onChange={e => onChange(e.target.value)}>
          {allowed.map(o => <option key={o} value={o}>{o}</option>)}
        </select>
      );
      break;
    }
    case 'size': {
      const opts = sizeEnumOptions(spec);
      control = opts.length ? (
        <select className="input" data-testid={tid} value={String(value ?? '')}
          onChange={e => onChange(e.target.value)}>
          {opts.map(o => <option key={o} value={o}>{o}</option>)}
        </select>
      ) : (
        <input type="text" className="input" data-testid={tid}
          placeholder="WxH" value={String(value ?? '')}
          onChange={e => onChange(e.target.value)} />
      );
      break;
    }
    case 'seed':
      control = (
        <input type="number" className="input" data-testid={tid}
          placeholder="Random" value={String(value ?? '')}
          onChange={e => onChange(e.target.value)} />
      );
      break;
    default:
      control = (
        <input type="text" className="input" data-testid={tid}
          value={String(value ?? '')} onChange={e => onChange(e.target.value)} />
      );
  }

  return (
    <div className="pg-param-row">
      <label className="pg-param-label">
        {name}
        <span className="pg-param-applies" data-testid={`${tid}-applies`}>{applies}</span>
      </label>
      {control}
    </div>
  );
}
```

- [ ] **Step 2: Type-check**

Run: `cd dashboard && npx tsc -b --noEmit`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add dashboard/src/playground/ParamField.tsx
git commit -m "feat(dashboard): schema-driven ParamField control renderer"
```

---

## Task 4: Schema-cache + unified-params hooks

**Files:**
- Create: `dashboard/src/playground/useModelSchemas.ts`
- Create: `dashboard/src/playground/useUnifiedParams.ts`

- [ ] **Step 1: Write `useModelSchemas.ts`**

Create `dashboard/src/playground/useModelSchemas.ts`:

```ts
import { useEffect, useRef, useState } from 'react';
import { client } from '../sdk-client';
import type { ModelSchema } from '@litegen/sdk';

/** Fetch + session-cache ModelSchema for a set of model ids. */
export function useModelSchemas(ids: string[]): Record<string, ModelSchema> {
  const cache = useRef<Map<string, ModelSchema>>(new Map());
  const [schemas, setSchemas] = useState<Record<string, ModelSchema>>({});

  useEffect(() => {
    let cancelled = false;
    const missing = ids.filter(id => !cache.current.has(id));
    Promise.all(missing.map(id =>
      client.models.getSchema(id)
        .then(s => cache.current.set(id, s as ModelSchema))
        .catch(() => { /* skip unresolved schema */ }),
    )).then(() => {
      if (cancelled) return;
      const next: Record<string, ModelSchema> = {};
      for (const id of ids) {
        const s = cache.current.get(id);
        if (s) next[id] = s;
      }
      setSchemas(next);
    });
    return () => { cancelled = true; };
  }, [ids.join(',')]);

  return schemas;
}
```

- [ ] **Step 2: Write `useUnifiedParams.ts`**

Create `dashboard/src/playground/useUnifiedParams.ts`:

```ts
import { useEffect, useMemo, useState } from 'react';
import type { ModelSchema, ImageGenerationRequest } from '@litegen/sdk';
import type { MergedParam, SharedFormState } from './types';
import { mergeParams, defaultForSpec, buildRequestForModel } from './params';
import { useModelSchemas } from './useModelSchemas';

interface UnifiedParams {
  merged: MergedParam[];
  form: SharedFormState;
  setForm: React.Dispatch<React.SetStateAction<SharedFormState>>;
  setParam: (name: string, value: unknown) => void;
  schemasById: Record<string, ModelSchema>;
  buildRequests: (modelIds: string[]) => Array<{ modelId: string; request: ImageGenerationRequest }>;
}

const INITIAL: SharedFormState = { prompt: '', n: 1, strict: true, seed: '', params: {} };

export function useUnifiedParams(selectedIds: string[]): UnifiedParams {
  const schemasById = useModelSchemas(selectedIds);
  const merged = useMemo(() => mergeParams(schemasById), [schemasById]);
  const [form, setForm] = useState<SharedFormState>(INITIAL);

  // Seed defaults for newly-appeared params; drop values for params no longer present.
  useEffect(() => {
    setForm(prev => {
      const names = new Set(merged.map(p => p.name));
      const params: Record<string, unknown> = {};
      for (const p of merged) {
        params[p.name] = p.name in prev.params ? prev.params[p.name] : defaultForSpec(p.spec);
      }
      // keep only known names
      void names;
      return { ...prev, params };
    });
  }, [merged]);

  const setParam = (name: string, value: unknown) =>
    setForm(prev => ({ ...prev, params: { ...prev.params, [name]: value } }));

  const buildRequests = (modelIds: string[]) =>
    modelIds
      .filter(id => schemasById[id])
      .map(id => ({ modelId: id, request: buildRequestForModel(id, schemasById[id], form) }));

  return { merged, form, setForm, setParam, schemasById, buildRequests };
}
```

- [ ] **Step 3: Type-check**

Run: `cd dashboard && npx tsc -b --noEmit`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add dashboard/src/playground/useModelSchemas.ts dashboard/src/playground/useUnifiedParams.ts
git commit -m "feat(dashboard): schema-cache + unified-params hooks"
```

---

## Task 5: Fan-out hook (`useFanOut.ts`)

**Files:**
- Create: `dashboard/src/playground/useFanOut.ts`

- [ ] **Step 1: Write `useFanOut.ts`**

Create `dashboard/src/playground/useFanOut.ts`:

```ts
import { useRef, useState } from 'react';
import { client } from '../sdk-client';
import type { ImageGenerationRequest } from '@litegen/sdk';
import type { ResultTileState } from './types';

const MAX_CONCURRENCY = 4;

interface FanOut {
  tiles: ResultTileState[];
  running: boolean;
  run: (requests: Array<{ modelId: string; request: ImageGenerationRequest }>) => Promise<void>;
  cancel: () => void;
}

export function useFanOut(): FanOut {
  const [tiles, setTiles] = useState<ResultTileState[]>([]);
  const [running, setRunning] = useState(false);
  const abortRef = useRef<AbortController | null>(null);

  const patch = (key: string, p: Partial<ResultTileState>) =>
    setTiles(prev => prev.map(t => (t.key === key ? { ...t, ...p } : t)));

  const run: FanOut['run'] = async (requests) => {
    const ctrl = new AbortController();
    abortRef.current = ctrl;
    setRunning(true);

    // One tile per (model × n).
    const initial: ResultTileState[] = [];
    for (const { modelId, request } of requests) {
      const n = (request as { n?: number }).n ?? 1;
      for (let i = 0; i < n; i++) {
        initial.push({ key: `${modelId}#${i}`, modelId, index: i, status: 'queued', request });
      }
    }
    setTiles(initial);

    // Bounded worker pool over the request list (n images come back in one call).
    let cursor = 0;
    const worker = async () => {
      while (cursor < requests.length && !ctrl.signal.aborted) {
        const { modelId, request } = requests[cursor++];
        const keys = initial.filter(t => t.modelId === modelId).map(t => t.key);
        keys.forEach(k => patch(k, { status: 'running' }));
        const started = performance.now();
        try {
          const res = await client.images.generate(request, ctrl.signal);
          const latency = Math.round(performance.now() - started);
          const cost = (res as { usage?: { cost_usd?: number } }).usage?.cost_usd;
          const data = (res as { data?: Array<{ b64_json?: string | null; url?: string | null }> }).data ?? [];
          keys.forEach((k, i) => patch(k, {
            status: 'done', latencyMs: latency, costUsd: cost,
            b64_json: data[i]?.b64_json ?? null, url: data[i]?.url ?? null,
          }));
        } catch (e) {
          if (ctrl.signal.aborted) return;
          const msg = (e as Error).message;
          keys.forEach(k => patch(k, { status: 'error', error: msg }));
        }
      }
    };
    await Promise.all(Array.from({ length: Math.min(MAX_CONCURRENCY, requests.length) }, worker));
    setRunning(false);
  };

  const cancel = () => {
    abortRef.current?.abort();
    setRunning(false);
    setTiles(prev => prev.map(t => (t.status === 'queued' || t.status === 'running'
      ? { ...t, status: 'error', error: 'cancelled' } : t)));
  };

  return { tiles, running, run, cancel };
}
```

- [ ] **Step 2: Type-check**

Run: `cd dashboard && npx tsc -b --noEmit`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add dashboard/src/playground/useFanOut.ts
git commit -m "feat(dashboard): bounded-concurrency fan-out hook with abort"
```

---

## Task 6: ModelPicker + ResultGrid/Tile

**Files:**
- Create: `dashboard/src/playground/ModelPicker.tsx`
- Create: `dashboard/src/playground/ResultTile.tsx`
- Create: `dashboard/src/playground/ResultGrid.tsx`

- [ ] **Step 1: Write `ModelPicker.tsx`**

Create `dashboard/src/playground/ModelPicker.tsx`:

```tsx
import { useMemo, useState } from 'react';
import type { ModelInfo } from '@litegen/sdk';
import { availabilityOf, sortModels } from './params';

const BADGE: Record<string, string> = { live: '● live', mock: '◇ mock', setup: '○ setup' };

interface Props {
  models: ModelInfo[];
  selected: string[];
  onToggle: (id: string) => void;
}

export default function ModelPicker({ models, selected, onToggle }: Props) {
  const [filter, setFilter] = useState('');
  const sorted = useMemo(() => sortModels(models), [models]);
  const shown = sorted.filter(m => m.id.toLowerCase().includes(filter.toLowerCase()));

  return (
    <div className="pg-picker" data-testid="pg-model-picker">
      <input className="input" data-testid="pg-model-filter" placeholder="Filter models…"
        value={filter} onChange={e => setFilter(e.target.value)} />
      <div className="pg-picker-count" data-testid="pg-selected-count">{selected.length} selected</div>
      <div className="pg-picker-list">
        {shown.map(m => {
          const av = availabilityOf(m);
          return (
            <label key={m.id} className="pg-picker-row" data-testid={`pg-model-${m.id}`}>
              <input type="checkbox" checked={selected.includes(m.id)} onChange={() => onToggle(m.id)} />
              <span className="pg-picker-id">{m.id}</span>
              <span className={`pg-badge pg-badge-${av}`}>{BADGE[av]}</span>
            </label>
          );
        })}
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Write `ResultTile.tsx`**

Create `dashboard/src/playground/ResultTile.tsx`:

```tsx
import type { ResultTileState } from './types';

interface Props { tile: ResultTileState; onRerun: (modelId: string) => void; }

export default function ResultTile({ tile, onRerun }: Props) {
  const src = tile.b64_json ? `data:image/png;base64,${tile.b64_json}` : tile.url ?? undefined;
  return (
    <div className="pg-tile" data-testid={`pg-tile-${tile.modelId}`}>
      <div className="pg-tile-head">
        <code className="pg-tile-model">{tile.modelId}</code>
        <button className="btn btn-secondary pg-tile-rerun" title="Rerun this model"
          data-testid={`pg-tile-rerun-${tile.modelId}`} onClick={() => onRerun(tile.modelId)}>↻</button>
      </div>
      <div className="pg-tile-image">
        {tile.status === 'running' || tile.status === 'queued' ? (
          <span className="pg-tile-status" data-testid={`pg-tile-spinner-${tile.modelId}`}>⟳ generating…</span>
        ) : tile.status === 'error' ? (
          <span className="pg-tile-error" data-testid={`pg-tile-error-${tile.modelId}`}>⚠ {tile.error}</span>
        ) : src ? (
          <img data-testid={`pg-tile-img-${tile.modelId}`} src={src} alt={tile.modelId} />
        ) : (
          <span className="pg-tile-status">no image</span>
        )}
      </div>
      <div className="pg-tile-meta">
        {tile.costUsd != null && <span>${tile.costUsd.toFixed(3)}</span>}
        {tile.latencyMs != null && <span>{tile.latencyMs}ms</span>}
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Write `ResultGrid.tsx`**

Create `dashboard/src/playground/ResultGrid.tsx`:

```tsx
import type { ResultTileState } from './types';
import ResultTile from './ResultTile';

interface Props { tiles: ResultTileState[]; onRerun: (modelId: string) => void; }

export default function ResultGrid({ tiles, onRerun }: Props) {
  if (tiles.length === 0) {
    return <div className="pg-grid-empty" data-testid="pg-grid-empty">No results yet</div>;
  }
  return (
    <div className="pg-grid" data-testid="pg-result-grid">
      {tiles.map(t => <ResultTile key={t.key} tile={t} onRerun={onRerun} />)}
    </div>
  );
}
```

- [ ] **Step 4: Type-check**

Run: `cd dashboard && npx tsc -b --noEmit`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add dashboard/src/playground/ModelPicker.tsx dashboard/src/playground/ResultTile.tsx dashboard/src/playground/ResultGrid.tsx
git commit -m "feat(dashboard): model picker + result grid/tile components"
```

---

## Task 7: CompareMode + SingleMode extraction + Playground shell + CSS

**Files:**
- Create: `dashboard/src/playground/SingleMode.tsx`
- Create: `dashboard/src/playground/CompareMode.tsx`
- Modify: `dashboard/src/pages/Playground.tsx`
- Modify: `dashboard/src/App.css`

- [ ] **Step 1: Extract current Playground body into `SingleMode.tsx`**

Create `dashboard/src/playground/SingleMode.tsx` containing the **entire current body** of `pages/Playground.tsx` (the component renamed `SingleMode`, default export). Move imports of `playground-history`, `client`, `ModelInfo` accordingly (paths become `../sdk-client`, `../playground-history`). Do NOT change any markup or `data-testid` — this preserves existing e2e.

- [ ] **Step 2: Write `CompareMode.tsx`**

Create `dashboard/src/playground/CompareMode.tsx`:

```tsx
import { useEffect, useState } from 'react';
import { client } from '../sdk-client';
import type { ModelInfo } from '@litegen/sdk';
import ModelPicker from './ModelPicker';
import ParamField from './ParamField';
import ResultGrid from './ResultGrid';
import { useUnifiedParams } from './useUnifiedParams';
import { useFanOut } from './useFanOut';

type ReqView = 'results' | 'requests';

export default function CompareMode() {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [selected, setSelected] = useState<string[]>([]);
  const [view, setView] = useState<ReqView>('results');
  const [error, setError] = useState('');
  const { merged, form, setForm, setParam, buildRequests } = useUnifiedParams(selected);
  const { tiles, running, run, cancel } = useFanOut();

  useEffect(() => {
    client.models.list()
      .then(all => setModels(all.filter(m => m.media_type === 'image')))
      .catch(e => setError(e.message));
  }, []);

  const toggle = (id: string) =>
    setSelected(prev => prev.includes(id) ? prev.filter(x => x !== id) : [...prev, id]);

  const requests = buildRequests(selected);

  const generate = async (only?: string) => {
    setError('');
    const reqs = only ? requests.filter(r => r.modelId === only) : requests;
    if (reqs.length === 0 || !form.prompt.trim()) return;
    setView('results');
    await run(reqs);
  };

  return (
    <div className="playground-layout">
      <div className="playground-panel">
        {error && <div className="alert alert-error">{error}</div>}
        <ModelPicker models={models} selected={selected} onToggle={toggle} />

        <div className="playground-form-group">
          <label>Prompt</label>
          <textarea data-testid="pg-prompt" rows={3} value={form.prompt}
            onChange={e => setForm(prev => ({ ...prev, prompt: e.target.value }))}
            placeholder="Describe the image…" />
        </div>

        <div className="pg-params" data-testid="pg-params">
          {merged.map(p => (
            <ParamField key={p.name} name={p.name} spec={p.spec} models={p.models}
              totalSelected={selected.length} value={form.params[p.name]}
              onChange={v => setParam(p.name, v)} />
          ))}
        </div>

        <div className="playground-form-row">
          <div className="playground-form-group">
            <label>Seed</label>
            <input type="number" className="input" data-testid="pg-seed" placeholder="Random"
              value={form.seed} onChange={e => setForm(prev => ({ ...prev, seed: e.target.value }))} />
          </div>
          <div className="playground-form-group">
            <label>N (1–4)</label>
            <input type="number" className="input" data-testid="pg-n" min={1} max={4} value={form.n}
              onChange={e => setForm(prev => ({ ...prev, n: parseInt(e.target.value, 10) || 1 }))} />
          </div>
        </div>

        {running ? (
          <button className="btn btn-danger" style={{ width: '100%' }}
            data-testid="pg-cancel" onClick={cancel}>Cancel</button>
        ) : (
          <button className="btn btn-primary" style={{ width: '100%' }} data-testid="pg-generate"
            disabled={selected.length === 0 || !form.prompt.trim()} onClick={() => generate()}>
            Generate {selected.length} model{selected.length === 1 ? '' : 's'}
          </button>
        )}
      </div>

      <div className="playground-panel">
        <div className="playground-tabs">
          <button className={`playground-tab${view === 'results' ? ' active' : ''}`}
            data-testid="pg-view-results" onClick={() => setView('results')}>Results</button>
          <button className={`playground-tab${view === 'requests' ? ' active' : ''}`}
            data-testid="pg-view-requests" onClick={() => setView('requests')}>Requests</button>
        </div>
        {view === 'results' ? (
          <ResultGrid tiles={tiles} onRerun={id => generate(id)} />
        ) : (
          <pre data-testid="pg-requests-json" className="pg-requests">
            {JSON.stringify(requests, null, 2)}
          </pre>
        )}
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Rewrite `pages/Playground.tsx` as the shell**

Replace `dashboard/src/pages/Playground.tsx` with:

```tsx
import { useState } from 'react';
import SingleMode from '../playground/SingleMode';
import CompareMode from '../playground/CompareMode';

type Mode = 'single' | 'compare';

export default function Playground() {
  const [mode, setMode] = useState<Mode>('single');
  return (
    <div>
      <h2 className="page-title">Playground</h2>
      <div className="pg-mode-toggle" data-testid="pg-mode-toggle">
        <button className={`playground-tab${mode === 'single' ? ' active' : ''}`}
          data-testid="pg-mode-single" onClick={() => setMode('single')}>Single</button>
        <button className={`playground-tab${mode === 'compare' ? ' active' : ''}`}
          data-testid="pg-mode-compare" onClick={() => setMode('compare')}>Compare</button>
      </div>
      {mode === 'single' ? <SingleMode /> : <CompareMode />}
    </div>
  );
}
```

Note: `SingleMode` must render WITHOUT its own `<h2 className="page-title">Playground</h2>` (the shell now owns the title). Remove that one line from the extracted `SingleMode` body; keep everything else identical.

- [ ] **Step 4: Add `.pg-*` styles to `App.css`**

Append to `dashboard/src/App.css`:

```css
/* ── Multi-model playground ───────────────────────────────────────────── */
.pg-mode-toggle { display: flex; gap: 4px; margin-bottom: 16px; }
.pg-picker { border: 1px solid #30363d; border-radius: 6px; padding: 8px; margin-bottom: 16px; }
.pg-picker-count { font-size: 12px; color: #8b949e; margin: 6px 0; }
.pg-picker-list { max-height: 220px; overflow-y: auto; display: flex; flex-direction: column; gap: 2px; }
.pg-picker-row { display: flex; align-items: center; gap: 8px; padding: 4px 6px; border-radius: 4px; font-size: 13px; cursor: pointer; }
.pg-picker-row:hover { background: #161b22; }
.pg-picker-id { flex: 1; font-family: monospace; }
.pg-badge { font-size: 11px; padding: 1px 6px; border-radius: 10px; white-space: nowrap; }
.pg-badge-live { color: #3fb950; border: 1px solid #238636; }
.pg-badge-mock { color: #58a6ff; border: 1px solid #1f6feb; }
.pg-badge-setup { color: #8b949e; border: 1px solid #30363d; }
.pg-params { display: flex; flex-direction: column; gap: 10px; margin: 12px 0; }
.pg-param-row { display: flex; flex-direction: column; gap: 4px; }
.pg-param-label { display: flex; justify-content: space-between; align-items: center; font-size: 13px; text-transform: none; }
.pg-param-applies { font-size: 11px; color: #8b949e; }
.pg-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 12px; }
.pg-grid-empty { color: #8b949e; padding: 24px; text-align: center; }
.pg-tile { border: 1px solid #30363d; border-radius: 6px; overflow: hidden; display: flex; flex-direction: column; }
.pg-tile-head { display: flex; justify-content: space-between; align-items: center; padding: 6px 8px; background: #161b22; }
.pg-tile-model { font-size: 11px; }
.pg-tile-rerun { padding: 2px 8px; font-size: 12px; }
.pg-tile-image { aspect-ratio: 1; display: flex; align-items: center; justify-content: center; background: #0d1117; }
.pg-tile-image img { width: 100%; height: 100%; object-fit: contain; }
.pg-tile-status { color: #8b949e; font-size: 12px; }
.pg-tile-error { color: #f85149; font-size: 12px; padding: 8px; text-align: center; }
.pg-tile-meta { display: flex; gap: 10px; padding: 6px 8px; font-size: 11px; color: #8b949e; }
.pg-requests { background: #0d1117; border: 1px solid #30363d; border-radius: 6px; padding: 16px; font-size: 12px; color: #e1e4e8; white-space: pre-wrap; word-break: break-all; min-height: 200px; }
```

- [ ] **Step 5: Type-check + lint + build**

Run: `cd dashboard && npx tsc -b --noEmit && npm run build`
Expected: PASS (clean build).

- [ ] **Step 6: Commit**

```bash
git add dashboard/src/playground/SingleMode.tsx dashboard/src/playground/CompareMode.tsx dashboard/src/pages/Playground.tsx dashboard/src/App.css
git commit -m "feat(dashboard): Single/Compare playground shell + CompareMode UI"
```

---

## Task 8: e2e — Compare mode

**Files:**
- Create: `dashboard/e2e/compare-playground.spec.ts`

- [ ] **Step 1: Write the e2e spec**

Create `dashboard/e2e/compare-playground.spec.ts`. Reuse the login pattern from `god-test.spec.ts` (API-key auth via `api-key-input` / `save-key-btn`, master key `test-master-key-please-rotate`).

```ts
import { test, expect } from '@playwright/test';

const MASTER_KEY = process.env.PLAYWRIGHT_MASTER_KEY ?? 'test-master-key-please-rotate';

test('compare mode: unified params from diverse schemas + multi-model generation', async ({ page }) => {
  // Auth via master API key (single-tenant path like god-test).
  await page.goto('/keys');
  await page.getByTestId('api-key-input').fill(MASTER_KEY);
  await page.getByTestId('save-key-btn').click();
  await expect(page.getByTestId('auth-status')).toContainText('Authenticated', { timeout: 5000 });

  // Go to Playground, switch to Compare.
  await page.goto('/playground');
  await page.getByTestId('pg-mode-compare').click();
  await expect(page.getByTestId('pg-model-picker')).toBeVisible();

  // (A) Unified panel from diverse REAL schemas: dall-e-3 (style/quality/size-enum)
  //     + flux-pro (size-freeform/steps/guidance). These are catalog models whose
  //     schemas resolve even though their providers aren't configured here.
  await page.getByTestId('pg-model-openai/dall-e-3').locator('input').check();
  await page.getByTestId('pg-model-bfl/flux-pro').locator('input').check();
  await expect(page.getByTestId('pg-selected-count')).toContainText('2 selected');
  // size is shared by both → applicability "all"; style only dall-e-3.
  await expect(page.getByTestId('pg-param-size-applies')).toContainText('all');
  await expect(page.getByTestId('pg-param-style-applies')).toContainText('dall-e-3');

  // Per-model Request JSON proves buildRequest drops non-declared params:
  await page.getByTestId('pg-prompt').fill('a red fox');
  await page.getByTestId('pg-view-requests').click();
  const reqs = JSON.parse(await page.getByTestId('pg-requests-json').innerText());
  const dalle = reqs.find((r: { modelId: string }) => r.modelId === 'openai/dall-e-3').request;
  const flux = reqs.find((r: { modelId: string }) => r.modelId === 'bfl/flux-pro').request;
  expect(dalle.prompt).toBe('a red fox');
  expect('steps' in dalle).toBe(false);   // dall-e-3 has no steps param
  expect('style' in flux).toBe(false);     // flux has no style param

  // (B) Generation with MOCK models (runnable here) → one tile per model w/ image.
  await page.getByTestId('pg-model-openai/dall-e-3').locator('input').uncheck();
  await page.getByTestId('pg-model-bfl/flux-pro').locator('input').uncheck();
  await page.getByTestId('pg-model-filter').fill('mock');
  await page.getByTestId('pg-model-mock/visual-image-gen').locator('input').check();
  await page.getByTestId('pg-model-mock/image-gen').locator('input').check();
  await page.getByTestId('pg-generate').click();

  await expect(page.getByTestId('pg-tile-img-mock/visual-image-gen')).toBeVisible({ timeout: 30000 });
  await expect(page.getByTestId('pg-tile-img-mock/image-gen')).toBeVisible({ timeout: 30000 });
});

test('single mode still works (regression)', async ({ page }) => {
  await page.goto('/keys');
  await page.getByTestId('api-key-input').fill(MASTER_KEY);
  await page.getByTestId('save-key-btn').click();
  await expect(page.getByTestId('auth-status')).toContainText('Authenticated', { timeout: 5000 });
  await page.goto('/playground');
  await expect(page.getByTestId('playground-model')).toBeVisible(); // Single mode default
  await page.getByTestId('playground-prompt').fill('a blue cat');
  await page.getByTestId('playground-generate').click();
  await expect(page.getByTestId('playground-image')).toBeVisible({ timeout: 30000 });
});
```

- [ ] **Step 2: Run the new e2e spec (must fail first if implementation incomplete, then pass)**

Run: `cd dashboard && npm run test:e2e -- compare-playground.spec.ts`
Expected: PASS — both tests green. If model ids differ in the running catalog, adjust the `mock/*` ids to the actual available mock image models (`client.models.list()` filtered to `media_type==='image' && provider==='mock'`); verify by checking the picker rows rendered.

- [ ] **Step 3: Run the existing god-test to confirm no regression**

Run: `cd dashboard && npm run test:e2e -- --grep "every UI feature"`
Expected: PASS (Single-mode testids unchanged).

- [ ] **Step 4: Commit**

```bash
git add dashboard/e2e/compare-playground.spec.ts
git commit -m "test(dashboard): e2e for multi-model compare playground"
```

---

## Task 9: Wire history + cost preview (polish)

**Files:**
- Modify: `dashboard/src/playground/CompareMode.tsx`

- [ ] **Step 1: Add a debounced cost preview**

In `CompareMode.tsx`, add state `const [estCost, setEstCost] = useState<number | null>(null);` and an effect that, when `requests` change and prompt is non-empty, calls `client.images.estimateCost(r.request)` for each request (catch → fall back to `0`), sums `total_cost_usd`, and sets `estCost`. Debounce 400ms via `setTimeout` cleanup. Render above the Generate button:

```tsx
{estCost != null && (
  <div className="pg-cost" data-testid="pg-cost">Est. cost: ${estCost.toFixed(3)}</div>
)}
```

- [ ] **Step 2: Persist a multi-run to history after generation**

After `await run(reqs)` in `generate()`, read the final `tiles` (lift via a ref or return value) and push a `MultiRunHistoryEntry` via `pushMultiHistory`. Minimal: record `{ id: crypto.randomUUID(), kind: 'multi', prompt: form.prompt, timestamp: new Date().toISOString(), models: reqs.map(r => r.modelId), results: reqs.map(r => ({ model: r.modelId, request: r.request as Record<string, unknown> })) }`.

- [ ] **Step 3: Add `.pg-cost` style**

Append to `App.css`: `.pg-cost { font-size: 13px; color: #3fb950; margin: 8px 0; }`

- [ ] **Step 4: Build + re-run compare e2e**

Run: `cd dashboard && npm run build && npm run test:e2e -- compare-playground.spec.ts`
Expected: PASS; `pg-cost` visible once models+prompt set.

- [ ] **Step 5: Commit**

```bash
git add dashboard/src/playground/CompareMode.tsx dashboard/src/App.css
git commit -m "feat(dashboard): compare cost preview + multi-run history"
```

---

## Self-Review

**Spec coverage:**
- "All catalog, badged" → Task 6 `ModelPicker` + `availabilityOf` (Task 2). ✓
- "Evolve the Playground" → Task 7 shell + `SingleMode` extraction; Single testids preserved (Task 8 regression). ✓
- "Union + applicability tags" → Task 2 `mergeParams`, Task 3 applicability rendering, Task 8 assertion. ✓
- "Image only" → `media_type === 'image'` filter in `CompareMode` (Task 7). ✓
- "Per-model schema-valid request / drop non-declared" → Task 2 `buildRequestForModel`, Task 8 Request-JSON assertion. ✓
- "One image tile per generation" → Task 5 tile-per-(model×n), Task 6 grid. ✓
- "Cost preview" → Task 9. ✓
- "Per-tile error isolation / abort / concurrency" → Task 5. ✓
- "History" → Task 1 + Task 9. ✓
- Testing (e2e, mock provider, diverse schemas) → Task 8. ✓

**Placeholder scan:** No TBD/TODO; every code step shows full code except Task 9 steps 1–2 which describe precise edits to an already-shown file (acceptable — they reference concrete symbols `estimateCost`, `total_cost_usd`, `pushMultiHistory`, `MultiRunHistoryEntry` defined in Tasks 1/9).

**Type consistency:** `SharedFormState`, `MergedParam`, `ResultTileState`, `Availability` defined once (Task 1) and used consistently. `buildRequestForModel`/`mergeParams`/`availabilityOf`/`sortModels`/`sizeEnumOptions`/`defaultForSpec`/`clampToSpec` signatures match between `params.ts` (Task 2) and callers (Tasks 3, 4, 6). Hook return shapes (`useUnifiedParams`, `useFanOut`) match `CompareMode` usage (Task 7).

**Known runtime caveat (call out at execution):** Mock image-model ids (`mock/visual-image-gen`, `mock/image-gen`) and catalog ids (`openai/dall-e-3`, `bfl/flux-pro`) are assumed from exploration; Task 8 Step 2 instructs verifying/adjusting against the live picker if they differ.
