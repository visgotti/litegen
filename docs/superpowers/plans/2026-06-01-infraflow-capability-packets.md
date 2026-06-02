# InfraFlow Capability-Driven Packet Glyphs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make each spoke of the landing-page InfraFlow diagram stream per-provider input-shape glyphs (text / +image / multi-image), chosen at random per emission from what that provider actually accepts, with inbound result tiles drawn as still images or video filmstrips — all dogfooded live from `GET /v1/models`.

**Architecture:** A fail-open Node prebuild script fetches `/v1/models`, reduces it to a per-provider capability map, and writes a committed `capabilities.generated.ts`. `nodes.ts` attaches that vocab to each provider node and exposes a 3-bit mask; `renderer.ts` packs the mask into a new per-node uniform array; `shader.ts` draws the typed outbound glyphs and still/filmstrip inbound tiles. One backend line makes `/v1/models` report real multi-image counts.

**Tech Stack:** Rust (axum, litegen-core), Node ESM build script (`node --test`), TypeScript, Next 15 / React 19, WebGPU + WGSL.

**Conventions for this plan:**
- Node may not be on `PATH`; it lives at `/opt/homebrew/bin`. If `node`/`npm` aren't found, prefix with that path.
- Landing commands use `npm --prefix apps/landing …` to avoid `cd`.
- The landing app has **no TS unit-test runner** and WebGPU shaders can't be meaningfully unit-tested. Real logic (the capability reducer, a `.mjs` module) is covered by `node --test`; `nodes.ts`/`renderer.ts`/`shader.ts` are verified by typecheck (`next build`) plus an explicit visual checklist in Task 9. This is intentional, not a gap.

---

## File structure

| Path | Responsibility | Status |
|---|---|---|
| `litegen-core/src/api/handlers/mod.rs` | `project_model_info` reports `max_images` from `ref_inputs.max_total`; new test mod | modify |
| `apps/landing/scripts/derive-capabilities.mjs` | Pure reducer `deriveCapabilities(models)` + `renderGeneratedTs(caps)` + baked `FALLBACK_CAPABILITIES` | create |
| `apps/landing/scripts/derive-capabilities.test.mjs` | `node --test` unit tests for the reducer | create |
| `apps/landing/scripts/sync-capabilities.mjs` | Fetch `/v1/models` → derive → write; fail-open | create |
| `apps/landing/src/components/infra-flow/capabilities.generated.ts` | Committed per-provider capability snapshot | create (generated) |
| `apps/landing/package.json` | `sync-capabilities`, `test:scripts`, `prebuild` scripts | modify |
| `apps/landing/src/components/infra-flow/nodes.ts` | `InputVocab` type, attach `inputs`, `inputsMask()` helper | modify |
| `apps/landing/src/components/infra-flow/renderer.ts` | `nodeFlags` uniform array; `inputsMask` on `FlowHandle` | modify |
| `apps/landing/src/components/InfraFlow.tsx` | Pass `inputsMask` into each `FlowHandle` | modify |
| `apps/landing/src/components/infra-flow/shader.ts` | `nodeFlags` in `struct U`; `pickVocab`/`glyph` fns; typed outbound glyphs; filmstrip tiles | modify |
| `apps/landing/.env.example` | Document `LITEGEN_API_URL` | create |

---

## Task 1: Backend — `/v1/models` reports real `max_images`

**Files:**
- Modify: `litegen-core/src/api/handlers/mod.rs:597`
- Test: `litegen-core/src/api/handlers/mod.rs` (new `#[cfg(test)] mod model_info_tests` at end of file)

- [ ] **Step 1: Write the failing test**

Append at the end of `litegen-core/src/api/handlers/mod.rs`:

```rust
#[cfg(test)]
mod model_info_tests {
    use super::*;
    use crate::capabilities::CapabilityRegistry;

    fn shipped_registry() -> CapabilityRegistry {
        // <repo>/models, mirroring build_test_state() in key_endpoint_tests.
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        CapabilityRegistry::from_dir(&p).expect("load shipped models")
    }

    #[test]
    fn max_images_reflects_ref_inputs_max_total() {
        let reg = shipped_registry();

        // Multi-image model: ByteDance Seedream accepts up to 6 init images.
        let seedream = reg
            .get("bytedance/seedream-4-0-250828")
            .expect("seedream present");
        assert!(
            project_model_info(seedream).capabilities.max_images >= 2,
            "multi-image model must report max_images >= 2"
        );

        // Single-ref model: Stability SD3-Large caps at one init image.
        let sd3 = reg.get("stability/sd3-large").expect("sd3-large present");
        assert_eq!(project_model_info(sd3).capabilities.max_images, 1);

        // No-ref model: Recraft v3 has no ref_inputs → defaults to 1.
        let recraft = reg.get("recraft/recraftv3").expect("recraftv3 present");
        assert_eq!(project_model_info(recraft).capabilities.max_images, 1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p litegen-core model_info_tests`
