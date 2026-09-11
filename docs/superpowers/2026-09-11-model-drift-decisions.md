# Model drift: decisions from the first run (2026-09-11)

The first `/model-drift` run fixed every typing that a vendor's own schema
proved wrong; see the commit log. The items below are the ones that are the
**product owner's call**: models to remove, replace or add, and prices to
change. Each item is backed by the vendor evidence in the linked docs.

Tick a box (`- [x] add` / `- [x] skip`) or edit the row, and the next
`/model-drift` run acts on it:

- An added model gets a catalog row with verified typings.
- A skipped model gets an `acknowledged` entry (with the reason) in
  `scripts/model-drift/providers/<p>.mjs`.
- A removed model is deleted from `models/<p>.yaml` and from the drift map.

Legend: **y/n** = our adapter can serve it unchanged or not. Prices are the
vendor's published prices on 2026-09-11.

---

## 1. Dated shutdowns: act before the date

- [x] ✅ 2026-09-11: successors added: `kling/video-kling-v2-5-turbo`, `kling/video-kling-v2-6` and Image 2.1 `kling/kling-v2-1`. The four retiring models are in `SCHEDULED_SHUTDOWNS`, so CI fails from 9/16 until they are removed.
  **Kling, 2026-09-15 (4 days).** All four carried models retire:
  `kling/kling-v2`, `kling/kling-v1-5`, `kling/video-kling-v1-6` and
  `kling/video-kling-v2-1`.
  https://kling.ai/document-api/api/image/2-0/image-generation.md
  - Recommended replacements, all y (they are in the same legacy endpoints'
    `model_name` enums):
    - video: `kling-v2-5-turbo` ($0.042/s at 720p, $0.07/s at 1080p) and
      `kling-v2-6` (adds native audio);
    - image: Image 2.1, `kling-v2-1` on `/v1/images/generations`
      ($0.014 text-to-image, $0.028 image-to-image);
    - `kling-v3` optional ($0.084–0.168/s).
  - Pricing: https://kling.ai/document-api/pricing/base/video.md
- [x] ✅ 2026-09-11: switched to the native `SubmitHunyuanToVideoJob` / `DescribeHunyuanToVideoJob` (720p, prompt ≤200 characters). The Kling-V1-6 dependency is gone.
  **`hunyuan/hunyuan-video`, same date.** It is not a Hunyuan model: it
  calls Tencent vclm `SubmitImageToVideoJob` with `Model: "Kling-V1-6"`
  hard-coded, which is the Kling version retiring 9/15.
  - Replace with `SubmitHunyuanToVideoJob`, the native Hunyuan video action
    (720p, prompt ≤200 characters). It is n: a new action plus its poll.
  - Or rename the model and bump `Model` to `Kling-V2-6`, which is y.
  - https://cloud.tencent.com/document/product/1616/130567
- [x] ✅ 2026-09-11: scheduled in `SCHEDULED_SHUTDOWNS`; remove on the date.
  **OpenAI Sora, 2026-09-24.** `openai/sora` and `openai/sora-2-pro` go
  away together with the whole Videos API. There is no replacement: remove
  both. https://developers.openai.com/api/docs/deprecations
- [x] ✅ 2026-09-11: scheduled in `SCHEDULED_SHUTDOWNS`; remove on the date.
  **Bedrock Nova, 2026-09-30.** `bedrock/amazon.nova-canvas-v1:0` and
  `bedrock/amazon.nova-reel-v1:1` reach end of life.
  - As Legacy models they are already refused to new customers.
  - Bedrock has no Amazon successor; Titan Image v2 is past end of life
    too. Remove both.
- [x] ✅ 2026-09-11: scheduled in `SCHEDULED_SHUTDOWNS`; remove on the date.
  **`google/gemini-2.5-flash-image`, 2026-10-02.** Remove.
  `resolve_model` already falls back to `gemini-3.1-flash-image`.
  https://ai.google.dev/gemini-api/docs/deprecations
