# Multi-Model Generation Playground — Design

**Date:** 2026-06-09
**Status:** Approved
**Component:** `dashboard/` (app.litegen.ai dashboard, React 19 + Vite + TS)

## Problem

The dashboard ships a single-model, mock-only Playground
([dashboard/src/pages/Playground.tsx](../../../dashboard/src/pages/Playground.tsx)):
pick one model, type a prompt, set a few hardcoded params (negative prompt, seed,
size, n, strict), generate one image. It only lists `mock` image models and only
surfaces `size` from the model schema — every other per-model param (style,
quality, steps, guidance, aspect ratio, …) is invisible.

We want an out-of-the-box section where you can **test-generate across many models
at once**: select several image models, fill in **one unified parameter panel**
built from the union of all selected models' real schemas, and generate so that
each model produces a result tile (one image per generation), comparable
side-by-side.

## Decisions (from brainstorming)

1. **Model scope: all catalog, badged.** Show every image model. Models whose
   provider is configured (`is_available`) run live (cost money, consume quota);
   unconfigured catalog models and mock models are badged. Nothing is hidden.
2. **Placement: evolve the Playground.** Upgrade `/playground` to do both Single
   and Compare. Existing single-model behavior and its `data-testid`s are
   preserved so current e2e tests keep passing.
3. **Unified params: union + applicability tags.** Show every param any selected
   model supports; tag each control with which models it applies to; only send a
   param to models that declare it.
4. **Modality: image only (v1).** Text-to-image, one image tile per generation.
   Architecture leaves room for video later (a tile could host a player + poll).
5. **Mode toggle kept** (Single / Compare) rather than always-multi — Single is a
   strict superset-preserving fallback for the existing tested flow.
6. **`○setup` models are selectable** (run-and-let-it-error) rather than hidden —
   the resulting provider error is surfaced inline on that model's tile.

## Approach: client-side fan-out, no backend changes

Everything needed already exists in the TS SDK (`@litegen/sdk`) and is reachable
from the authenticated dashboard session:

- `client.models.list()` → `ModelInfo[]` (carries `is_available`, `media_type`,
  `provider`, `pricing.base_cost_usd`).
- `client.models.getSchema(id)` → `ModelSchema` with
  `params: Record<string, ParamSpec>`.
- `client.images.estimateCost(req)` → `POST /v1/images/cost` → `CostEstimate`
  (`total_cost_usd`, incl. markup).
- `client.images.generate(req)` → `POST /v1/images/generations`.

On **Generate**, the browser fires **one independent `images.generate()` call per
selected model**, each tailored to that model's own schema. Tiles fill in as
requests resolve.

**Rejected alternative — server-side `/v1/images/compare` batch endpoint:** more
Rust surface to maintain, weaker per-model error isolation, and it would
re-implement validation the single-image endpoint already performs. Client
fan-out gives isolated per-tile loading/error state, progressive results, and zero
backend risk. Not pursued unless a future requirement (e.g. server-side rate
shaping) demands it.

## `ParamSpec` → control mapping

`ParamSpec` is a `kind`-tagged union
([litegen-core/src/capabilities/schema.rs](../../../litegen-core/src/capabilities/schema.rs)):

| `kind` | Fields | Control |
|---|---|---|
| `bool` | `default?` | checkbox |
| `int` | `min? max? default?` | number input clamped to `min`/`max` |
| `float` | `min? max? default?` | number input clamped to `min`/`max` |
| `string` | `max_length? enum_values[] pattern? default?` | `<select>` if `enum_values` non-empty, else text/textarea bounded by `max_length` |
| `size` (mode `enum`) | `values: [w,h][]` | `<select>` of `WxH` strings |
| `size` (mode `freeform`) | `min/max width/height`, `multiple_of?` | width/height number inputs stepping by `multiple_of` |
| `aspect_ratio` | `allowed[] default?` | `<select>` of `allowed` |
| `seed` | `min max` | number input, empty = random |

`size` enum tuples `[w,h]` serialize to the wire as `"WxH"` strings (the existing
Playground already does this conversion).

## Unified parameters: union + applicability

`useUnifiedParams(selectedModelIds)`:

1. Fetches+caches each selected model's `ModelSchema` (dedupe in-flight; cache by
   id for the session).
