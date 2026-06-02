# Unified `/reference` page + per-model capability table

**Date:** 2026-06-02
**Status:** Design — pending review
**Scope:** `apps/landing` only. No changes to the gateway, capability registry, or SDKs.

## Problem

The landing site splits developer docs across two nav entries that feel like the
same thing:

- **"SDKs"** — an in-page anchor (`#sdks`) to the `SdkUsage` section on the
  homepage (TS/Python copy-paste usage).
- **"API"** — a real route (`/api`) embedding the Redoc OpenAPI reference.

Separately, the capability data that already exists end-to-end
(`models/*.yaml` → `CapabilityRegistry` → `GET /v1/models` + `GET /v1/models/{id}`
→ OpenAPI) is **not surfaced as human-browsable per-model documentation**. The
Redoc page documents the *shape* of `ModelSchema`, not the actual per-model data
("dall-e-3 supports these sizes, recraft accepts 1 ref image, kling does
image→video").

## Goal

One developer-docs destination, `/reference`, that holds **everything on one
page**: SDK usage, a browsable per-model/provider capability table, and the full
REST API reference. Collapse the two nav entries into a single **"Reference"**
link.

## Decisions (from brainstorming)

- **One page** at `/[locale]/reference` with three sections. (User: "it should
  all be on 1 page.")
- **Models table** uses **expandable rows**: a compact summary row per model,
  click to expand the full spec.
- **Data source**: generated from `models/*.yaml` **at build** (not a live API
  fetch), because the Cloudflare Pages build has no gateway running and we need
  full per-model data. Refreshes on every deploy; cannot drift from the YAML the
  registry actually loads.

## Architecture

### Routing & IA

- New route: `src/app/[locale]/reference/page.tsx`.
- Three sections, **rendered as tabs** with deep-link hashes: `#sdk`,
  `#models`, `#rest-api`. Tabs (not one continuous scroll) so the heavy Redoc
  CDN bundle only loads when the REST API tab is activated. The active tab syncs
  to the URL hash so `/reference#models` etc. deep-link correctly. *(Still "one
  page" — one route. If a single continuous scroll is preferred over tabs, that
  is the one open UX question; flagged for review.)*
- `src/app/[locale]/api/page.tsx` becomes a redirect stub → `/reference#rest-api`.
  Static export can't do server redirects, so the stub uses
  `<meta http-equiv="refresh" content="0; url=/{locale}/reference#rest-api">` +
  a visible fallback link, plus a `router.replace` in a small client effect.
  Preserves existing `/api` links/bookmarks.
- Nav (`Nav.tsx`): remove the `#sdks` anchor and the `/api` link; add a single
  `<Link href="/reference">{t('reference')}</Link>`. Resulting nav:
  `How it works · Features · Providers · Quickstart · Reference · GitHub`.

### Homepage

- Remove the full `SdkUsage` section from `src/app/[locale]/page.tsx`. Replace
  with a compact teaser/CTA (`SdkTeaser.tsx`): one line + a button
  "SDK usage & API reference →" linking to `/reference#sdk`.
- Keep the `Quickstart` (curl) section on the homepage as a marketing hook.
  *(Open question for review: mirror the curl snippet into `/reference` too, or
  leave it homepage-only.)*

### Components

| Component | Type | Purpose |
|---|---|---|
| `ReferenceTabs.tsx` | client | Tab bar; active tab ↔ URL hash; renders the three panels; lazy-mounts Redoc. |
| SDK panel | — | Reuse existing `SdkUsage` content, rendered inside the SDK tab. |
| `ModelsReference.tsx` | client | Expandable per-model table grouped by provider + a client-side search/filter box. |
| REST API panel | — | Reuse existing `ApiReference.tsx` (Redoc embed), mounted only when its tab is active. |
| `SdkTeaser.tsx` | — | Compact homepage CTA replacing the old SDK section. |

`ModelsReference` row (compact): `id · modalities · output · sizes/ARs · max
refs · price`. Expanded: full `params` (kind, enum/allowed, default, min/max),
ref-image roles + counts, aspect ratios, prompt limits, tags.

### Data pipeline

- New `scripts/sync-models.mjs`:
  - Reads `../../models/*.yaml` (repo root `models/`; the full monorepo is
    checked out on Cloudflare Pages).
  - Adds a `yaml` devDependency for parsing.
  - Emits `src/config/models.generated.ts` exporting `MODELS: ModelEntry[]`
    (and `MODELS_BY_PROVIDER`), with a "generated — do not edit" header.
  - **Fail-open** like `sync-capabilities.mjs`: if `models/` is missing or a
    file fails to parse, log a warning and keep the committed
    `models.generated.ts`. The committed snapshot is the source of truth shipped
    to CF; the script regenerates it on each build when the YAML is present.
  - The pure derive function (`deriveModel(yamlModel)`) is exported for testing.
- Wire into `package.json` `prebuild`:
  `sync-capabilities && sync-openapi && sync-models`.

**`ModelEntry` shape** (serialized to `models.generated.ts`):

```ts
interface ModelEntry {
  id: string;            // "openai/dall-e-3"
  provider: string;      // "openai"
  displayName: string;
  mediaType: 'image' | 'video';
  output: 'image' | 'video';
  capabilities: {        // from YAML `capabilities`
    textToImage: boolean; imageToImage: boolean; inpainting: boolean;
    textToVideo: boolean; imageToVideo: boolean;
  };
  sizes: string[];       // ["1024x1024", ...] from params.size (enum mode)
  aspectRatios: string[];// from params.aspect_ratio.allowed
  maxRefImages: number;  // ref_inputs.max_total ?? 0
  refRoles: { name: string; required: boolean; min: number; max: number }[];
  promptLimits: { required: boolean; minLength?: number; maxLength?: number };
  params: { name: string; kind: string; enum?: string[]; default?: unknown;
            min?: number; max?: number }[];
  pricing?: { baseCostUsd: number };
  tags: string[];
}
```

**Derivation mirrors the Rust projection** (`project_model_info` /
`extract_sizes` in `litegen-core/src/api/handlers/mod.rs`): sizes from the
`size` param when `kind: size, mode: enum`; aspect ratios from
`aspect_ratio.allowed`; `maxRefImages` from `ref_inputs.max_total`. Everything
else (`capabilities`, `prompt`, `params`, `pricing`, `tags`) is read directly
from the YAML. This is a tiny amount of derivation; the YAML is otherwise a
direct mapping.

*Note:* this reads YAML directly, unlike `sync-capabilities.mjs` (which fetches
`/v1/models`). Deliberate — CF's build has no gateway and we need full per-model
data. The diagram's `capabilities.generated.ts` is untouched; unifying both
generators onto YAML is a possible later cleanup, out of scope here.

### i18n

- New `reference.*` keys in `messages/en.json` + `messages/es.json`: tab labels
  (SDK Usage / Models & Capabilities / REST API), table column + section labels
  (Inputs, Output, Sizes, Aspect ratios, Reference images, Params, Price,
  Prompt, Default), search placeholder, expand/collapse aria labels, the teaser
  copy, and the nav `reference` label.
- Model ids, provider names, param names, and param values stay verbatim
  (untranslated).

## Error handling

- Generator: fail-open to the committed snapshot (above). Never breaks the build.
- `ModelsReference`: if `MODELS` is empty (e.g. snapshot somehow blank), render a
  short "Model reference is temporarily unavailable" notice rather than an empty
  table.
- `/api` redirect stub works without JS (meta refresh + link).

## Testing

- `scripts/sync-models.test.mjs` (run by `test:scripts`, `node --test`):
  - `deriveModel` maps a sample YAML model to the expected `ModelEntry`
    (sizes, capability flags, `maxRefImages`, params).
  - Reading the real `models/` dir yields ≥ the expected model count and
    `openai/dall-e-3` derives its three sizes + `textToImage: true`.
- Landing `npm run build` (incl. `prebuild` + static export) must stay green.
- No Rust or SDK changes; their existing suites are unaffected.

## Out of scope

- Gateway, capability registry, OpenAPI generation, and SDK code — unchanged.
- Unifying `capabilities.generated.ts` (diagram) onto the YAML generator.
- Any new capability *fields*; we surface what the registry already specs.

## Open questions for review

1. **Tabs vs. single continuous scroll** on `/reference`. Recommend tabs (Redoc
   lazy-load); easy to switch to scroll if preferred.
2. **Quickstart curl**: keep homepage-only, or also mirror onto `/reference`?