- [x] ✅ 2026-09-11: scheduled in `SCHEDULED_SHUTDOWNS`; already marked DEPRECATED, and gpt-image-2 can edit.
  **`openai/gpt-image-1`, 2026-10-23.** Replace with `gpt-image-2`. The
  catalog now marks it DEPRECATED and gives `gpt-image-2` editing and
  inpainting.

## 2. Already dead: every request fails today

- [x] ✅ 2026-09-11: removed (moved to `no_retired_model_ids_are_advertised`).
  **`luma/ray-3` and `luma/ray-hdr-3`.** They are not in the Dream Machine
  model enum, which is `ray-2 | ray-flash-2`.
  - Remove them.
  - The successor, `ray-3.2`, exists only on the separate Luma Agents API
    (new adapter, own keys): $0.15 / $0.30 / $1.20 per 5 s at 540p / 720p /
    1080p.
  - https://docs.lumalabs.ai/reference/creategeneration
- [x] ✅ 2026-09-11: removed. Successors added: `seedream-5-0-lite-260128` and `seedance-1-0-pro-fast-251015` (see §3).
  **`bytedance/seedream-3-0-t2i-250415` and
  `bytedance/doubao-seedance-1-0-lite-i2v-250428`.** Both were deactivated
  2026-05-13. https://docs.byteplus.com/en/docs/ModelArk/1350667
  - Replace them with `seedream-5-0-lite-260128` and
    `seedance-1-0-pro-fast-251015`, which are the vendor's recommended
    successors.
- [x] ✅ 2026-09-11: removed. `leonardo/veo3.1` (VEO3_1) and `leonardo/kling2.5` (KLING2_5) added; their prices are unverified because Leonardo publishes them only behind a login.
  **`leonardo/veo3`.** Retired 2026-06-29.
  - Replace with `VEO3_1`. It needs one mapping line: unknown ids currently
    fall back to MOTION2 silently.
  - https://docs.leonardo.ai/docs/deprecations-changes

## 3. Recommended additions

| provider | id | what it is | y/n | price | |
|---|---|---|---|---|---|
| openai | `gpt-image-2.5-flare`, `gpt-image-2.5-sunburst` | GPT Image 2.5, released 2026-09-08; OpenAI's recommended model for new integrations | y (a catalog row; quality adds `xhigh`/`max`) | $8 in / $30 out per 1M image tokens | - [x] added ✅ 2026-09-11 |
| kling | `kling-v2-5-turbo`, `kling-v2-6`, Image 2.1 | the 9/15 successors (section 1) | y | see §1 | - [x] added ✅ 2026-09-11 |
| vidu | `viduq3-turbo`, `viduq3-pro`, `viduq2-pro-fast` | Q3 generation, plus a cheap Q2 Pro | y | $0.055/s; $0.045–0.12/s; $0.04 + $0.01/s | - [x] added ✅ 2026-09-11 |
| pixverse | `v6`, `c1` | PixVerse's two recommended models (everything else is now "Legacy") | y | $0.35 / $0.40 for 5 s at 540p | - [x] added ✅ 2026-09-11 |
| ideogram | `ideogram-v4` | Ideogram 4.0, the flagship | n (a small v4 branch: new path, `text_prompt`) | $0.03 / $0.06 / $0.10 | - [ ] add |
| minimax | `MiniMax-Hailuo-2.3-Fast` | the cheapest Hailuo image-to-video | y | $0.19 / $0.32 / $0.33 | - [x] added ✅ 2026-09-11 |
| minimax | `MiniMax-H3`, `MiniMax-H3-Max` | MiniMax's current video models; Hailuo is now "legacy" | n (V2 API) | $0.05–0.13/s | - [ ] add later |
| recraft | `recraftv4_1_vector`, `recraftv4_1_pro_vector`, `recraftv2_vector` | V4.1 SVG output (we only carry V4.1 raster) | y (no `size`) | $0.08 / $0.30 / $0.044 | - [x] added ✅ 2026-09-11 |
| bytedance | `seedance-1-0-pro-fast-251015`, `dreamina-seedance-2-0-mini-260615`, `seedream-4-5-251128`, `seedream-5-0-lite-260128` | the successors, plus Seedance 2.0 mini | y / partial (2.0 reference roles) | $1 per 1M tokens; $3.5 per 1M; $0.04; $0.035 per image | - [x] added ✅ 2026-09-11 |
| google | `gemini-omni-1.1-flash` | Google's default video model (GA 2026-08-27) | n (Interactions API) | ≈$0.10/s at 720p | - [ ] deferred 2026-09-11: needs a core change (see note below) |
| hunyuan | aiart `SubmitTextToImageJob` | Hunyuan Image 3.0: newer, and 0.2 CNY vs 0.5 CNY | n (aiart service) | 0.2 CNY/image | - [ ] add or replace hunyuan-image |
| bfl | `flux-2-max`, `flux-2-klein-9b` | FLUX.2 top tier and fast tier | y (same schema as flux-2-pro) | from $0.07/MP; from $0.015 | - [x] added ✅ 2026-09-11 |
| stability | `sd3.5-medium` | SD 3.5 Medium on the sd3 route | y (`resolve_model` already routes it) | $0.035 | - [x] added ✅ 2026-09-11 |
| replicate | `black-forest-labs/flux-1.1-pro` | replaces `replicate/flux-pro`: BFL retired FLUX.1 [pro] on its own API | n (one `resolve_model_version` line) | $0.04/image | - [x] added ✅ 2026-09-11 (flux-pro kept) |
| fal | `fal-ai/flux-2` (FLUX.2 [dev]) | half the price of `fal/flux-dev`; BFL hosts no [dev] | n (one `resolve_endpoint` line; unknown ids fall back to flux/dev) | $0.012/MP | - [x] added ✅ 2026-09-11 |
| fal | `ltx-2.3/{text,image}-to-video` or `ltx-video-13b-distilled` | successors for `fal/video`'s LTX endpoints | n (new endpoint specs; LTX-2.3 takes 6/8/10 s) | $0.08/s at 1080p; $0.04/video | - [ ] replace fal/video |

