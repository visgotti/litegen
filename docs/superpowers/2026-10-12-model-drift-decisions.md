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
- [ ] **Adapters that now serve no catalog model.** (2026-11-04: `image/bedrock.rs` serves Stability's models now; 2026-11-10: `video/bedrock.rs` serves Luma Ray 2, its Nova Reel body removed.) `image/bedrock.rs` and
  `video/bedrock.rs` (they build only the Nova body) and `video/openai.rs`
  (Sora). Their unit tests now load the removed rows from
  `litegen-core/tests/fixtures/retired-models.yaml`. Keep them for a future
  model, or delete them?

## 3. New models

| ✔ | provider · id | what | y/n | vendor price | recommendation |
|---|---|---|---|---|---|
| [ ] | google · `gemini-nano-banana-2.1` | Nano Banana 2.1, GA 2026-10-06; Google's successor to `gemini-3.1-flash-image` (now deprecated). 14 ratios; 1K/2K/4K; 14 refs | n (one `resolve_model` line; unknown ids fall back to 3.1 Flash today) | $0.0336 (1K) · $0.0504 (2K) · $0.113 (4K) https://ai.google.dev/gemini-api/docs/pricing | **Add**; consider making it the adapter's fallback |
| [x] ✅ 2026-11-03: added (`bytedance/dola-seedream-5-0-flash-260915`, with 5.0 Pro) | bytedance · `dola-seedream-5-0-flash-260915` | Seedream 5.0 flash: fast t2i, 1–10 refs | y | $0.018/image https://docs.byteplus.com/en/docs/modelark/model-pricing | **Add**: presets 1K/1.5K/2K (default 2K), 921,600–4,624,220 px at ratio [1/16, 16], refs ≤10, no seed |
| [x] ✅ 2026-11-03: added (`recraft/recraftv4_1_flash`) | recraft · `recraftv4_1_flash` | V4.1 Flash raster, ~1.3 s, no styles | y | $0.007/image https://www.recraft.ai/docs/api-reference/pricing.md | **Add** (prompt 10000, 1K sizes) |
| [ ] | vidu · `viduq4-preview` | Vidu's new flagship (2026-10-07): img2video + reference2video, 3–16 s, up to 4K, ≤15 refs | y (catalog only: no `last_frame` role, `text_to_video` false) | 19 credits/s at 720p ≈ $0.475 per 5 s https://platform.vidu.com/docs/overview/pricing.md | Add or wait for GA. Note `is_rec` defaults to true (Vidu rewrites the prompt) |
| [ ] | replicate · `black-forest-labs/flux-3-image` | FLUX 3 Image (2026-10-01) | n (one `resolve_model_version` arm for t2i; edit takes an `images` array) | $0.0205 (768sq) · $0.024 (1k) · $0.05 (2k) · $0.3035 (4k) https://replicate.com/black-forest-labs/flux-3-image | **The FLUX 3 route if you want one**: half BFL's price |
| [x] ✅ 2026-11-03: added (`bfl/flux-3-image`, with Klein): the adapter sends output_format only to rows that declare it, references as `images` | bfl · `flux-3-image` | the same model, direct | n (`Flux3ImageInputs` is `additionalProperties: false`; our body sends `output_format`, `seed`, `width`, `height` → 422) | $0.041 · $0.048 · $0.100 · $0.607 by tier https://docs.bfl.ml/quick_start/pricing.md | Decide with Replicate's; skip if Replicate |
| [x] ✅ 2026-11-04: added (`fal/flux-3-image`, `fal/flux-3-image-klein`) under the hub-route policy: text-to-image, or the `/edit-image` twin when images come (`edit_twin`, as `image_urls`). The klein `/lora` twins are skipped, since litegen takes no LoRA input. Klein *base* and FLUX 3 Max video are open (below) | fal · `blackforestlabs/flux-3/text-to-image`, `/edit-image` | the same model on fal | n (no `resolve_endpoint` arm: falls back to flux/dev) | $0.048/MP | Skip (decide with the two above) |
| [ ] | ideogram · `ideogram-4-5` | Ideogram 4.5 (2026-09-30), v2-API only | n (a v2 branch: new path, `size`, `quality`) | Ideogram's page did not load; fal resells it at $0.03/0.06/0.22 | Add later, after confirming Ideogram's own price |
| [x] ✅ 2026-11-04: `ideogram/nano-banana-2` and `-pro` added on v2 under the hub-route policy (a resold model is carried even when litegen has the original). Still skipped: `gpt-image-2*` (its "Usage-based" rates sit in a collapsed panel, unread), `nano-banana-2-1` and `z-image` (no Ideogram price), `auto` (Ideogram picks the model) and legacy 2.x | ideogram · `auto`, `ideogram-2`, `-2a`, `gpt-image-2*`, `nano-banana-*`, `z-image` | v2 router, legacy 2.x, and resold OpenAI/Google/Alibaba models | n | — | Skip (we carry the originals direct) |
| [x] | ideogram · `p-image-ideogram` (`/v2/image/generate/p-image-ideogram`), `gpt-image-2`, `gpt-image-2-5-flare`, `gpt-image-2-5-sunburst` | P-Image (budget tier, JSON body) and the GPT Image resales | P-Image y | P-Image $0.003-$0.066 by quality × 1K/2K; GPT Image "Usage-based" | ✅ 2026-11-04: `ideogram/p-image-ideogram` added (always sends a quality, so the bill is deterministic). GPT Image stays skipped, now for its real reason: Ideogram bills OpenAI tokens as used and the request sets no quality, so no price can be quoted (its own estimate at 1024² runs $0.0069-$0.2118; read from https://ideogram.ai/api/usage-pricing/catalog) |
| [ ] | kling · `kling-3.0-turbo` | Kling 3.0 Turbo, new API only | n (new body, API-key auth, `GET /tasks`) | $0.112/s (720p) · $0.14/s (1080p) https://kling.ai/document-api/pricing/base/video.md | Skip unless the video adapter moves to Kling's new API (see §5) |
| [x] | runway · `gpt_image_2`, `gpt_image_2_5_{flare,sunburst}`, `gpt_image_2_6_{flare,sunburst}`, `gemini_2.5_flash`, `gemini_image3_pro`, `gemini_image3.1_flash`, `gemini_image3.1_flash_lite`, `nano_banana_2.1`, `muse_image`, `seedream5_{pro,lite,flash}`, `grok_imagine_image_2`, `bria_fibo_1_5`, `bria_fibo_edit_1_5`, `ideogram_4_0`, `ideogram_4_5`, `ideogram_vector`, `recraft_v4_1_{flash,vector}` | the image models Runway resells on `/v1/text_to_image` | y (a branch each: `image/runway.rs` `RESOLD`) | $0.01-$0.75 per image by quality, tier and aspect, + $0.01 per reference on GPT 2.5 / 2.6 and Grok https://docs.dev.runwayml.com/guides/pricing.md | ✅ 2026-11-10: 20 rows added as `runway/<model>` under the hub-route policy (the 2026-09-11 "carried direct" skip reversed): `aspect_ratio` + `resolution` pick each branch's `ratio` size. Skipped: `recraft_v4_1_flash` and `recraft_v4_1_vector` (no price on the guide). The guide is now a drift source (`pricing`). Not yet: `/v1/image_to_vector`, `/v1/image_upscale`, the routed `/v1/generate/*` (a saved Model Router config, no model) |
| [x] | ideogram · `video/kling-3-standard`, `video/minimax-h3`, `video/seedance-2`, `video/seedance-2-5` (`/v2/video/generate/{model}-{text,image,reference}-to-video`), `kling_4` (priced only) | the video models Ideogram resells on v2 | y (a new video/ideogram.rs on the same Api-Key) | per second: Kling 3.0 Std $0.084 ($0.126 audio); MiniMax H3 $0.05–$0.16 (+$0.04 per reference past five); Seedance 2.0 $0.07/$0.16; Seedance 2.5 $0.103/$0.231/$0.614 https://ideogram.ai/api/usage-pricing/catalog | ✅ 2026-11-10: 4 rows added as `ideogram/<model>`; the price catalog is now a drift source (`prices`). Skipped: Kling 4.0 (priced at /v2/video/generate/kling-4, but in neither the spec nor the docs) and the Seedance 2.0 video edit (no endpoint) |
| [x] | runway · `veo3.1`, `veo3.1_fast`, `gemini_omni_flash{,_1.1,_1.2}`, `kling2.5_turbo_pro`, `kling3.0_{standard,pro,4k}`, `klingO3_{standard,pro,4k}`, `kling_4`, `kling_4_flash`, `hailuo3`, `h3_max`, `seedance2{,_fast,_mini,_5}`, `grok_imagine_1_5{,_lite}`, `wan3{,_prime}`, `happyhorse_1_0` | the video models Runway resells on `/v1/text_to_video` and `/v1/image_to_video` | y (two branches each: `video/runway.rs` `RESOLD`) | 2–150 credits/s by tier and audio, + per-reference credits https://docs.dev.runwayml.com/guides/pricing.md | ✅ 2026-11-10: 25 rows added as `runway/<model>` (the 2026-09-11 "carried direct" skips reversed; `runway` leaves the drift guard's PENDING). Not taken: reference video and audio, `duration: "auto"`, Seedance 2.5 `outputFormat: mov`. Not yet: `/v1/video_to_video` (aleph2 and the v2v branches: litegen's source-video operations), `/v1/video_upscale`, `/v1/video_to_hdr` |
| [x] | runway · `grok_imagine_1_5_lite` | xAI Grok Imagine Video 1.5 Lite, resold | y | 2/3/14 credits/s at 480p/720p/1080p https://docs.dev.runwayml.com/guides/pricing.md | ✅ 2026-11-10: added as `runway/grok_imagine_1_5_lite` with the other video resales (row below) |
| [x] ✅ 2026-11-04: a provider of its own, `tokenhub` (Bearer key; `hunyuan` stays for the old platform's buyers): `tokenhub/hy-image-v3.5-preview`, `tokenhub/hy-video-v1.5`, plus `tokenhub/wand-vega-video-1.0-lite`; `hy-image-v3` skipped (TokenHub retires it 2026-11-12) | hunyuan · `hy-image-v3`, `hy-video-v1.5`, `hy-image-v3.5-preview` | TokenHub models | n (TokenHub client) | 0.2 CNY/img · 0.3 CNY/s · 0.15–0.2 CNY/img https://cloud.tencent.com/document/product/1823/130055 | Tie to §2 hunyuan-image-3 |
| [x] | bedrock · `luma.ray-v2:0` | Luma Ray 2 on Bedrock (StartAsyncInvoke; the MP4 to the caller's S3 bucket); the 2026-09-11 skip "carried direct from Luma" falls under the hub-route policy | y (video/bedrock.rs: Ray 2's body, the MP4 read back from S3) | $1.50/s 720p, $0.75/s 540p (us-west-2) https://aws.amazon.com/bedrock/pricing/ | ✅ 2026-11-10: added as `bedrock/luma.ray-v2:0` (Nova Reel's body removed: EOL 2026-09-30); `bedrock` leaves the drift guard's PENDING. Open: a dashboard field for `s3_output_uri` (an OpenAPI change, known-gaps) |
| [x] ✅ 2026-11-04: added (`bedrock/stability.*`, Stability's InvokeModel body, us-west-2) | bedrock · `stability.sd3-5-large-v1:0`, `stability.stable-image-core-v1:1`, `stability.stable-image-ultra-v1:1` | Stability models hosted on Bedrock | n (adapter builds only the Nova body) | $0.08 · $0.04 · $0.14 (direct: $0.065 · $0.03 · $0.08) https://aws.amazon.com/bedrock/pricing/ | Skip ("carried direct from Stability") unless you want an AWS-billed route |
| [ ] | fal · `fal-ai/flux-3-action/so101` | robotics action model (SO-101 arm) | n | $0.003/compute-s | Skip: not a media model |
| [x] | fal · `recraft/v4.1/flash/text-to-image` | Recraft V4.1 Flash on fal | y | $0.007 | ✅ 2026-11-04: added as `fal/recraft-v4.1-flash` (the hub-route policy reversed "Recraft direct covers it") |
| [x] | fal · `fal-ai/recraft-20b`, `fal-ai/recraft/v4[.1][/pro][/utility]/text-to-{image,vector}` | Recraft V2 (the listed id, its page now Recraft V2), V4 and V4.1 on fal | y | V2 $0.022 ($0.044 vector style); V4 $0.04 / pro $0.25 / vector $0.08 / pro vector $0.30; V4.1 $0.035 / pro $0.21 / vector $0.08 / pro vector $0.30 / utility $0.035 / utility pro $0.21 | ✅ 2026-11-04: 11 rows added beside the flash one; the 2026-09-11 "Recraft direct covers it" skip is gone, so fal leaves the drift guard's PENDING list |
| [x] | fal · `fal-ai/recraft/v3/image-to-image`, `recraft/v4/style/*`, `*/create-style`, `fal-ai/recraft/vectorize`, `fal-ai/recraft/upscale/{crisp,creative}` | the rest of Recraft on fal | y | $0.04 / $0.035-$0.12 (+$0.005 to make a style) / $0.01 / $0.004 / $0.25 | ✅ 2026-11-04: V3's image-to-image twin; `fal/recraft-v4-style`, `-style-pro`, `-style-vector`, `-style-pro-vector`, `fal/recraft-vectorize`, `-upscale-crisp`, `-upscale-creative` added. Skipped: the create-style endpoints (they answer a style_id, not an image; the id goes to the style rows as `extra.style_id`) |
| [x] | fal · `blackforestlabs/flux-3/klein/base/{text-to-image,edit-image}/lora` | FLUX 3 Klein *base* (undistilled; `loras` optional, default []) (seen 2026-11-04) | y (a new row: these endpoints, `loras` never sent) | "$0.035 per megapixel of output … up to 1024x1024 counts as 1 megapixel, 2048x2048 as 1.56" | ✅ 2026-11-04: added as `fal/flux-3-image-klein-base` ($0.035 up to 1 MP, $0.0546 above it; sizes stop at 2048² pixels, the largest fal prices) |
| [x] | fal · `blackforestlabs/flux-3-max/{text,image,keyframes}-to-video` | FLUX 3 Max video, fal's post-trained FLUX 3 Video (spec path `/fal-ai/alligator/…`): 5–20 s integer, 720p/1080p, audio, `reasoning_level` (seen 2026-11-04) | y | list $0.17/s 720p, $0.29/s 1080p (75 % off until 2026-11-07) | ✅ 2026-11-04: added as `fal/flux-3-max-video` at the list price; the last frame goes as image-to-video's `end_image_url` |
| [x] | fal · `blackforestlabs/flux-3/{text,image,first-last-frame,keyframes}-to-video` and their `/draft` twins | FLUX 3 video and its 720p drafts; the 2026-09-11 skip "decide together with BFL's own flux-3-video" falls under the hub-route policy | y | $0.17/s 720p, $0.29 1080p, $0.32 1440p, $0.47 2160p (first-last and keyframes 720p/1080p only); drafts $0.06/s | ✅ 2026-11-04: added as `fal/flux-3-video` and `fal/flux-3-video-draft`; 1440p/2160p with frames or keyframes is refused (unpriced there) |
| [x] | fal · `blackforestlabs/flux-3/draft-enhance` | re-renders a FLUX 3 draft from its `draft_cache` at 1080p | n | $0.29/s | Skip (2026-11-04): litegen's video output keeps no `draft_cache` |
| [ ] | fal · `blackforestlabs/flux-3/edit-video`, `extend-video`, `extend-video/draft` | FLUX 3 edit and extension of a source clip | n (litegen's `edit` / `extend` tasks exist; no vendor row yet) | edit $0.03/s (720p); extend $0.41/s 720p, $0.53/s 1080p | Carry as fal video operations once pricing can read the source clip's length: edit bills "per second of generated video", i.e. the source's own length, which PriceFacts can't see (the measured source isn't in the request params). Extend prices from its added `duration_seconds` and could land now; both wait so they land together (backlog, 2026-11-04) |
| [x] | fal · `fal-ai/flux-2{,-pro,-max,-flex}`, `fal-ai/flux-2/{turbo,flash}`, `fal-ai/flux-2/klein/{4b,9b}[/base]`, each with its `/edit` | FLUX.2 on fal. The 2026-09-11 skips ("BFL direct is our FLUX.2 route", "we carry the base fal-ai/flux-2") fall under the hub-route policy | y | per megapixel; pro/max/flex by rounded-up megapixel bands; edits add each input | ✅ 2026-11-04: 9 rows added (`fal/flux-2-turbo`, `-flash`, `-pro`, `-max`, `-flex`, `-klein-4b`, `-klein-9b`, `-klein-4b-base`, `-klein-9b-base`), and `fal/flux-2` gains its /edit twin. Skipped: the FLUX.2 LoRA endpoints (litegen takes no LoRA input) and `fal-ai/flux-2/klein/realtime` (realtime, per compute second). Outpaint added too, as `fal/flux-2-pro-outpaint` ($0.045, one megapixel in and one out, through `expand`) |
| [x] | fal · `fal-ai/flux-pro/kontext[/max][/text-to-image|/multi]`, `v1.1-ultra[/redux]`, `v1.1/redux`, `v1/fill`, `v1/erase` | FLUX.1 [pro] variants on fal; the 2026-09-11 skip "BFL direct covers them" falls under the hub-route policy | y | Kontext $0.04 / $0.08 per image, ultra $0.06, Redux and fill $0.05/MP, erase $0.03 first MP + $0.004 | ✅ 2026-11-04: added `fal/flux-kontext-pro`, `-max`, `fal/flux-pro-1.1-ultra`, `-1.1-redux`, `-1.1-ultra-redux`, `fal/flux-pro-fill`, `-erase`. Skipped: `v1/vto` (virtual try-on: no try-on input) and the `-finetuned` endpoints (BFL fine-tunes: no fine-tune input) |

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