Expected: FAIL — `max_images_reflects_ref_inputs_max_total` panics on the first assert (`max_images` is hardcoded to `1`, so `1 >= 2` is false).

- [ ] **Step 3: Implement the fix**

In `litegen-core/src/api/handlers/mod.rs`, inside `project_model_info`, replace the line:

```rust
            max_images: 1,
```

with:

```rust
            max_images: s.ref_inputs.as_ref().map(|ri| ri.max_total).unwrap_or(1),
```

(`RefInputSpec.max_total` is `u32`; `ModelCapabilities.max_images` is `u32` — no cast needed.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p litegen-core model_info_tests`
Expected: PASS.

- [ ] **Step 5: Guard against regressions in the rest of the suite**

Run: `cargo test -p litegen-core`
Expected: PASS (no other test asserted the old hardcoded `1`).

- [ ] **Step 6: Commit**

```bash
git add litegen-core/src/api/handlers/mod.rs
git commit -m "fix(litegen-core): /v1/models max_images reflects ref_inputs.max_total"
```

---

## Task 2: Capability reducer + baked fallback (pure logic, TDD)

**Files:**
- Create: `apps/landing/scripts/derive-capabilities.mjs`
- Test: `apps/landing/scripts/derive-capabilities.test.mjs`

- [ ] **Step 1: Write the failing test**

Create `apps/landing/scripts/derive-capabilities.test.mjs`:

```js
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { deriveCapabilities, FALLBACK_CAPABILITIES } from './derive-capabilities.mjs';

test('text-only model yields text vocab, image output', () => {
  const caps = deriveCapabilities([
    { provider: 'recraft', media_type: 'image', capabilities: { supports_text_to_image: true, supports_image_to_image: false, max_images: 1 } },
  ]);
  assert.deepEqual(caps.recraft.inputs, { text: true, image: false, multi: false });
  assert.equal(caps.recraft.output, 'image');
});

test('single-ref model yields text+image', () => {
  const caps = deriveCapabilities([
    { provider: 'bfl', media_type: 'image', capabilities: { supports_text_to_image: true, supports_image_to_image: true, max_images: 1 } },
  ]);
  assert.deepEqual(caps.bfl.inputs, { text: true, image: true, multi: false });
});

test('multi-image model sets multi and implies image', () => {
  const caps = deriveCapabilities([
    { provider: 'bytedance', media_type: 'image', capabilities: { supports_text_to_image: true, supports_image_to_image: true, max_images: 6 } },
  ]);
  assert.deepEqual(caps.bytedance.inputs, { text: true, image: true, multi: true });
});

test('union is taken across a provider\'s models; any video model -> video output', () => {
  const caps = deriveCapabilities([
    { provider: 'luma', media_type: 'image', capabilities: { supports_text_to_image: true, supports_image_to_image: true, max_images: 4 } },
    { provider: 'luma', media_type: 'video', capabilities: { supports_text_to_video: true, supports_image_to_video: true, supports_first_frame: true, supports_last_frame: true, max_images: 2 } },
  ]);
  assert.deepEqual(caps.luma.inputs, { text: true, image: true, multi: true });
  assert.equal(caps.luma.output, 'video');
});

test('first_frame counts as an accepted reference image', () => {
  const caps = deriveCapabilities([
    { provider: 'leonardo', media_type: 'video', capabilities: { supports_image_to_video: true, supports_first_frame: true, max_images: 1 } },
  ]);
  assert.equal(caps.leonardo.inputs.image, true);
});

test('a vocab is never empty (defaults to text)', () => {
  const caps = deriveCapabilities([
    { provider: 'weird', media_type: 'image', capabilities: { max_images: 1 } },
  ]);
  assert.equal(caps.weird.inputs.text, true);
});

test('baked fallback covers all 18 diagram providers', () => {
  const ids = ['openai','stability','replicate','bfl','ideogram','recraft','leonardo','google','fal','runway','luma','kling','minimax','bytedance','bedrock','hunyuan','vidu','pixverse'];
  for (const id of ids) {
    assert.ok(FALLBACK_CAPABILITIES[id], `fallback missing ${id}`);
    const v = FALLBACK_CAPABILITIES[id].inputs;
    assert.ok(v.text || v.image || v.multi, `${id} has empty vocab`);
  }
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `node --test apps/landing/scripts/derive-capabilities.test.mjs`
Expected: FAIL — module `./derive-capabilities.mjs` does not exist.

- [ ] **Step 3: Write the reducer + fallback + renderer**

Create `apps/landing/scripts/derive-capabilities.mjs`:

```js
/**
 * Pure helpers for the InfraFlow capability sync.
 *
 * deriveCapabilities() reduces a /v1/models ModelInfo[] into the per-provider
 * input-packet vocabulary the diagram streams (Core-3: text / image / multi)
 * plus the headline output modality. FALLBACK_CAPABILITIES is the baked snapshot
 * used when no gateway is reachable, and is the single source for the committed
 * capabilities.generated.ts (renderGeneratedTs renders it).
 */

/** @typedef {{ text: boolean, image: boolean, multi: boolean }} InputVocab */
/** @typedef {{ provider: string, inputs: InputVocab, output: 'image'|'video' }} ProviderCapability */

/** Reduce a ModelInfo[] (from GET /v1/models) to a provider→capability map. */
export function deriveCapabilities(models) {
  /** @type {Map<string, {text:boolean,image:boolean,multi:boolean,hasVideo:boolean}>} */
  const acc = new Map();
  for (const m of models) {
    const cap = m.capabilities ?? {};
    const acceptsRef = Boolean(
      cap.supports_image_to_image ||
      cap.supports_image_to_video ||
      cap.supports_inpainting ||
      cap.supports_first_frame ||
      cap.supports_last_frame
    );
    const maxImages = Number(cap.max_images ?? 1);
    const v = acc.get(m.provider) ?? { text: false, image: false, multi: false, hasVideo: false };
    if (cap.supports_text_to_image || cap.supports_text_to_video) v.text = true;
    if (acceptsRef) v.image = true;
    if (maxImages >= 2) { v.multi = true; v.image = true; }
    if (m.media_type === 'video') v.hasVideo = true;
    acc.set(m.provider, v);
  }
  /** @type {Record<string, ProviderCapability>} */
  const out = {};
  for (const [provider, v] of acc) {
    const text = v.text || (!v.image && !v.multi); // a spoke is never empty
    out[provider] = { provider, inputs: { text, image: v.image, multi: v.multi }, output: v.hasVideo ? 'video' : 'image' };
  }
  return out;
}

/** Render a provider→capability map to the committed TS module (stable order). */
export function renderGeneratedTs(caps) {
  const ids = Object.keys(caps).sort();
  const entries = ids.map((id) => {
    const c = caps[id];
    const i = c.inputs;
    return `  ${JSON.stringify(id)}: { provider: ${JSON.stringify(c.provider)}, inputs: { text: ${i.text}, image: ${i.image}, multi: ${i.multi} }, output: ${JSON.stringify(c.output)} },`;
  });
  return `// GENERATED by scripts/sync-capabilities.mjs — do not edit by hand.
// Run \`npm run sync-capabilities\` (optionally with LITEGEN_API_URL) to refresh.
export interface ProviderCapability {
  /** registry provider id, e.g. "stability" */
  provider: string;
  /** accepted input packet shapes (Core-3) */
  inputs: { text: boolean; image: boolean; multi: boolean };
  /** headline output modality */
  output: 'image' | 'video';
}

export const PROVIDER_CAPABILITIES: Record<string, ProviderCapability> = {
${entries.join('\n')}
};
`;
}

/**
 * Baked snapshot derived from the shipped models/*.yaml registry. Used only when
 * no gateway is reachable AND no committed snapshot exists yet. Keep in sync with
 * the registry if you change it offline; the live sync overrides this anyway.
 * Vocab rule: text = any t2i|t2v; image = accepts any ref; multi = max_images>=2.
 */
export const FALLBACK_CAPABILITIES = {
  // image hemisphere
  openai:    { provider: 'openai',    inputs: { text: true,  image: true,  multi: true  }, output: 'image' },
  stability: { provider: 'stability', inputs: { text: true,  image: true,  multi: true  }, output: 'image' },
  replicate: { provider: 'replicate', inputs: { text: true,  image: true,  multi: false }, output: 'image' },
  bfl:       { provider: 'bfl',       inputs: { text: true,  image: true,  multi: false }, output: 'image' },
  ideogram:  { provider: 'ideogram',  inputs: { text: true,  image: true,  multi: true  }, output: 'image' },
  recraft:   { provider: 'recraft',   inputs: { text: true,  image: false, multi: false }, output: 'image' },
  leonardo:  { provider: 'leonardo',  inputs: { text: true,  image: true,  multi: false }, output: 'image' },
  google:    { provider: 'google',    inputs: { text: true,  image: true,  multi: true  }, output: 'image' },
  fal:       { provider: 'fal',       inputs: { text: true,  image: true,  multi: false }, output: 'image' },
  // video hemisphere
  runway:    { provider: 'runway',    inputs: { text: true,  image: true,  multi: true  }, output: 'video' },
  luma:      { provider: 'luma',      inputs: { text: true,  image: true,  multi: true  }, output: 'video' },
  kling:     { provider: 'kling',     inputs: { text: true,  image: true,  multi: true  }, output: 'video' },
  minimax:   { provider: 'minimax',   inputs: { text: true,  image: true,  multi: true  }, output: 'video' },
  bytedance: { provider: 'bytedance', inputs: { text: true,  image: true,  multi: true  }, output: 'video' },
  bedrock:   { provider: 'bedrock',   inputs: { text: true,  image: true,  multi: false }, output: 'video' },
  hunyuan:   { provider: 'hunyuan',   inputs: { text: true,  image: true,  multi: false }, output: 'video' },
  vidu:      { provider: 'vidu',      inputs: { text: true,  image: true,  multi: true  }, output: 'video' },
  pixverse:  { provider: 'pixverse',  inputs: { text: true,  image: true,  multi: true  }, output: 'video' },
};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `node --test apps/landing/scripts/derive-capabilities.test.mjs`
Expected: PASS (all 7 tests).

- [ ] **Step 5: Commit**

```bash
git add apps/landing/scripts/derive-capabilities.mjs apps/landing/scripts/derive-capabilities.test.mjs
git commit -m "feat(landing): capability reducer for infra-flow packet vocab"
```

---

## Task 3: Sync script + generated snapshot + npm wiring

**Files:**
- Create: `apps/landing/scripts/sync-capabilities.mjs`
- Create (generated): `apps/landing/src/components/infra-flow/capabilities.generated.ts`
- Modify: `apps/landing/package.json`

- [ ] **Step 1: Write the sync script**

Create `apps/landing/scripts/sync-capabilities.mjs`:

```js
/**
 * Build-time sync: fetch GET /v1/models from a running gateway, derive the
 * per-provider packet vocabulary, and write capabilities.generated.ts.
 *
 * Fail-open: if the gateway is unreachable / returns nothing, keep the existing
 * committed snapshot (so `next build` never breaks offline). If no snapshot
 * exists yet, write the baked FALLBACK so imports always resolve.
 */
import { writeFileSync, existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { deriveCapabilities, renderGeneratedTs, FALLBACK_CAPABILITIES } from './derive-capabilities.mjs';

const BASE = process.env.LITEGEN_API_URL ?? 'http://localhost:4000';
const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, '..', 'src', 'components', 'infra-flow', 'capabilities.generated.ts');

try {
  const res = await fetch(`${BASE}/v1/models`);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const json = await res.json();
  const caps = deriveCapabilities(json.data ?? []);
  if (Object.keys(caps).length === 0) throw new Error('no models returned');
  writeFileSync(OUT, renderGeneratedTs(caps));
  console.log(`[sync-capabilities] wrote ${Object.keys(caps).length} providers from ${BASE}/v1/models`);
} catch (err) {
  const msg = err instanceof Error ? err.message : String(err);
  if (existsSync(OUT)) {
    console.warn(`[sync-capabilities] warning: ${msg}; keeping existing committed snapshot`);
    process.exit(0);
  }
  console.warn(`[sync-capabilities] warning: ${msg}; no snapshot exists, writing baked fallback`);
  writeFileSync(OUT, renderGeneratedTs(FALLBACK_CAPABILITIES));
}
```

- [ ] **Step 2: Generate the initial committed snapshot (from fallback)**

With no gateway running (so the fallback path executes), run:

Run: `node apps/landing/scripts/sync-capabilities.mjs`
Expected: prints `… no snapshot exists, writing baked fallback` and creates `apps/landing/src/components/infra-flow/capabilities.generated.ts`.

- [ ] **Step 3: Verify the generated file content**

Run: `node --test apps/landing/scripts/derive-capabilities.test.mjs` (still green) and open `apps/landing/src/components/infra-flow/capabilities.generated.ts`.
Expected: exports `PROVIDER_CAPABILITIES` with 18 providers; `recraft` is `{ text:true, image:false, multi:false }`; `bytedance` is `{ text:true, image:true, multi:true }`.

- [ ] **Step 4: Add npm scripts**

In `apps/landing/package.json`, replace the `"scripts"` block with:

```json
  "scripts": {
    "dev": "next dev -p 8019",
    "build": "next build",
    "start": "next start -p 8019",
    "lint": "next lint",
    "sync-capabilities": "node scripts/sync-capabilities.mjs",
    "test:scripts": "node --test scripts/",
    "prebuild": "node scripts/sync-capabilities.mjs"
  },
```

- [ ] **Step 5: Verify npm wiring**

Run: `npm --prefix apps/landing run test:scripts`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/landing/scripts/sync-capabilities.mjs apps/landing/src/components/infra-flow/capabilities.generated.ts apps/landing/package.json
git commit -m "feat(landing): sync /v1/models capability snapshot at build (fail-open)"
```

---

## Task 4: nodes.ts — attach vocab + expose mask

**Files:**
- Modify: `apps/landing/src/components/infra-flow/nodes.ts`

- [ ] **Step 1: Add the InputVocab type and the field on FlowNode**

In `apps/landing/src/components/infra-flow/nodes.ts`, add the import near the top (after the file's doc comment, before `export type NodeRole`):

```ts
import { PROVIDER_CAPABILITIES } from './capabilities.generated';
```

Add the interface just below the `MediaKind` type:

```ts
/** Which input packet shapes a provider accepts (drives the streamed glyphs). */
export interface InputVocab {
  text: boolean;
  image: boolean;
  multi: boolean;
}
```

In the `FlowNode` interface, add to the provider-only metadata block (next to `kind`/`model`):

```ts
  /** Accepted input packet shapes (provider-only; attached from the registry). */
  inputs?: InputVocab;
```

- [ ] **Step 2: Attach vocab to provider nodes**

In `nodes.ts`, rename the existing `export const PROVIDER_NODES: FlowNode[] = [ … ];` literal to a private `const PROVIDER_NODES_RAW: FlowNode[] = [ … ];` (change only the declaration line; leave the array contents unchanged), then immediately after the array's closing `];` add:

```ts
/** Safe default for any provider missing from the generated snapshot. */
function vocabFor(id: string): InputVocab {
  return PROVIDER_CAPABILITIES[id]?.inputs ?? { text: true, image: true, multi: false };
}

/** Provider nodes with their accepted input vocabulary attached from the registry. */
export const PROVIDER_NODES: FlowNode[] = PROVIDER_NODES_RAW.map((n) => ({
  ...n,
  inputs: vocabFor(n.id),
}));
```

- [ ] **Step 3: Add the mask helper**

At the end of `nodes.ts` (after `GATEWAY_PILLS`/`GatewayPill`), add:

```ts
/**
 * Pack a node's input vocabulary into the 3-bit mask the shader reads:
 * text = 1, image = 2, multi = 4. Returns 0 for nodes without a vocab
 * (app / gateway), which the shader treats as "text only".
 */
export function inputsMask(node: FlowNode): number {
  const v = node.inputs;
  if (!v) return 0;
  return (v.text ? 1 : 0) | (v.image ? 2 : 0) | (v.multi ? 4 : 0);
}
```

- [ ] **Step 4: Typecheck**

Run: `npm --prefix apps/landing run build`
Expected: compiles (Next type-checks). If the gateway isn't running, `prebuild` prints the fail-open warning and the build proceeds using the committed snapshot. A successful build is the pass condition; you may interrupt after "Compiled successfully" / type-check completes.

- [ ] **Step 5: Commit**

```bash
git add apps/landing/src/components/infra-flow/nodes.ts
git commit -m "feat(landing): attach per-provider input vocab + inputsMask to flow nodes"
```

---

## Task 5: renderer.ts — pack the vocab mask into the uniform

**Files:**
- Modify: `apps/landing/src/components/infra-flow/renderer.ts`

- [ ] **Step 1: Extend FlowHandle**

In `renderer.ts`, in the `FlowHandle` interface, add after `kind?:`:

```ts
  /** Input-vocab bitmask (text=1|image=2|multi=4); 0 for app/gateway. */
  inputsMask?: number;
```

- [ ] **Step 2: Update the uniform layout constants**

Replace the constants block:

```ts
const MAX_NODES = 24;
const UNIFORM_FLOATS = 12 + MAX_NODES * 4 + MAX_NODES * 4; // res+ctl+mouse + geom[N] + meta[N]
const GEOM_BASE = 12;
const META_BASE = 12 + MAX_NODES * 4;
```

with:

```ts
const MAX_NODES = 24;
// res+ctl+mouse(12) + geom[N] + meta[N] + flags[N]. KEEP IN SYNC with the three
// array<vec4<f32>, N> sizes in shader.ts — bump all of them together or the
// std140 layout desyncs and the uniform reads garbage.
const UNIFORM_FLOATS = 12 + MAX_NODES * 4 * 3;
const GEOM_BASE = 12;
const META_BASE = 12 + MAX_NODES * 4;
const FLAGS_BASE = 12 + MAX_NODES * 4 * 2;
```

- [ ] **Step 3: Write the mask each frame**

In `renderFrame`, inside the `for (let i = 0; i < nodeList.length; i++)` loop, after the `const m = META_BASE + i * 4; … data[m + 3] = …;` block, add:

```ts
      // Per-node input-vocab bitmask (app/gateway → 0 → "text only" in shader).
      data[FLAGS_BASE + i * 4] = node.inputsMask ?? 0;
```

- [ ] **Step 4: Typecheck**

Run: `npm --prefix apps/landing run build`
Expected: compiles. (Shader still declares only `geom`/`nodeMeta`; the larger uniform buffer is harmless until Task 7 adds `nodeFlags`. Do not ship between Task 5 and Task 7 — they land together.)

- [ ] **Step 5: Commit**

```bash
git add apps/landing/src/components/infra-flow/renderer.ts
git commit -m "feat(landing): pack per-node input-vocab mask into the flow uniform"
```

---

## Task 6: InfraFlow.tsx — pass the mask through

**Files:**
- Modify: `apps/landing/src/components/InfraFlow.tsx`

- [ ] **Step 1: Import the helper**

In the import from `'./infra-flow/nodes'`, add `inputsMask` to the named imports:

```ts
import {
  ALL_NODES,
  APP_NODE,
  GATEWAY_NODE,
  GATEWAY_PILLS,
  PROVIDER_NODES,
  inputsMask,
  type FlowNode,
} from './infra-flow/nodes';
```

- [ ] **Step 2: Populate inputsMask on each handle**

In the handle-assembly loop, change the `handles.push({ … })` call to include the mask:

```ts
      handles.push({
        id: node.id,
        el: measureEl,
        role: node.role,
        kind: node.kind,
        inputsMask: inputsMask(node),
      });
```

- [ ] **Step 3: Typecheck**

Run: `npm --prefix apps/landing run build`
Expected: compiles.

- [ ] **Step 4: Commit**

```bash
git add apps/landing/src/components/InfraFlow.tsx
git commit -m "feat(landing): pass input-vocab mask from nodes into the renderer"
```

---

## Task 7: shader.ts — typed outbound glyphs + filmstrip tiles

**Files:**
- Modify: `apps/landing/src/components/infra-flow/shader.ts`

> Shader constants (sizes, speeds, tints) below are working starting values; expect to fine-tune them during the Task 9 visual check. The structure and signatures are fixed.

- [ ] **Step 1: Add `nodeFlags` to the uniform struct**

In `shader.ts`, in `struct U`, add a line after `nodeMeta`:

```wgsl
  nodeMeta : array<vec4<f32>, 24>,   // 'meta' is a reserved word in WGSL
  nodeFlags : array<vec4<f32>, 24>,  // (.x = input-vocab bitmask: text1|image2|multi4)
```

Update the uniform-layout doc comment block above `FLOW_WGSL` to list the third array:

```
 *   nodeFlags : vec4[24] = per node (.x input-vocab bitmask text1|image2|multi4, .yzw pad)
```

- [ ] **Step 2: Add the packet-picker and glyph SDF helpers**

In `shader.ts`, immediately after the `bez(...)` function (before `curveDist`), add:

```wgsl
// Cheap 1D hash for per-packet identity (which glyph this emission shows).
fn hash1(n : f32) -> f32 {
  return fract(sin(n * 12.9898) * 43758.5453123);
}

// Choose one accepted input type for a packet. mask bits: text=1, image=2,
// multi=4. r in [0,1). Returns 0=text, 1=image, 2=multi. Falls back to text
// when the mask is empty (app/gateway trunk).
fn pickVocab(mask : i32, r : f32) -> i32 {
  var opts = array<i32, 3>(0, 0, 0);
  var n = 0;
  if ((mask & 1) != 0) { opts[n] = 0; n = n + 1; }
  if ((mask & 2) != 0) { opts[n] = 1; n = n + 1; }
  if ((mask & 4) != 0) { opts[n] = 2; n = n + 1; }
  if (n == 0) { return 0; }
  let idx = clamp(i32(floor(r * f32(n))), 0, n - 1);
  return opts[idx];
}

// An outbound input "packet" glyph centered at q=0. kind: 0 text (tile with
// scribble strokes), 1 single image (framed square), 2 multi-image (offset
// squares). Cool/desaturated so the inbound media still dominates the route.
fn glyph(q : vec2<f32>, kind : i32, boost : f32) -> vec3<f32> {
  let cool = vec3<f32>(0.55, 0.62, 0.95);
  let amp = 0.45 + 0.55 * boost;
  if (kind == 0) {
    let sd = sdRoundBox(q, vec2<f32>(0.013, 0.010), 0.004);
    let frame = smoothstep(0.0022, 0.0, abs(sd)) * 0.6;
    var lines = 0.0;
    for (var k = 0; k < 2; k = k + 1) {
      let yy = -0.004 + f32(k) * 0.008;
      let wob = 0.0016 * sin(q.x * 420.0);
      lines = lines + smoothstep(0.0014, 0.0, abs(q.y - yy - wob)) * step(abs(q.x), 0.009);
    }
    return cool * (frame + lines * 0.5) * amp;
  }
  if (kind == 1) {
    let sd = sdRoundBox(q, vec2<f32>(0.011, 0.011), 0.003);
    let body = smoothstep(0.001, 0.0, sd) * 0.32;
    let frame = smoothstep(0.0022, 0.0, abs(sd)) * 0.72;
    return cool * (body + frame) * amp;
  }
  // kind == 2: two offset squares
  var acc = 0.0;
  for (var k = 0; k < 2; k = k + 1) {
    let off = vec2<f32>(-0.004 + f32(k) * 0.008, 0.004 - f32(k) * 0.008);
    let sd = sdRoundBox(q - off, vec2<f32>(0.0075, 0.0075), 0.0025);
    acc = acc + smoothstep(0.0019, 0.0, abs(sd)) * 0.7 + smoothstep(0.0009, 0.0, sd) * 0.22;
  }
  return cool * acc * amp;
}
```

- [ ] **Step 3: Extend `edge()` signature and replace the outbound dots**

In `shader.ts`, change the `edge()` signature from:

```wgsl
fn edge(p : vec2<f32>, a : vec2<f32>, b : vec2<f32>,
        mt : f32, boost : f32, mediaTint : vec3<f32>, rm : f32, bowScale : f32) -> vec3<f32> {
```

to (adds `vocab`, `spokeId`, `outKind`):

```wgsl
fn edge(p : vec2<f32>, a : vec2<f32>, b : vec2<f32>,
        mt : f32, boost : f32, mediaTint : vec3<f32>, rm : f32, bowScale : f32,
        vocab : i32, spokeId : f32, outKind : f32) -> vec3<f32> {
```

Then replace this whole block (the outbound request dots **and** the single prompt pulse):

```wgsl
  // outbound request dots — the dull, "plain text" prompt heading out (a->b),
  // deliberately desaturated so the vibrant media coming back pops by contrast.
  let dull = vec3<f32>(0.42, 0.44, 0.58);
  let spd = 0.18 + 0.22 * boost;
  for (var i = 0; i < 2; i = i + 1) {
    let ph = fract(mt * spd + f32(i) * 0.5);
    let center = bez(a, c, b, ph);
    let g = smoothstep(0.05, 0.0, abs(along - ph)) * smoothstep(0.006, 0.0, length(p - center));
    col = col + dull * g * 0.40 * (1.0 - rm);
  }
  // a single brighter "prompt" pulse leading each request, sweeping a -> b
  let pph = fract(mt * (0.12 + 0.10 * boost));
  let pcenter = bez(a, c, b, pph);
  let pg = smoothstep(0.006, 0.0, length(p - pcenter));
  col = col + vec3<f32>(0.52, 0.54, 0.70) * pg * (0.5 + 0.6 * boost) * (1.0 - rm);
```

with:

```wgsl
  // outbound input packets — each a small cool glyph whose shape is one of the
  // provider's accepted input types (text scribble / single image / multi
  // image), re-rolled each cycle so the stream shows the provider's real input
  // vocabulary. Parked at mid-spoke under reduced motion so the static frame
  // still reads as varied.
  let ospd = 0.16 + 0.16 * boost;
  let ocount : i32 = select(2, 1, rm > 0.5);
  for (var i = 0; i < ocount; i = i + 1) {
    let raw = mt * ospd + f32(i) * 0.5;
    let ph = select(fract(raw), 0.46 + f32(i) * 0.12, rm > 0.5);
    let cyc = select(floor(raw), 0.0, rm > 0.5);
    let kindSel = pickVocab(vocab, hash1(spokeId * 7.13 + cyc * 3.7 + f32(i) * 1.9));
    let center = bez(a, c, b, ph);
    let life = smoothstep(0.0, 0.10, ph) * smoothstep(0.0, 0.18, 1.0 - ph);
    col = col + glyph(p - center, kindSel, boost) * life;
  }
```

Then, because `along` (`let along = info.y;`, declared just above the early-bail) was referenced **only** by the removed outbound dots, delete that line too — otherwise it's an unused binding. Leave `let d = info.x;` and the `if (d > 0.09) { return col; }` early-bail exactly as they are (the glyph loop stays *after* the bail, so glyphs are only evaluated near the trace — invariant preserved).

- [ ] **Step 4: Make inbound tiles render as a filmstrip for video**

In `edge()`, inside the inbound media-tile loop, find:

```wgsl
    let sd = sdRoundBox(q, vec2<f32>(hsz, hsz), hsz * 0.3);
    let body = smoothstep(0.002, 0.0, sd);
```

and replace those two lines with:

```wgsl
    let sd = sdRoundBox(q, vec2<f32>(hsz, hsz), hsz * 0.3);
    // Video results read as a tiny film strip: carve two vertical sprocket gaps.
    let lx = q.x / hsz;                    // local x in roughly [-1, 1]
    let sprocket = 1.0 - 0.55 * (smoothstep(0.16, 0.0, abs(lx + 0.45)) + smoothstep(0.16, 0.0, abs(lx - 0.45)));
    let strip = select(1.0, sprocket, outKind > 0.5);
    let body = smoothstep(0.002, 0.0, sd) * strip;
```

- [ ] **Step 5: Update the three `edge()` call sites in `fs`**

Call site A — the app→gateway trunk. Change:

```wgsl
  col = col + edge(p, app, gate, time, agBoost, mix(imgTint, vidTint, 0.5), rm, 0.0);
```

to (text-only vocab `1`, spokeId `0`, neutral output `0.0`):

```wgsl
  col = col + edge(p, app, gate, time, agBoost, mix(imgTint, vidTint, 0.5), rm, 0.0, 1, 0.0, 0.0);
```

Call site B — the gateway→provider loop. Change:

```wgsl
    let kind = u.nodeMeta[i].y;
    let tint = mix(imgTint, vidTint, kind);
    let sdir = (pc - gate) / max(length(pc - gate), 1.0e-4);
    let spokeBeam = smoothstep(0.985, 1.0, dot(beamDir, sdir));
    let boost = max(max(foc, gateFoc * 0.8), spokeBeam * 0.5);
    col = col + edge(p, gate, pc, time + f32(i) * 1.3, boost, tint, rm, 0.12);
```

to:

```wgsl
    let kind = u.nodeMeta[i].y;
    let tint = mix(imgTint, vidTint, kind);
    let vocab = i32(u.nodeFlags[i].x);
    let sdir = (pc - gate) / max(length(pc - gate), 1.0e-4);
    let spokeBeam = smoothstep(0.985, 1.0, dot(beamDir, sdir));
    let boost = max(max(foc, gateFoc * 0.8), spokeBeam * 0.5);
    col = col + edge(p, gate, pc, time + f32(i) * 1.3, boost, tint, rm, 0.12, vocab, f32(i), kind);
```

- [ ] **Step 6: Build and smoke-test in a WebGPU browser**

Run: `npm --prefix apps/landing run build`
Expected: compiles with no WGSL/TS errors.

Then run `npm --prefix apps/landing run dev` and open `http://localhost:8019` in a WebGPU-capable browser (Chrome/Edge). Expected: the diagram renders (no all-black canvas, no console WGSL validation error). Detailed visual acceptance is Task 9.

- [ ] **Step 7: Commit**

```bash
git add apps/landing/src/components/infra-flow/shader.ts
git commit -m "feat(landing): stream typed input-packet glyphs + filmstrip result tiles"
```

---

## Task 8: Document `LITEGEN_API_URL`

**Files:**
- Create: `apps/landing/.env.example`

- [ ] **Step 1: Write the env example**

Create `apps/landing/.env.example`:

```bash
# Base URL of a running LiteGen gateway. The `prebuild` step fetches
# `${LITEGEN_API_URL}/v1/models` to refresh the InfraFlow diagram's per-provider
# capability snapshot (src/components/infra-flow/capabilities.generated.ts).
# Unset / unreachable is fine: the build keeps the committed snapshot.
LITEGEN_API_URL=http://localhost:4000

# Canonical public site URL (used for metadata/SEO).
NEXT_PUBLIC_SITE_URL=https://litegen.dev
```

- [ ] **Step 2: Commit**

```bash
git add apps/landing/.env.example
git commit -m "docs(landing): document LITEGEN_API_URL for capability sync"
```

---

## Task 9: Full verification

**Files:** none (verification only)

- [ ] **Step 1: Backend tests**

Run: `cargo test -p litegen-core`
Expected: PASS (incl. `model_info_tests`).

- [ ] **Step 2: Script tests**

Run: `npm --prefix apps/landing run test:scripts`
Expected: PASS.

- [ ] **Step 3: Build resilience — no gateway**

With no gateway on `:4000`, run: `npm --prefix apps/landing run build`
Expected: `prebuild` prints `keeping existing committed snapshot`; build succeeds.

- [ ] **Step 4: Live sync — gateway running**

In one shell, start the gateway (from `litegen-core`, `cargo run`, listening on `:4000`). In another:
Run: `LITEGEN_API_URL=http://localhost:4000 npm --prefix apps/landing run sync-capabilities`
Expected: prints `wrote 18 providers …`; `git diff apps/landing/src/components/infra-flow/capabilities.generated.ts` shows the live-derived data (should match the fallback for the current registry — i.e. likely **no diff**, confirming the dogfood agrees with the baked snapshot). If it differs, the live registry is the source of truth — keep it.

- [ ] **Step 5: Visual acceptance (WebGPU browser)**

`npm --prefix apps/landing run dev`, open `http://localhost:8019`, scroll to "How it works". Confirm:
- Text-only providers (Recraft) stream **only scribble** glyphs outbound.
- Multi-image providers (ByteDance, Luma, Google, Ideogram) visibly stream **stacked-square** glyphs (mixed with single-square and scribble).
- Single-ref-only providers (BFL, Bedrock, Hunyuan, Fal) stream scribble + **single-square**, never stacked.
- Inbound tiles on **video** spokes (lower hemisphere) read as **filmstrips**; **image** spokes (upper) read as solid still tiles.
- Hovering a provider still brightens its route and intensifies its packets.
- `prefers-reduced-motion` (DevTools → Rendering → Emulate CSS prefers-reduced-motion) shows a static frame with a representative parked glyph per spoke.
- Disable WebGPU (or a non-WebGPU browser): the static SVG fallback is unchanged.

- [ ] **Step 6: Final integration commit (if any tuning was applied)**

```bash
git add -A apps/landing/src/components/infra-flow/shader.ts
git commit -m "chore(landing): tune infra-flow packet glyph constants"
```

(Skip if Step 5 needed no tuning.)

---

## Notes for the executor

- **Tasks 5–7 must ship together.** Between Task 5 (larger uniform buffer) and Task 7 (shader reads the new `nodeFlags` array) the std140 layout has more floats than the shader declares — harmless (extra trailing data) but visually inert. Do not deploy a half-applied state.
- **Don't sweep unrelated WIP.** The working tree already contains an in-progress radial-web redesign of `nodes.ts`/`renderer.ts`/`shader.ts`/`InfraFlow.module.css`. Stage only the files named in each task's commit step.
- **Node location:** if `node`/`npm` aren't on `PATH`, they're at `/opt/homebrew/bin`.
```