**Why Gemini Omni is deferred (investigated 2026-09-11).** Auth is not the
problem: the Interactions API takes our `x-goog-api-key`. Neither way it
returns the video fits the proxy today.

1. **The synchronous call** (Google's documented path) blocks until the
   video is done, which can take minutes, and 4K takes longer.
   - The router wraps every video `generate()` in a hard 120 s timeout and
     retries a timeout twice (`proxy/router.rs` ~L476-503). Each retry is a
     new generation that Google bills.
   - With `delivery: "uri"` the result is a gated Files link, which would
     still need re-hosting.
2. **Background mode plus polling** returns the video as inline base64 on
   every GET, but the router (`get_video_status`) and poller forward only
   `video_url`. `VideoGenerationPollResult.video_data` is silently dropped;
   Sora's bytes are dropped the same way today. Google's background-execution
   page doesn't list Omni either, and inline delivery above about 4 MB
   (>720p) may hit payload limits.

The unblock is one of two core changes:

- **Option A:** re-host `video_data` through `ImageStore` in
  `get_video_status` and the poller, the way `build_image_results` stores
  images. This also fixes Sora.
- **Option B:** give video submits a longer, non-retrying timeout per model.

Request shape, for whoever builds it:
- `model`, and `input` as a string or `{type: image | text}` parts.
- `response_format` of `{type: video}` with `aspect_ratio` (16:9 or 9:16),
  `resolution` (360p, 720p, 1080p or 4k), `duration` (`"Ns"`, 3–10) and
  `delivery`.
- Status lives in `in_progress | queued | completed | failed | …`.
- Sources: https://ai.google.dev/gemini-api/docs/omni and
  https://ai.google.dev/static/api/interactions.openapi.json.

## 4. Price drift on carried models

| model | ours | vendor | source | |
|---|---|---|---|---|
| runway/gen4.5 | $0.50 | $0.60 per 5 s | docs.dev.runwayml.com/guides/pricing.md | - [x] updated ✅ 2026-09-11 |
| openai/gpt-image-2 | $0.04 | $0.053 (medium, 1024²) | developers.openai.com image guide | - [x] updated ✅ 2026-09-11 |
| minimax/image-01 | $0.01 | $0.0035 | platform.minimax.io pricing-paygo | - [x] updated ✅ 2026-09-11 |
| minimax/MiniMax-Hailuo-02, -2.3 | $0.43 / $0.49 | $0.28 (768P 6 s) | same | - [x] updated ✅ 2026-09-11 |
| google/gemini-3.1-flash-image | $0.02 | ~$0.067 (1K) | ai.google.dev/gemini-api/docs/pricing | - [x] updated ✅ 2026-09-11 |
| google/gemini-3.1-flash-lite-image | $0.01 | ~$0.034 | same | - [x] updated ✅ 2026-09-11 |
| google/gemini-3-pro-image | $0.05 | ~$0.134 | same | - [x] updated ✅ 2026-09-11 |
| kling (all carried) | $0.035–1.00 | $0.014–0.098/s | kling.ai pricing | moot after §1 |
| hunyuan/hunyuan-image | $0.04 | 0.5 CNY (≈$0.07) | cloud.tencent.com/document/product/1729/105925 | - [x] updated ✅ 2026-09-11 |
| recraft/recraftv4_1, _pro | $0.04 / $0.25 | $0.035 / $0.21 | recraft.ai/docs/api-reference/pricing.md | - [x] updated ✅ 2026-09-11 |
| vidu/viduq1 | $0.30 | $0.40 | platform.vidu.com/docs/pricing.md | - [x] updated ✅ 2026-09-11 |
| pixverse/v3.5 | $0.30 | $0.45 | docs.platform.pixverse.ai pricing | - [x] updated ✅ 2026-09-11 |
| bytedance/seedance-1-0-pro | $0.50 | $0.61 (1080p 5 s) | BytePlus 1544106 | - [x] updated ✅ 2026-09-11 |
| ideogram (character reference) | flat | $0.10 / $0.15 / $0.20 with a character ref | ideogram.ai/pricing | - [ ] variable pricing? |
| stability/sdxl | $0.002 | $0.009 (we undercharge 4.5×) | platform.stability.ai/pricing | - [x] updated ✅ 2026-09-11 |
| bfl/flux-2-pro | $0.05 | from $0.03/MP | docs.bfl.ml/quick_start/pricing.md | - [x] updated ✅ 2026-09-11 |
| fal/flux-pro | $0.05 | $0.04/MP | fal.ai/models/fal-ai/flux-pro/v1.1/llms.txt | - [x] updated ✅ 2026-09-11 |
| fal/sd35-medium | $0.025 | $0.02/MP | fal model page | - [x] updated ✅ 2026-09-11 |
| fal/video | $0.20 | $0.02/video (we charge 10×) | fal.ai/models/fal-ai/ltx-video/llms.txt | - [x] updated ✅ 2026-09-11 |
| replicate/sdxl | $0.012 | ~$0.0025/run ($0.005 covers our 100-step max) | replicate.com/stability-ai/stable-diffusion-xl | - [x] updated ✅ 2026-09-11 |
| replicate/video | $0.30 | ~$0.10/run | replicate.com/lucataco/animate-diff | - [x] updated ✅ 2026-09-11 |

## 5. Recommended skips (acknowledge in bulk)

✅ 2026-09-11: every skip below is recorded as an `acknowledged` entry in
`scripts/model-drift/providers/<p>.mjs`. The weekly report now shows only the
add-later items: Gemini Omni, Hunyuan Image 3.0 and its general image-to-video
action, Ideogram v4, Kling v3, Leonardo Veo 3.1 Fast, and fal's LTX successors.

Each skip is recorded in the definition's `acknowledged` list with the reason
given here.

- **runway:** 24 resold third-party models, such as Veo 3.1, Seedance 2,
  GPT Image 2, Gemini image, Hailuo 3, Wan 3 and Grok Imagine. We carry most
  of them direct.
  - The Runway adapter also maps any unknown id to `gen4_image` or
    `gen4_turbo`.
  - Revisit Grok, Wan and Muse as one batch if we ever want them: Runway is
    our only route to those.
- **openai:** `gpt-image-1.5`, `gpt-image-1-mini` and `chatgpt-image-latest`,
  all shutting down 2026-12-01.
- **minimax:** the `*-01` generation (T2V-01, I2V-01*, image-01-live). They're
  superseded and unpriced.
- **google:** Imagen 4 and Veo 3.0. Deprecated, and past their dates.
- **kling:** `kling-v1`, `v2-master`, `v2-1-master` and `v2-new`. All retire
  9/15.
- **hunyuan:** the Kling and Vidu resale actions (we integrate those vendors
  directly), plus 7 template/app actions.
- **bedrock:** `luma.ray-v2:0` (carried direct) and 13 Stability image
  services, which are edit tools needing an input image.
- **recraft:** the V4 (non-.1) generation, the utility variants, and the
  styles family (needs a `style_id` workflow).
- **ideogram:** `p-image-ideogram` (budget tier, own branch) and 3.0 `FLASH`
  (no gain over Turbo).
- **pixverse:** `v5.5`, `v4`.
- **vidu:** `viduq3-drama` and `viduq3-ad`, which no endpoint accepts. The
  rest are optional.
- **bytedance:** `seedance-1-5-pro`, which deactivates 2026-11-11.
- **leonardo:** 16 platform image UUIDs. Each needs a mapping, and unmapped
  names silently become Lightning XL.
- **fal** (252 endpoints in carried families):
  - FLUX 3 video: decide together with BFL's own `flux-3-video`.
  - FLUX.1 re-hosts, LoRA, control and community variants.
  - FLUX.1 [pro] variants (BFL direct covers them).
  - SDXL and SD 1.5/3/3.5 variants.
  - Recraft (Recraft direct covers it).
  - The other 90-odd LTX endpoints.
  - `/stream` routes are already acknowledged as transport.
- **replicate:** the other FLUX.1 ids and tools, FLUX.2 (BFL direct and fal
  cover it), `flux-3`, the SD 3.5 family (Stability direct), and SD 1.5/2.1.
- **stability:** `sd3.5-flash` until the `model` enum lists it (the docs
  price it, the enum doesn't).
- **bfl:**
  - Skip `flux-2-klein-4b`, a near-duplicate of 9b.
  - Skip `flux-2-flex`: BFL's own price pages conflict.
  - Skip `flux-3-video`: it needs a video adapter.
  - Skip `flux-pro-1.0-fill` and `-expand` (they need image and mask
    inputs) and the `flux-tools/*`.

## 6. Systemic issues found along the way (engineering, not product)

- **The request validator never reads the capability flags.** They only feed
  `/v1/models`, so a model marked image-to-video only still accepts a
  prompt-only request. That request fails at the vendor.
  - This run worked around it per model by making the first-frame input
    required, which the validator does enforce.
  - A central capability gate in the validator is the real fix.
- **Adapters silently substitute unknown ids.** runway → `gen4_image` /
  `gen4_turbo`, leonardo → MOTION2 or Lightning XL, luma → `ray-2`, fal →
  `flux/dev`. A misconfigured model id bills as a different model instead of
  failing.
- **The schema has no discrete duration sets.** Many vendors take "5 or 10 s
  only", but we can only express a 5–10 range. Affected: Kling, Luma
  (5/9 s), PixVerse, MiniMax Hailuo (6/10 s), Veo (4/6/8 s) and vidu2.0.
  - A `values:` list on number params would close a whole class of vendor
    400s.
- **`n > 1` pays for images we never deliver.** The fal (`num_images`) and
  Google (`candidateCount`) adapters forward `n`, but the proxy returns and
  bills one image.
- **Per-megapixel vendor prices are billed flat.** fal FLUX and BFL FLUX.2
  are priced per megapixel, so a 4 MP image costs us 4× its catalog price.
- **Dead code:** the `sd-1.6` arm in `image/stability.rs` targets an API
  discontinued 2025-07-24.
- **Inline video bytes are dropped.** `VideoGenerationPollResult.video_data`
  is never read by `router.rs get_video_status` or the poller, which forward
  only `video_url`. Found 2026-09-11; it affects Sora today.
- **A timed-out video submit is retried as a new billed generation.** The
  router's hard 120 s timeout plus up to 2 retries is harmless for fast
  async submits, but double-bills any submit that runs long. Found
  2026-09-11.
