# Model drift: decisions from the 2026-10-12 run

This run repaired every broken source, closed the coverage gaps it found, fixed
every typing a vendor's own schema proved wrong, and removed the models whose
announced shutdown (scheduled 2026-09-11) has taken effect. The items below are
the **product owner's call**. Each is backed by the vendor evidence linked.

Tick a box (`- [x] add` / `- [x] skip`) or edit the row, and the next
`/model-drift` run acts on it:

- An added model gets a catalog row with verified typings.
- A skipped model gets an `acknowledged` entry (with the reason) in
  `scripts/model-drift/providers/<p>.mjs`.
- A removed model is deleted from `models/<p>.yaml` and from the drift map.

Legend: **y/n** = our adapter can serve it unchanged or not. Prices are the
vendor's published prices on 2026-10-12.

---

## 1. Dated shutdowns: act before the date

- [ ] **Google Veo 3.1 previews, 2026-10-22 (10 days).**
  `google/veo-3.1-generate-preview`, `-fast-` and `-lite-` shut down; Google's
  named replacement is `gemini-omni-1.1-flash`.
  https://ai.google.dev/gemini-api/docs/deprecations
  - They are in `SCHEDULED_SHUTDOWNS`, so CI fails from 2026-10-23 until they
    are removed.
  - The replacement is **n**: it runs on the Interactions API
    (`POST /v1beta/interactions`), which the 2026-09-11 decision left blocked
    (the router's 120 s attempt timeout with retries; `video_data` is dropped).
    Video only, 3–10 s, 360p/720p/1080p/4k, 16:9 or 9:16. About $0.10/s at
    720p ($17.50 per 1M video output tokens at 5,792 tokens/s).
    https://ai.google.dev/gemini-api/docs/pricing
  - Options: build the core change (re-host `video_data`) and add Omni, or let
    Google video go dark on 10-22 and remove the three rows.
- [ ] **`openai/gpt-image-1`, 2026-10-23.** Already scheduled; remove on the
  date. OpenAI now names `gpt-image-2.5-sunburst` or `-flare` as the
  replacement (we carry both). https://developers.openai.com/api/docs/deprecations

## 2. Already dead: every request fails today

- [ ] **`hunyuan/hunyuan-image-3`.** Tencent deleted `SubmitTextToImageJob` and
  `QueryTextToImageJob` on 2026-09-22 (aiart changelog release 31:
  https://cloud.tencent.com/document/product/1668/88078). The drift check now
  reports it as missing. Whether already-enabled accounts still get through is
  undocumented ("用户已购买的模型服务可继续使用").
  - Migrate: Hunyuan Image 3.0 lives on TokenHub as `hy-image-v3`. **n**: a
    new host (`tokenhub.tencentmaas.com`), a Bearer API key (a new credential
    type), a synchronous response, `size` as `WxH`, boolean `revise`,
    `footnote` in place of `LogoAdd`. Same price, 0.2 CNY/image.
    https://cloud.tencent.com/document/product/1823/135745
  - Or remove the model.
  - Tencent is moving all of Hunyuan to TokenHub; the old platform gets no new
    models, and no offline date is announced yet. A TokenHub client would also
    serve `hy-video-v1.5` (§3).
- [ ] **`leonardo/kling2.1`.** Leonardo retired Kling 2.1 Pro on 2026-09-15:
  "After September 15, requests using v2.1/pro will return an error."
  https://docs.leonardo.ai/docs/kling-2-1-pro.md. It was announced only in that
  guide's callout (not the Deprecations page), and the v1.0 docs the check
  read still listed it. Recommended: remove (the successor, Kling 3.0 Pro, is
  v2-API only, **n**). The new `kling21-guide` drift source now snapshots
  that callout.
- [ ] **Adapters that now serve no catalog model.** `image/bedrock.rs` and
  `video/bedrock.rs` (they build only the Nova body) and `video/openai.rs`
  (Sora). Their unit tests now load the removed rows from
  `litegen-core/tests/fixtures/retired-models.yaml`. Keep them for a future
  model, or delete them?

## 3. New models

| ✔ | provider · id | what | y/n | vendor price | recommendation |
|---|---|---|---|---|---|
| [ ] | google · `gemini-nano-banana-2.1` | Nano Banana 2.1, GA 2026-10-06; Google's successor to `gemini-3.1-flash-image` (now deprecated). 14 ratios; 1K/2K/4K; 14 refs | n (one `resolve_model` line; unknown ids fall back to 3.1 Flash today) | $0.0336 (1K) · $0.0504 (2K) · $0.113 (4K) https://ai.google.dev/gemini-api/docs/pricing | **Add**; consider making it the adapter's fallback |
| [ ] | bytedance · `dola-seedream-5-0-flash-260915` | Seedream 5.0 flash: fast t2i, 1–10 refs | y | $0.018/image https://docs.byteplus.com/en/docs/modelark/model-pricing | **Add**: presets 1K/1.5K/2K (default 2K), 921,600–4,624,220 px at ratio [1/16, 16], refs ≤10, no seed |
| [ ] | recraft · `recraftv4_1_flash` | V4.1 Flash raster, ~1.3 s, no styles | y | $0.007/image https://www.recraft.ai/docs/api-reference/pricing.md | **Add** (prompt 10000, 1K sizes) |
| [ ] | vidu · `viduq4-preview` | Vidu's new flagship (2026-10-07): img2video + reference2video, 3–16 s, up to 4K, ≤15 refs | y (catalog only: no `last_frame` role, `text_to_video` false) | 19 credits/s at 720p ≈ $0.475 per 5 s https://platform.vidu.com/docs/overview/pricing.md | Add or wait for GA. Note `is_rec` defaults to true (Vidu rewrites the prompt) |
| [ ] | replicate · `black-forest-labs/flux-3-image` | FLUX 3 Image (2026-10-01) | n (one `resolve_model_version` arm for t2i; edit takes an `images` array) | $0.0205 (768sq) · $0.024 (1k) · $0.05 (2k) · $0.3035 (4k) https://replicate.com/black-forest-labs/flux-3-image | **The FLUX 3 route if you want one**: half BFL's price |
| [ ] | bfl · `flux-3-image` | the same model, direct | n (`Flux3ImageInputs` is `additionalProperties: false`; our body sends `output_format`, `seed`, `width`, `height` → 422) | $0.041 · $0.048 · $0.100 · $0.607 by tier https://docs.bfl.ml/quick_start/pricing.md | Decide with Replicate's; skip if Replicate |
| [ ] | fal · `blackforestlabs/flux-3/text-to-image`, `/edit-image` | the same model on fal | n (no `resolve_endpoint` arm: falls back to flux/dev) | $0.048/MP | Skip (decide with the two above) |
| [ ] | ideogram · `ideogram-4-5` | Ideogram 4.5 (2026-09-30), v2-API only | n (a v2 branch: new path, `size`, `quality`) | Ideogram's page did not load; fal resells it at $0.03/0.06/0.22 | Add later, after confirming Ideogram's own price |
| [ ] | ideogram · `auto`, `ideogram-2`, `-2a`, `gpt-image-2*`, `nano-banana-*`, `z-image` | v2 router, legacy 2.x, and resold OpenAI/Google/Alibaba models | n | — | Skip (we carry the originals direct) |
| [ ] | kling · `kling-3.0-turbo` | Kling 3.0 Turbo, new API only | n (new body, API-key auth, `GET /tasks`) | $0.112/s (720p) · $0.14/s (1080p) https://kling.ai/document-api/pricing/base/video.md | Skip unless the video adapter moves to Kling's new API (see §5) |
| [ ] | runway · `grok_imagine_1_5_lite` | xAI Grok Imagine Video 1.5 Lite, resold | n (unknown ids bill as `gen4_turbo`) | 2/3/14 credits/s at 480p/720p/1080p https://docs.dev.runwayml.com/guides/pricing.md | Skip; batch with the 2026-09-11 `grok_imagine_1_5` decision |
| [ ] | hunyuan · `hy-image-v3`, `hy-video-v1.5`, `hy-image-v3.5-preview` | TokenHub models | n (TokenHub client) | 0.2 CNY/img · 0.3 CNY/s · 0.15–0.2 CNY/img https://cloud.tencent.com/document/product/1823/130055 | Tie to §2 hunyuan-image-3 |
| [ ] | bedrock · `stability.sd3-5-large-v1:0`, `stability.stable-image-core-v1:1`, `stability.stable-image-ultra-v1:1` | Stability models hosted on Bedrock | n (adapter builds only the Nova body) | $0.08 · $0.04 · $0.14 (direct: $0.065 · $0.03 · $0.08) https://aws.amazon.com/bedrock/pricing/ | Skip ("carried direct from Stability") unless you want an AWS-billed route |
| [ ] | fal · `fal-ai/flux-3-action/so101` | robotics action model (SO-101 arm) | n | $0.003/compute-s | Skip: not a media model |
| [ ] | fal · `recraft/v4.1/flash/text-to-image` | Recraft V4.1 Flash on fal | n | $0.007 | Skip: Recraft direct covers it |

**fal 3D** (the families we now carry; all **n**, each needs a `resolve_spec`
branch in `model3d/fal.rs`; prices from `https://fal.ai/models/<id>/llms.txt`):

| ✔ | id | what | price | recommendation |
|---|---|---|---|---|
| [ ] | `fal-ai/hunyuan3d/v2/turbo`, `/mini`, `/mini/turbo` | v2 variants, same input schema | $0.14 · $0.10 · $0.08 (3× textured) | Cheap adds; turbo most likely |
| [ ] | `fal-ai/hyper3d/rodin/v2.5` (+ `/text-to-3d`, `/fast`, `/text-to-3d/fast`) | Rodin Gen-2.5; images in `image_urls` (≤5) | $0.40 (+$0.80 HighPack) · fast $0.10 | Strongest Rodin add |
| [ ] | `fal-ai/hyper3d/rodin/v2` | Rodin Gen-2 | $0.40 | Consider |
| [ ] | `fal-ai/trellis/multi` | TRELLIS, several images | $0.02 | Cheap add |
| [ ] | `fal-ai/trellis-2` | TRELLIS.2 (`model_glb` output) | $0.25/0.30/0.35 at 512/1024/1536 | Consider |
| [ ] | `fal-ai/hunyuan-3d/v3.1/pro/{image,text}-to-3d`, `fal-ai/hunyuan3d-v3/{image,text}-to-3d` | Hunyuan 3D v3 / v3.1 Pro | $0.375 (+$0.15 add-ons) | Consider v3.1 Pro (supersedes v3) |
| [ ] | `fal-ai/hunyuan-3d/v3.1/rapid/{image,text}-to-3d` | v3.1 Rapid (text side returns `model_obj`) | $0.225 | Image side only, for now |
| [ ] | multi-view (×2), `v3.1/part`, `smart-topology`, `trellis-2/retexture`, `trellis-2-lora`, `hunyuan3d-v3/sketch-to-3d` | multi-image, mesh-input or LoRA tools | — | Skip |

## 4. Prices that differ from the vendor's

- [ ] `google/gemini-2.5-flash-image`: we bill **$0.02**; Google charges
  **$0.039** per image (standard), $0.0195 batch.
  https://ai.google.dev/gemini-api/docs/pricing
- [ ] `vidu/viduq3-turbo`: we bill **$0.275**; 720p text/img/start-end2video is
  now 12 credits/s, **$0.30** per 5 s (reference2video unchanged at 10).
  https://platform.vidu.com/docs/overview/pricing.md
- [ ] `vidu/viduq2-pro`: we bill **$0.40**; 5 s 720p is 55 credits (**$0.275**)
  on img2video/start-end2video, 50 (**$0.25**) on reference2video, $0.575 at
  1080p. Same URL.
- [ ] `vidu/viduq3-drama`, `vidu/viduq3-ad`: skipped 2026-09-11 because "no
  endpoint accepts it"; both now have reference2video operations. Re-decide.
- FYI: `openai/gpt-image-1` is $0.042 vs our $0.04 (it dies 10-23; leave it).
  `bfl/flux-dev` no longer appears in BFL's FLUX.1 price table, though
  `/v1/flux-dev` is still in the spec: a possible quiet delisting.

## 5. Adapter bugs the vendor schemas expose (pre-existing; not drift)

Found while auditing coverage. None was changed in this run. Each needs a
wiremock test with its fix.

- [ ] **Wrong `content_type` on image output.** Each adapter labels bytes in a
  format the vendor did not send by default:
  - `image/replicate.rs:408` says png; FLUX/SD3 default to webp.
  - `image/recraft.rs` says png; raster defaults to lossless WebP (send
    `image_format: png`).
  - `image/bytedance.rs:244` says png; Seedream defaults to jpeg.
  - `image/luma.rs:281` says jpeg; `format: png` is allowlisted.
  - `image/openai.rs` says png; `output_format` jpeg/webp is allowlisted.
- [ ] **Fields an endpoint does not define.** `image/openai.rs` forwards
  `moderation` to multipart `/images/edits` (generations-only). PixVerse
  allowlists `negative_prompt`, `motion_mode`, `style` (no schema has them; the
  adapter also sends `negative_prompt`). `video/kling.rs` sends
  `aspect_ratio` on image2video.
- [ ] **Ideogram.** The v3 JSON body for requests without refs (the spec
  declares multipart only; unverified live); the multipart path drops array
  and object extras (`style_codes`, `color_palette`); `num_images = n` is
  forwarded while one image is returned and billed.
- [ ] **Kling.** The documented host moved to `api-singapore.klingai.com`
  (legacy routes still answer on `api.klingai.com`); the legacy
  `/v1/videos/*` endpoints the video adapter calls are no longer documented
  (Kling says they keep working; every new video model is new-API only).
- [ ] **ByteDance.** Seedance `duration` is an integer; the catalog says float
  and the adapter truncates 4.5 s to 4.
- [ ] **Recraft (medium confidence; verify live).** Generation `style` now
  takes curated style names (V3 default "Recraft V3 Raw"; `substyle` is gone);
  `recraftv3_vector` takes `w:h` ratios only, not `WxH` sizes.

## 6. Can't express (what we accept instead)

- Durations as sets: Kling 2.5 Turbo/2.6 and Leonardo Kling {5, 10}; Veo
  {4, 6, 8} (8 at 1080p/4k or with refs); PixVerse v3.5/v4.5/v5 {5, 8}; Luma
  {5, 9}; vidu2.0 {4, 8}. We accept the range.
- Per-endpoint limits for one model: Vidu prompt (2000 on img2video, 5000 or
  20000 elsewhere; we take the strictest), Vidu resolutions/durations per
  endpoint, Kling `aspect_ratio` on text-to-video only.
- Per-model `imageSize` on Gemini (passed through `extra_allowlist`
  unchecked); GPT Image 2* pixel-count and 1:3–3:1 ratio rules (held as
  per-edge bounds); Seedream preset sizes and pixel-count rules (held as a
  width × height box).
- "No prompt" on a 3D model (TRELLIS, Hunyuan3D v2): handled in the adapter
  and validator by the 3D session.
- Tiered prices (FLUX 3 by resolution, Ideogram 4.5 by pixels, Vidu by
  resolution): the catalog bills flat.
