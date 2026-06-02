# InfraFlow Capability-Driven Packet Glyphs — Design

**Status:** approved
**Date:** 2026-06-01
**Scope:** apps/landing (WebGPU diagram) + one honesty fix in litegen-core

---

## 1. Problem / goal

The "How it works" diagram (`apps/landing/src/components/InfraFlow.tsx` + `infra-flow/*`)
is a radial hub-and-spoke web: the app sends a prompt into the central LiteGen
gateway, which fans out to 18 provider nodes. Today every spoke animates the
same generic traffic — dull outbound "request dots" and bright inbound "media
tiles" tinted only by image-vs-video.

That undersells the product. Different providers accept genuinely different
**input shapes** over the wire: some take only a text prompt, some take a prompt
plus one reference image, some take several reference images, some take video
keyframes or an inpaint mask. We want the diagram to *show* that variety: each
spoke should stream outbound packets whose glyph is randomly drawn, per
emission, from the set of input shapes **that provider actually accepts**, and
inbound result tiles rendered as that provider's output modality (still image
vs. video filmstrip).

The per-provider capability data is **dogfooded live** from the gateway's
`GET /v1/models` endpoint — the same registry data the API serves to real
clients — not hand-maintained in the front end.

### Decisions locked during brainstorming

- **Data source:** live fetch of `GET /v1/models` (not codegen-from-yaml, not a
  hand-baked table).
- **Glyph set:** "Core 3 + filmstrip" — outbound `text`, `+image`, `multi-image`;
  inbound `still` (image) vs `filmstrip` (video). Mask/inpaint and first+last
  frames fold into the nearest core glyph rather than getting dedicated shapes.
- **Legibility:** purely visual — no legend. Glyph shapes carry the meaning.

### Non-goals

- No legend / tooltip / on-hover capability list (purely visual, per decision).
- No dedicated glyphs for mask, first/last frame, style, character, reference
  roles — they collapse into `image` (single) or `multi` (≥2).
- No change to node positions, hover/focus behaviour, the scan beam, halos,
  cursor lensing, or the SVG fallback.
- No new runtime dependency in the landing app and no requirement that a gateway
  be reachable at build time (see §3 fallback).

---

## 2. Architecture

```
  gateway (running)                      apps/landing (build time)
  ┌───────────────┐   GET /v1/models     ┌─────────────────────────────┐
  │ /v1/models    │ ───────────────────► │ scripts/sync-capabilities.mjs│
  │ (registry)    │   ModelInfo[]        │  reduce → per-provider vocab  │
  └───────────────┘                      └──────────────┬──────────────┘
                                                         │ writes (committed)
                                                         ▼
                                  infra-flow/capabilities.generated.ts
                                                         │ imported by
                                                         ▼
              nodes.ts ──► InfraFlow.tsx ──► renderer.ts ──► shader.ts
              (attach vocab+output to each provider node; renderer packs a
               per-node vocab bitmask into the uniform; shader draws the
               typed outbound glyphs + still/filmstrip inbound tiles)
```

Single direction of new data: `/v1/models` → generated TS → node metadata →
uniform → shader. Nothing in the render path fetches at runtime; the data is
baked at build.

---

## 3. Data pipeline

### 3.1 Source endpoint

`GET /v1/models` returns `ModelListResponse { object, data: ModelInfo[] }`.
Each `ModelInfo` (see `sdks/typescript/src/generated/schema.d.ts`) carries:

- `provider: string` — e.g. `"stability"`, `"bytedance"`.
- `media_type: "image" | "video"` — output modality.
- `capabilities: ModelCapabilities` with the booleans
  `supports_text_to_image`, `supports_image_to_image`, `supports_inpainting`,
  `supports_text_to_video`, `supports_image_to_video`, `supports_first_frame`,
  `supports_last_frame`, and the integer `max_images`.

### 3.2 Backend honesty fix (litegen-core)

`project_model_info` (`litegen-core/src/api/handlers/mod.rs:577`) currently
hardcodes `max_images: 1`. The real cap lives in `ref_inputs.max_total`. Without
this, the slim list cannot distinguish single-image from multi-image providers
and "multi" cannot be dogfooded from `/v1/models`.

