# 3D vendor notes

These notes condense `docs/superpowers/3d-provider-landscape.md` (researched
2026-09-12) and §9 of `docs/superpowers/specs/2026-08-19-3d-model-generation-design.md`
(researched 2026-08-19). **Treat every row as a lead, not as wire truth.**
Re-read the vendor's live docs before coding, and quote what you used in the
adapter. Items marked *unverified* survived only one research pass.

## Contents

- [Output shapes](#output-shapes)
- [First-party vendors](#first-party-vendors)
- [Aggregators](#aggregators)
- [No public generation API](#no-public-generation-api)
- [Recommended order](#recommended-order)

## Output shapes

Vendors deliver output in four different shapes, and the shape decides what
`output_formats` and `max_items` say:

| Shape | Vendors | Catalog consequence |
|---|---|---|
| Bundle: many formats from one job | Meshy (`model_urls`: glb, obj, fbx, stl, usdz, plus 3mf on request; `target_formats` narrows the set) | `max_items` can be > 1; ask for exactly what was requested |
| One-of: one container per job | Hyper3D Rodin (`geometry_file_format`, a scalar), Tencent Hunyuan3D | `max_items: 1`. litegen derives the rest from whichever container it gets; Rodin serves `[glb, fbx]` with its own fbx plus a derived glb |
| GLB-only, no choice | Stability (SF3D, SPAR3D), most fal and Replicate slugs | `enum_values: [glb]`, `max_items: 1`; send no format field |
| Convert as a second, billed job | Tripo (glb native; other formats need a separate convert task) | Declare only glb natively. Its `quad: true` silently forces FBX. |

Every vendor with a public API can emit **GLB**. That's why glb is mandatory
in every row. Textures differ too: Stability embeds them in the GLB, while
Meshy and Rodin return separate PBR maps, which become `Texture` files.

## First-party vendors

**Meshy** (`meshy`, new; docs.meshy.ai)
- Auth: `Authorization: Bearer`. Self-serve, but API access may need the Pro
  tier (the docs disagree).
- Text-to-3D is **two-stage**: `mode: "preview"`, then `mode: "refine"` with
  `preview_task_id`, at `POST /openapi/v2/text-to-3d`. This hits the
  multi-stage gap (see SKILL.md).
- Image-to-3D is single-stage (`POST /openapi/v1/image-to-3d`,
  `should_texture`). Multi-image takes 1–4 views.
- *Unverified*: whether text-to-3D is on v2 and image-to-3D on v1. Settle it
  with a live call.
- Output: per-format `model_urls`, `texture_urls` (base_color, metallic,
  normal, roughness, emission) and `thumbnail_url`.
- Images go as a URL or a base64 data URI.
- Params: `topology` (quad or triangle), `target_polycount` (100–300,000),
  `symmetry_mode` (**deprecated**), `texture_resolution` (2k, 4k, 8k),
  `art_style`, `should_remesh` and `pose_mode`.
- Pricing: credits. Preview ≈ 20 credits; refine 10–15; image-to-3D 20
  untextured or 30 textured.
- **No documented refund on failure.**

**Tripo / VAST AI** (`tripo3d`, new; platform.tripo3d.ai)
- Auth: `Authorization: Bearer`. Target the v3 surface
  (`https://openapi.tripo3d.ai/v3`).
- Single-stage for models from `v2.5-20250123` on: `POST
  /generation/{text,image,multiview}-to-model`, then `GET /tasks/{task_id}`.
- **"Model URLs expire after 5 minutes. Download immediately after the task
  succeeds."**
- Images go as a `file_token` (from `POST /upload`), a public URL, or an
  earlier `task_id`. There is no base64.
- `quad: true` forces FBX; `face_limit`; `pbr` bakes the maps into the model.
- *Unverified*: the output field names (`model_url` vs `model`) and the model
  id spelling disagree between the quick-start and the OpenAPI spec. One live
  call settles both.
- Pricing: 100 credits = $1. The sources disagree. Spec §9 says text-to-3D
  costs 10 or 20 credits and image-to-3D 20 or 30. The landscape doc says a
  base of 5 credits, or 10 when `quad`, `face_limit`, `texture_size` or
  another option is non-default. Settle it from the live pricing page.
- Documents freeze-then-refund on failure.

**Stability 3D** (extend `stability`; platform.stability.ai)
- Uses the same account key litegen already has, with the same Bearer auth.
- **Synchronous**: `POST /v2beta/3d/stable-fast-3d` or
  `/v2beta/3d/stable-point-aware-3d` returns the GLB in the response body.
  This needs the sync shim and its caveats (see SKILL.md).
- Input is multipart `image` (the materializer's `multipart` form).
- Params: `texture_resolution` (512, 1024, 2048), `foreground_ratio`,
  `remesh` (none, quad, triangle) and `vertex_count`, plus `target_type`,
  `target_count`, `guidance_scale` and `seed` on SPAR3D.
- Pricing: SF3D 10 credits ($0.10), SPAR3D 4 credits ($0.04). Failures aren't
  charged.
- SPAR3D is marked "currently in preview", so tag it `preview`. The request
  body is capped at 10 MiB, and the shared rate limit is 150 requests per
  10 s.

**Hyper3D Rodin direct** (`rodin`; docs.hyper3d.ai)
- Auth: Bearer.
- API access is **Business tier, about $1,152 a year: a purchase order, not a
  signup**. Ask the user before going any further.
- Three calls: a multipart submit (`images`, 1–5), then `POST /api/v2/status`
  (wait at least 5 s before the first check and honour `Retry-After`), then a
  separate `POST /api/v2/download` that returns `{name, url}` entries, which
  may expire.
- Tiers: Gen-2.5 Extreme-Low through Extreme-High ("always send a Gen-2.5
  tier explicitly").
- `format`: glb, usdz, fbx, obj or stl. `material` takes PBR, Shaded, All,
  Hybrid or None (the sources list different subsets). There's also `mesh_mode` (Raw or Quad), and `addons` such as
  HighPack (×3 price).
- Rodin **via fal** is already integrated as `fal/hyper3d-rodin`, a different
  schema. Don't mix up the two.

**Tencent Hunyuan3D** (extend `hunyuan`)
- Auth: TC3 signing (`providers/auth/tc3.rs`), not Bearer.
- Text, image and multiview. Outputs GLB by default, plus OBJ, FBX, STL and
  USDZ through an export option.
- Face count 3k–1.5M. A JobId is valid for 24 h.
- *Unverified*: the docs disagree on the namespace and host (`hunyuan`,
  version 2023-09-01, vs `ai3d`, version 2025-05-13). Resolve that first.
- Hunyuan3D v2 **via fal** is already integrated (`fal/hunyuan3d-v2`).

**Leads only:** Sloyd (widest export list; *unverified*, its docs host
failed DNS), Alpha3D (*unverified*), Kaedim (sales-gated) and Polycam
(photogrammetry, a different product).

## Aggregators

| Platform | Auth | 3D catalogue | Notes |
|---|---|---|---|
| **fal.ai** (integrated: `model3d/fal.rs`) | `Authorization: Key $FAL_KEY` (**not** Bearer) | Rodin, TRELLIS, Hunyuan3D, Tripo, Meshy… | The format field is spelled four ways (`output_format`, `geometry_file_format`, `export_format`, `target_formats`) and most slugs have none. Mesh output is `model_mesh` or `model_glb`. A failure is reported as `COMPLETED` + `error`. There are three error envelopes. A 429 is the concurrency limiter (2 concurrent runs for new accounts). Add new slugs as `EndpointSpec` arms. |
| **Replicate** (integrated for image and video) | token | Hunyuan3D 3.1, TRELLIS, Tripo | A 3D arm would be a new `model3d/replicate.rs` using the existing key |
| WaveSpeedAI | Bearer | 33 3D endpoints, including ByteDance Seed3D 2.0 (not on fal) | The broadest single-key catalogue |
| Runware | — | trellis-2, meshy-6, rodin-gen-2, sam-3d | `taskType: "3dInference"`, GLB only, billed by compute time |
| Segmind | — | glb, obj, ply, stl outputs | The landscape doc records only its formats |
| 3D AI Studio | Bearer | resells Hunyuan, Tripo, Meshy | GLB plus optional PBR; credit-based |

## No public generation API

Don't chase these: Luma Genie, CSM.ai, Masterpiece X, Spline, Backflip,
Google, OpenAI, Meta 3D Gen, and Stability SV3D (it exists as weights but
has no API).

## Recommended order

This is the landscape doc's order, as of 2026-09-12:

1. fal (done).
2. **Meshy**, the only natural bundle vendor. It needs the multi-stage
   decision for text-to-3D.
3. **Stability**: the key is already in hand, but it needs the sync shim.
4. Tripo direct, only if billing for per-format conversion is acceptable.
5. Rodin direct last, because of the $1,152-a-year floor.