2. Builds a **merged param map**: keyed by param name. For each name it records
   the set of models that declare it (`applicability`) and a **merged spec** used
   to render one control. Merge rule for shared specs of the same `kind`:
   numeric `min` = max of mins, `max` = min of maxes (most restrictive
   intersection so the single control stays valid for every model that uses it);
   enum/allowed = intersection of option lists; `default` = first model's default.
   If two models declare the same param name with **incompatible kinds**, split
   into per-kind entries tagged by model (rare; defensive).
3. Holds shared form state (prompt is always present; plus every merged param).
4. **Per-model request builder** `buildRequest(modelId, formState)`: start from
   shared values, then for that model **drop any param it does not declare** and
   clamp remaining values to that model's own bounds. Result is a schema-valid
   `ImageGenerationRequest` body (`model`, `prompt`, `n`, `strict`, plus declared
   params). `strict` defaults on.

Applicability is surfaced in the UI as a small tag per control (`all` when every
selected model declares it, else the short model names).

## Components & files

New, under `dashboard/src/`:

- `pages/Playground.tsx` — refactored into a thin shell hosting **Single** and
  **Compare** modes via a toggle. Single mode keeps today's markup + `data-testid`s.
- `playground/ModelPicker.tsx` — searchable multi-select checklist of image
  models with availability badges (`●live` / `○setup` / `◇mock`), sorted
  live → mock → setup.
- `playground/ParamField.tsx` — schema-driven control renderer switching on
  `ParamSpec.kind` (table above). Pure/presentational; value + onChange in.
- `playground/useUnifiedParams.ts` — schema fetch+cache, union/applicability
  merge, shared form state, per-model `buildRequest`.
- `playground/useFanOut.ts` — runs N `images.generate()` calls with bounded
  concurrency (~4), per-tile state machine (`queued|running|done|error`), and
  `AbortController` cancel.
- `playground/ResultGrid.tsx` + `ResultTile.tsx` — one tile per generation
  (model × n). Tile shows image (b64 or url), model id, cost, latency, seed,
  error badge, enlarge, and rerun-just-this-one.

Changed:

- `App.css` — new `.pg-*` classes for picker, unified params, badges, result grid.
- `playground-history.ts` — extend the history entry so an entry can be a
  multi-model run (shared params + array of per-model `{model, request, response,
  error}`). Single-mode entries remain backward compatible.

## UX / layout

Left **control panel** / right **results** split (same skeleton as today):

- **Mode toggle** Single / Compare at top.
- **Compare → MODELS**: searchable checklist with availability badges + selected
  count.
- **Unified parameters**: prompt textarea, then one `ParamField` per merged param
  with its applicability tag; shared seed and N (1–4) and strict toggle.
- **Cost preview**: debounced sum of `images.estimateCost()` across selected
  models × n, shown above the Generate button (`Est. cost: $X.XX`). Falls back to
  a catalog estimate (`pricing.base_cost_usd × n`) if a cost call fails.
- **Generate** button labelled with the model count; a **Cancel** button while
  in flight.
- **Results grid**: tiles fill progressively; each independent.

Single mode renders the existing layout unchanged.

## Error / edge handling

- **Per-tile isolation**: one model failing (402 quota, 5xx, unconfigured
  provider, validation) shows an error badge on its own tile; others continue.
- **`○setup` models**: selectable; their provider error is surfaced inline, not
  pre-blocked.
- **Concurrency cap** (~4 in flight); the rest queue.
- **Abort**: Cancel aborts in-flight + queued via `AbortSignal`.
- **Quota** (`x-litegen-quota-exceeded` / 402): toast via existing handling.
- **Empty/invalid**: Generate disabled when no model selected or prompt blank.

## Testing

- **Unit (vitest):**
  - `useUnifiedParams` — union, applicability set, restrictive merge
    (min/max intersection, enum intersection), per-model `buildRequest` drops
    non-declared params and clamps bounds.
  - `ParamField` — renders the correct control for each `kind` (incl. both `size`
    modes) and emits expected values.
- **e2e (Playwright, mock provider):**
  - Single mode unchanged (existing tests stay green).
  - Compare mode: select 2+ mock image models, set prompt, generate, assert one
    result tile per selected model each showing an image; assert per-tile error
    isolation using a failing mock.

## Out of scope (v1)

- Video models (architecture leaves room: tile → player + polling later).
- Server-side batch/compare endpoint.
- Saving/sharing comparison permalinks.