Change:

```rust
max_images: s.ref_inputs.as_ref().map(|ri| ri.max_total).unwrap_or(1),
```

(`max_total` is `u32`; cast to the `i32`/`u32` the `ModelCapabilities` field
uses, matching the surrounding code.) An existing `list_models`/handler test is
extended to assert a known multi-image model (e.g. `bytedance/seedream-4-0-…`,
`max_total: 6`) reports `max_images >= 2`, and a single-ref model reports `1`.

### 3.3 Sync script

`apps/landing/scripts/sync-capabilities.mjs` (Node ESM, no deps — uses global
`fetch`):

1. `BASE = process.env.LITEGEN_API_URL ?? 'http://localhost:4000'`.
2. `GET ${BASE}/v1/models`. On any failure (env unreachable, non-200, network
   error, parse error): print a clear `[sync-capabilities] warning: …; keeping
   existing committed snapshot` and **exit 0 without writing**. This is what
   keeps `next build` green offline / in CI.
3. On success: group `data` by `provider`, derive per-provider vocab + output
   (§4), and write `infra-flow/capabilities.generated.ts` with a
   "GENERATED — do not edit; run `npm run sync-capabilities`" header. Stable key
   ordering so re-runs produce minimal diffs.

`package.json` scripts:

```json
"sync-capabilities": "node scripts/sync-capabilities.mjs",
"prebuild": "node scripts/sync-capabilities.mjs"
```

`prebuild` runs automatically before `next build`. Because the script is
fail-open, a build with no gateway simply reuses the committed file.

### 3.4 Generated file shape

```ts
// GENERATED by scripts/sync-capabilities.mjs — do not edit.
export interface ProviderCapability {
  /** registry provider id, e.g. "stability" */
  provider: string;
  /** accepted input packet shapes (Core-3) */
  inputs: { text: boolean; image: boolean; multi: boolean };
  /** headline output modality used for the inbound tile */
  output: 'image' | 'video';
}
export const PROVIDER_CAPABILITIES: Record<string, ProviderCapability>;
```

Keyed by provider id. A committed snapshot generated against a full local
gateway ships in the repo so the diagram is correct even before anyone runs the
script.

---

## 4. Per-provider vocab derivation (Core-3)

Union across all of a provider's `ModelInfo` entries:

| Vocab bit | True when ANY of the provider's models has… |
|---|---|
| `text` | `supports_text_to_image || supports_text_to_video` |
| `image` | accepts at least one reference image — `supports_image_to_image || supports_image_to_video || supports_inpainting || supports_first_frame || supports_last_frame` (i.e. `max_images >= 1`) |
| `multi` | `max_images >= 2` |

The bits are nested by construction: any `multi` provider is also `image`, and
nearly every provider is `text`. This is intentional — a provider that can take
several reference images can obviously also take one, so the picker (§7.1) will
stream single-image *and* multi-image glyphs for it. `multi` is the strict
"this one is special" signal; `image` is "takes a reference at all."

`output` is informational only. The diagram already assigns each provider node a
single headline `kind` (image nodes upper hemisphere, video lower), and that
`kind` stays the source of truth for which inbound tile (still vs filmstrip)
renders — keeping the visual upper/lower split consistent with the layout. The
generated `output` field is emitted for completeness / providers not pinned in
`nodes.ts`, set to `'video'` if the provider has any video model else `'image'`.

A provider with no accepted reference image at all (pure text-to-X, e.g.
Recraft) yields `{ text: true, image: false, multi: false }` and streams only
text glyphs. A provider must always have at least one bit set; if derivation
somehow yields none (data gap), default to `text: true` so a spoke is never
empty.

### 4.1 Sanity examples

The committed snapshot is generated from live data (§3.3) — that is the source
of truth, not this list. Two unambiguous cases to eyeball after a sync:

- **Recraft** — only `text_to_image`, no ref inputs → `{ text, !image, !multi }`;
  streams scribble glyphs only.
- **ByteDance** — Seedream accepts up to 6 init images (`max_images: 6`) →
  `{ text, image, multi }`; streams scribble, single-square, and stacked-square
  glyphs.

Video providers that take first+last keyframes (Kling, Luma, MiniMax, Veo,
Vidu, PixVerse, …) have `max_total == 2`, so they read as `{ text, image, multi }`
under the Core-3 rule — first/last-frame folds into `multi`, as decided.

---

## 5. Node metadata (nodes.ts)

`FlowNode` gains an optional input-vocab field, attached for provider nodes by
looking the provider up in `PROVIDER_CAPABILITIES`:

```ts
export interface InputVocab { text: boolean; image: boolean; multi: boolean; }
// on FlowNode (provider-only, like kind/model):
inputs?: InputVocab;
```

A small helper maps each `PROVIDER_NODES` entry's id → registry provider id
(the diagram ids already align with registry provider ids: `openai`,
`stability`, `bytedance`, …; the few brand-vs-id mismatches — e.g. `bfl`,
`google`, `bedrock`, `hunyuan` — are covered by an explicit id map kept beside
the node list). Nodes whose provider is missing from the generated map fall back
to `{ text: true, image: true, multi: false }` so the diagram degrades sanely.

The vocab is encoded into a 3-bit mask `text=1 | image=2 | multi=4` (0..7) for
the uniform.

---

## 6. Renderer + uniform (renderer.ts, shader.ts)

A new per-node uniform array carries the vocab mask. Following the existing
std140 discipline (see the KEEP-IN-SYNC comment in `renderer.ts`):

- Add `nodeFlags : array<vec4<f32>, 24>` to the WGSL `struct U` after
  `nodeMeta`. Channel layout: `.x = vocabMask (0..7 as f32)`, `.yzw` reserved/0.
- `renderer.ts`: `UNIFORM_FLOATS += MAX_NODES * 4`; add `FLAGS_BASE = META_BASE
  + MAX_NODES * 4`; in the per-node loop write
  `data[FLAGS_BASE + i*4] = vocabMaskFor(node)`. `FlowHandle` gains
  `inputsMask?: number`; `InfraFlow.tsx` passes it through from `FlowNode.inputs`.
- App and gateway nodes get mask `0` (they never emit typed glyphs; the
  app→gateway trunk keeps its current generic look).

The cap (`MAX_NODES = 24`) is unchanged. The three array sizes (`geom`,
`nodeMeta`, `nodeFlags`) stay equal and are bumped together if ever retuned.

---

## 7. Shader glyphs (shader.ts)

### 7.1 Outbound packets (replaces the "dull request dots" in `edge()`)

The `edge()` function takes the provider's `vocabMask` (passed from `fs` as a new
arg). The outbound stream still travels `a → b` (gateway → provider). For each of
a small number of outbound packets `j`:

- **Packet identity / re-roll:** `cycle = floor(mt * spd + j * offset)`. The
  type is chosen by `pickVocab(hash(i, cycle), vocabMask)` — a helper that hashes
  the (spokeIndex, cycle) pair to a value in `[0,1)` and maps it onto the set
  bits of `vocabMask` (so only accepted shapes appear, and each emission re-rolls
  every cycle → "randomly one of the supported types").
- **Phase along the curve:** `ph = fract(mt * spd + j * offset)`; `center =
  bez(a, c, b, ph)`; life envelope as today.
- **Glyph SDF** (all small, drawn near `center`, reuse `sdRoundBox`):
  - `text` → a rounded tile with 2–3 horizontal "scribble" strokes
    (`smoothstep` bands modulated by `sin` for a hand-written wobble).
  - `image` → a rounded square outline + faint fill (a single "picture").
  - `multi` → 2–3 offset small squares (a stack of pictures).
- Outbound glyphs stay comparatively **cool/desaturated** so the vibrant inbound
  media still reads as the brightest thing on the route (preserves the existing
  "plain-input-out, rich-media-back" contrast).

### 7.2 Inbound result tiles (existing tile loop, extended)

Keep the bloom-on-arrival inbound tiles (`b → a`). Differentiate by output kind
(already available via `nodeMeta.y` / `mediaTint`):

- **image (still):** current rounded tile (unchanged).
- **video (filmstrip):** same tile body plus 2–3 vertical sprocket divisions and
  a faint centre play-triangle notch, so video results read as a tiny film
  strip rather than a flat tile.

### 7.3 Invariants preserved

- Early-bail `if (d > 0.09) { return col; }` stays — glyphs only evaluate near
  the trace.
- Reduced-motion: outbound glyphs and inbound tiles **park** at fixed phases
  (no travel), exactly as the current tiles do, so the frozen frame still reads
  as full and varied. `pickVocab` uses a fixed cycle (e.g. 0) under reduced
  motion so the parked glyphs are deterministic.
- App→gateway trunk, scan beam, hub core, halos, vignette/tone unchanged.
- Branch-light: `pickVocab` + glyph selection avoid heavy per-pixel divergence
  (compute the chosen glyph only; mask is a uniform per spoke, not per pixel).

---

## 8. Files touched

| Path | Change |
|---|---|
| `litegen-core/src/api/handlers/mod.rs` | `max_images` ← `ref_inputs.max_total` |
| `litegen-core/src/api/handlers/…tests` | assert multi vs single `max_images` |
| `apps/landing/scripts/sync-capabilities.mjs` | **new** — live fetch → generated TS (fail-open) |
| `apps/landing/package.json` | `sync-capabilities` + `prebuild` scripts |
| `apps/landing/src/components/infra-flow/capabilities.generated.ts` | **new**, committed snapshot |
| `apps/landing/src/components/infra-flow/nodes.ts` | `InputVocab`, attach `inputs`, id map, mask helper |
| `apps/landing/src/components/infra-flow/renderer.ts` | `nodeFlags` uniform array, `inputsMask` in `FlowHandle` |
| `apps/landing/src/components/infra-flow/shader.ts` | `nodeFlags`, `pickVocab`, outbound glyphs, filmstrip tiles |
| `apps/landing/src/components/InfraFlow.tsx` | pass `inputsMask` into `FlowHandle` |
| `apps/landing/.env.example` (+ README note) | document `LITEGEN_API_URL` |

---

## 9. Testing & verification

- **Backend:** `cargo test -p litegen-core` — extended handler test proves
  `max_images` reflects `ref_inputs.max_total` for a multi-image and a
  single-image model.
- **Sync script:** run a local gateway (`:4000`) and `npm run sync-capabilities`;
  confirm `capabilities.generated.ts` is written and the derived vocab matches
  the §4.1 sanity table. Run with `LITEGEN_API_URL` unset/bad and confirm it
  exits 0 and leaves the file untouched.
- **Build resilience:** `next build` with and without a reachable gateway both
  succeed (the without-case uses the committed snapshot).
- **Visual:** open the landing page in a WebGPU browser; confirm (a) text-only
  providers (Recraft) stream only scribble glyphs, (b) multi-image providers
  (ByteDance/Luma/Google) visibly stream stacked-square glyphs, (c) video
  providers' inbound tiles read as filmstrips, (d) reduced-motion shows a static
  frame with a representative parked glyph per spoke, (e) the non-WebGPU SVG
  fallback is unchanged.

---

## 10. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Gateway unreachable at build → broken build | Sync script is fail-open; committed snapshot is the fallback. |
| Generated data drifts from registry | `prebuild` re-syncs whenever a gateway is reachable; snapshot is regenerated and committed in the same PRs that change models. |
| std140 uniform desync from the new array | Bump WGSL `nodeFlags` size and `UNIFORM_FLOATS`/`FLAGS_BASE` together; the existing KEEP-IN-SYNC comment is updated to mention three arrays. |
| Glyph noise at 18 spokes / small scale | Core-3 only; outbound glyphs kept cool/small; early-bail unchanged; visual check is an explicit acceptance step. |
| GPU branch divergence from per-packet type | Vocab mask is uniform per spoke; `pickVocab` picks one glyph to draw, not all three. |
| Provider id vs brand-name mismatch (`bfl`, `google`, …) | Explicit id map beside the node list; missing-provider fallback vocab. |
```
