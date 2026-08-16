# Provider API Conformance Audit — 2026-08-16

Systematic verification that every provider adapter in `litegen-core/src/providers/`
still matches its vendor's **current** published API: endpoint path, auth header,
request field names, response field names, and the model IDs we advertise in
`models/*.yaml`.

Method per row: open the vendor's live documentation URL (the one cited in the
adapter's `@see` doc comment), and match the literal strings the adapter sends
and parses against the doc. Anything that cannot be matched verbatim is recorded.

Legend: ✅ verified · ⚠️ mismatch / needs change · 🔴 broken today · 🆕 new since
our implementation · 📄 doc URL dead or moved

---

## Checklist

| # | Provider | Modality | Status | Doc verified |
|---|----------|----------|--------|--------------|
| 1 | openai | image | 🔴 | ✅ |
| 2 | openai | video | ⚠️ | ✅ |
| 3 | google | image | 🔴 | ✅ |
| 4 | google | video | 🔴 | ✅ |
| 5 | fal | image | ✅ | ✅ |
| 6 | fal | video | 🔴 | ✅ |
| 7 | replicate | image | ✅ | ✅ |
| 8 | replicate | video | ⚠️ | ✅ |
| 9 | runway | image | ⚠️ | ✅ |
| 10 | runway | video | 🔴 | ✅ |
| 11 | luma | image | ✅ | ✅ |
| 12 | luma | video | ⚠️ | ✅ |
| 13 | bfl | image | 🔴 | ✅ |
| 14 | ideogram | image | ✅ | ✅ |
| 15 | recraft | image | ✅ | ✅ |
| 16 | stability | image | 🔴 | ✅ |
| 17 | minimax | image | ✅ | ✅ |
| 18 | minimax | video | ✅ | ✅ |
| 19 | bytedance | image | ✅ | ✅ |
| 20 | bytedance | video | ⚠️ | ✅ |
| 21 | vidu | video | ⚠️ | ✅ |
| 22 | pixverse | video | ⚠️ | ✅ |
| 23 | leonardo | image | ✅ | ✅ |
| 24 | leonardo | video | ⚠️ | ✅ |
| 25 | kling | image | ✅ | ✅ |
| 26 | kling | video | ⚠️ | ✅ |
| 27 | bedrock | image | ✅ | ✅ |
| 28 | bedrock | video | ✅ | ✅ |
| 29 | hunyuan | image | ✅ | ✅ |
| 30 | hunyuan | video | ✅ | ✅ |

> **Remediation status (2026-08-16):** every 🔴 finding and every ⚠️ finding
> that is a spec mismatch has been fixed and covered by a test. See
> [§ Remediation](#remediation) at the end of this document for the
> finding → fix → test mapping. Findings left open (new-model coverage,
> re-hosting auth-gated outputs) are listed there too, with reasons.

All 30 rows were checked against live vendor documentation. Nine vendors publish
a machine-readable spec, which is what the strongest findings below rest on:
OpenAI (`openai/openai-openapi`), Runway (`docs.dev.runwayml.com/openapi.json`),
BFL (`api.bfl.ai/openapi.json`), Ideogram (`developer.ideogram.ai/openapi.json`),
Stability (`api.stability.ai/v2alpha/openapi`), fal (per-endpoint OpenAPI),
MiniMax and Vidu (OpenAPI embedded in their `.md` docs), and Google (via the
`googleapis/python-genai` converters).

---

## Broken right now 🔴

Six provider·modality pairs send a model identifier the vendor has already
retired. These are not "will break" — the shutdown dates are in the past.

| Provider | What we send | Retired | Replacement |
|---|---|---|---|
| openai image | `dall-e-3`, `dall-e-2` — the *only* two models we ship | **2026-05-12** | `gpt-image-2` / `gpt-image-1` |
| google image | `imagen-3.0-generate-002` | **2025-11-10** | Imagen 4 → itself dead 2026-08-17 |
| google image | `gemini-3-pro-image-preview` | **2026-06-25** | `gemini-3-pro-image` (drop the suffix) |
| google video | `veo-2.0-generate-001`, `veo-3.0-generate-001`, `veo-3.0-fast-generate-001` | **2026-06-30** | `veo-3.1-*-preview` |
| runway video | `gen3a_turbo` — returned for *every* model id | **2026-07-30** | `gen4_turbo` / `gen4.5` |
| stability image | `sd3-large`, `sd3-turbo` | replaced | `sd3.5-large`, `sd3.5-large-turbo` |
| bfl image | `bfl/flux-pro` → `POST /v1/flux-pro` | endpoint gone | `flux-2-pro` |
| replicate video | `stability-ai/stable-video-diffusion` | page 404s | — |

Plus two wrong-shape bugs that break a declared capability:

- **google video** sends keyframes as `{"inlineData": {...}}`; the
  `:predictLongRunning` surface wants `{"bytesBase64Encoded": …, "mimeType": …}`.
  All Veo image-to-video and last-frame requests are affected (§4).
- **fal video** resolves the only catalog video model to `fal-ai/ltx-video`,
  which has no `image_url` field at all — image-to-video is impossible (§6).

## Shutting down soon ⏳

| Date | What |
|---|---|
| **2026-08-17** (tomorrow) | Google Imagen 4 — the entire Imagen surface |
| **2026-09-24** | OpenAI **Videos API** + every Sora 2 model. No replacement offered. |
| **2026-10-02** | `gemini-2.5-flash-image` → `gemini-3.1-flash-image` |
| **2026-12-01** | `gpt-image-1-mini`, `gpt-image-1.5`, `chatgpt-image-latest` → `gpt-image-2` |

## Clean bills of health ✅

Nine rows matched their vendor docs with no correctness defect:
**bedrock** image + video (line-for-line against the AWS Nova docs),
**hunyuan** image + video (action names confirmed against the official Tencent
Cloud Node SDK), **minimax** image + video (every enum identical),
**ideogram**, **recraft**, and **replicate** image.

Three deserve specific credit for getting a subtle thing right: Recraft's
V2/V3-only gating of `style`, Luma video sending `duration` as the string
`"5s"` (the exact mistake we make on Sora), and Kling's `video-` id prefix to
keep image and video entries distinct when they share a `model_name`.

## Recurring defect patterns

Worth fixing as classes rather than one at a time:

1. **`aspect_ratio_from_size()` emits ratios outside the vendor's enum.** Google
   (`5:4`, `4:5`), Runway image (`1808:1152`, `1152:1808`), Stability
   (`4:3`, `3:4`). Three independent copies of the same helper, each with at
   least one unsupported output.
2. **`_ =>` fallbacks that silently resolve to a wrong model.** OpenAI image
   falls back to the dead `dall-e-2`; Runway video returns the dead
   `gen3a_turbo` for *everything*; fal video falls back to a text-only endpoint.
   A fallback that guesses is worse than an error.
3. **Catalog advertises a capability the adapter never implements.** OpenAI
   `dall-e-2` inpainting, Stability sd3 image-to-image (no `mode` field sent),
   Vidu `vidu2.0`/`viduq2-pro` text-to-video, fal `fal/video` image-to-video,
   Hunyuan `hunyuan-video` text-to-video (the adapter only ever calls
   `SubmitImageToVideoJob`).

   *(An earlier draft also listed Recraft image-to-image and Leonardo video
   text-to-video here. Both were wrong. `models/recraft.yaml` declares
   `capabilities: { text_to_image: true }` on all five entries and never claims
   image-to-image; all three `models/leonardo.yaml` video entries declare
   `capabilities: { image_to_video: true }` only. Recraft's unused
   `imageToImage` endpoint and Leonardo's image-only video surface are missing
   features, not false claims.)*
4. **Declared params silently dropped.** Stability's `style` never becomes
   `style_preset`; Recraft never sends a seed.
5. **Auth-gated outputs handed to the caller as bare URLs.** Veo's `video.uri`
   and Bedrock's `s3Uri`. The Sora adapter solves this correctly by downloading
   the bytes — the other two should follow it.

## Biggest gaps vs. what vendors now offer

- **OpenAI**: the entire `gpt-image-*` family, `/v1/images/edits`.
- **Google**: `gemini-3.1-flash-image` / `-lite`, the new `/v1beta/interactions`
  API, Veo 3.1 `referenceImages`, `gemini-omni-flash`.
- **BFL**: FLUX.2 (5 models) and **FLUX 3 video** — BFL is a video vendor now.
- **Runway / Leonardo**: both have become multi-vendor routers (GPT Image 2,
  Gemini 3 Pro Image, Seedream 5, Kling 3.0, Veo 3.1, Hailuo 3, Seedance 2.5…).
  We expose 2 of Runway's 9 image models and 0 of its 12 video models.
- **Ideogram**: 4.0 is out; note it renames `prompt` → `text_prompt`.
- **Vidu**: the whole ViduQ3 series.
- **PixVerse**: C1 and V6; everything we ship is now labelled "legacy".

---

## 1. OpenAI — image 🔴

Adapter: `litegen-core/src/providers/image/openai.rs` · Catalog: `models/openai.yaml`
Docs: <https://developers.openai.com/api/docs/deprecations> ·
OpenAPI: `openai/openai-openapi@master` `CreateImageRequest`

### 🔴 Both advertised models are shut down

`models/openai.yaml` ships exactly two image models: `openai/dall-e-3` and
`openai/dall-e-2`. The OpenAI deprecations page lists:

> `dall-e-2`, `dall-e-3` — **shutdown date 2026-05-12** — replacement
> "gpt-image-2, gpt-image-1, or gpt-image-1-mini"

That date is three months in the past. Every OpenAI image generation we serve
is calling a model that no longer exists.

`resolve_model_name()` (`image/openai.rs:100-115`) also *falls back* to
`"dall-e-2"` for any unrecognised id, so adding a catalog entry alone will not
fix it — an unknown native id is silently rewritten to the dead model.

### 🆕 Current model line-up (verified against `CreateImageRequest`)

`gpt-image-2` (current flagship; arbitrary `WIDTHxHEIGHT` sizes, both dims
divisible by 16, AR between 1:3 and 3:1, max `3840x2160`), `gpt-image-1`,
`gpt-image-1-mini`, `gpt-image-1.5`, `chatgpt-image-latest`.
Note the second wave: `gpt-image-1-mini`, `gpt-image-1.5` and
`chatgpt-image-latest` are themselves slated for shutdown **2026-12-01** with
`gpt-image-2` as the replacement — so `gpt-image-2` and `gpt-image-1` are the
only two safe targets.

### ⚠️ `response_format` is sent unconditionally

`image/openai.rs:196-209` always puts `response_format` in the body. Spec:

> "This parameter isn't supported for the GPT image models, which always return
> base64-encoded images."

Sending it to a `gpt-image-*` model is an unknown-parameter error. It must be
gated to the (now-dead) DALL·E models, or dropped entirely.

### ⚠️ `quality` enum is DALL·E-only

Catalog `quality: [standard, hd]`. Spec enum is
`standard | hd | low | medium | high | auto`, where `hd`/`standard` are
dall-e-3-only and `low|medium|high|auto` are the GPT image values. `style`
(`vivid|natural`) is likewise dall-e-3-only.

### ⚠️ GPT-image-only parameters are entirely unimplemented

`background` (`transparent|opaque|auto`), `output_format` (`png|jpeg|webp`),
`output_compression` (0-100), `moderation` (`low|auto`), `stream`,
`partial_images`. None appear in the adapter or in `extra_allowlist`.

### ⚠️ Catalog advertises an unimplemented capability

`openai/dall-e-2` declares `capabilities: { image_to_image: true, inpainting: true }`
plus a multipart `ref_inputs` mapping (`init → image`, `mask → mask`), but the
adapter never implements it — `image/openai.rs:221-223` says so in a comment:

> `// (DALL-E 2 edits use a different endpoint; we do text-to-image only here).`

`grep` confirms no `edits` / `variations` / `multipart` path exists in the file.
A caller who sends a ref image gets a silent text-to-image result. The
replacement is `POST /v1/images/edits`, which the GPT image models also support.

### ✅ Verified correct

- `POST https://api.openai.com/v1/images/generations` — matches spec path.
- `Authorization: Bearer <key>` — matches.
- Request fields `model`, `prompt`, `size`, `n` — all present in
  `CreateImageRequest` with matching types.
- Response parse `data[0].b64_json` / `data[0].url` / `data[0].revised_prompt`
  — all present in the spec's image response object.
- DALL·E 3 size clamping to `1024x1024|1792x1024|1024x1792` and DALL·E 2 to
  `256x256|512x512|1024x1024` matches the spec's per-model size lists exactly.

---

## 2. OpenAI — video (Sora) ⚠️

Adapter: `litegen-core/src/providers/video/openai.rs` · Catalog: `models/openai.yaml`
Docs: <https://developers.openai.com/api/reference/resources/videos/methods/create>

### ⚠️ Whole API is deprecated — shutdown 2026-09-24

The deprecations page lists the **Videos API** itself plus `sora-2`,
`sora-2-pro`, `sora-2-2025-10-06`, `sora-2-2025-12-08`,
`sora-2-pro-2025-10-06`. **No replacement is offered.** Every operation under
`/videos` carries `deprecated: true` in the OpenAPI spec — including the newer
`/videos/edits`, `/videos/extensions`, `/videos/characters` and
`/videos/{id}/remix` routes.

That is ~5 weeks out from today. Both `openai/sora` and `openai/sora-2-pro`
need an end-of-life plan in the catalog.

### ⚠️ `seconds` is typed as a string, we send a number

`CreateVideoJsonBody.seconds` → `VideoSeconds`, which is:

```yaml
VideoSeconds:
  type: string
  enum: ["4", "8", "12"]
```

`video/openai.rs:199-204` builds the text-to-video body with
`"seconds": seconds` where `seconds: u64` — a JSON number. The multipart
(image-to-video) path is correct because `.text("seconds", seconds.to_string())`
stringifies it. Only the text-to-video path is exposed.

### 🆕 `input_reference` now accepts a JSON object

Spec `CreateVideoJsonBody.input_reference` → `ImageRefParam-2`: "Provide exactly
one of `image_url` or `file_id`." Our adapter only supports the multipart binary
form (`video/openai.rs:181-192`) — `init_bytes` matches on
`MaterializedRefForm::Base64` alone, so a URL-form ref is dropped and the job
runs as text-to-video.

Re-checked: this is **latent, not live**. Both Sora catalog entries set
`provider_format: { form: base64 }`, so the materializer never hands this
adapter a URL-form ref today. It becomes reachable the moment that changes.

### 🆕 Endpoints we do not implement

`POST /videos/edits`, `POST /videos/extensions`, `POST /videos/characters`,
`POST /videos/{video_id}/remix`. All deprecated alongside the rest, so this is
informational only.

### ✅ Verified correct

- `POST {base}/videos`, `GET {base}/videos/{id}`, `GET {base}/videos/{id}/content`
  — all three paths match the spec.
- `model` values `sora-2` / `sora-2-pro` match `VideoModel`.
- `size` values `720x1280 | 1280x720 | 1024x1792 | 1792x1024` match `VideoSize`
  exactly, and the pro-only gating of the 1792 sizes matches the docs.
- Status mapping `queued | in_progress | completed | failed` matches
  `VideoStatus` verbatim.
- Downloading `/content` with the bearer header rather than handing the caller
  a bare URL is correct — it is an auth-gated REST route, not a pre-signed URL.
- 🆕 `sora-2-2025-12-08` is a snapshot added after our implementation; the
  adapter passes unknown ids through unchanged, so it already works.

---

## 3. Google — image 🔴

Adapter: `litegen-core/src/providers/image/google.rs` · Catalog: `models/google.yaml`
Docs: <https://ai.google.dev/gemini-api/docs/deprecations> ·
<https://ai.google.dev/gemini-api/docs/imagen> ·
<https://ai.google.dev/gemini-api/docs/changelog>
Cross-checked against `googleapis/python-genai@main` `types.py` (`ImageConfig`,
`Modality`) for the canonical wire shapes.

### 🔴 `google/imagen-3` points at a model shut down 9 months ago

`resolve_model()` (`image/google.rs:87`) maps `imagen-3` →
`imagen-3.0-generate-002`. The deprecation table:

> `imagen-3.0-generate-002` — released 2025-02-06 — **shutdown 2025-11-10** —
> replacement `imagen-4.0-generate-001`

### 🔴 …and the whole Imagen surface dies tomorrow

`imagen-4.0-generate-001`, `imagen-4.0-ultra-generate-001` and
`imagen-4.0-fast-generate-001` all carry **shutdown 2026-08-17** — the day after
this audit — with `gemini-3.1-flash-image` as the replacement. The Imagen guide
now opens with "This model is deprecated and will be shut down on August 17,
2026; migrate to Nano Banana for image generation."

So repointing `imagen-3` at Imagen 4 buys one day. The `is_imagen_model()` /
`:predict` branch (`image/google.rs:62-75`, `192-224`, `343-373`) becomes dead
code once the catalog entry goes. Its request shape (`instances[].prompt`,
`instances[].negativePrompt`, `parameters.{sampleCount,aspectRatio,seed}`) and
response shape (`predictions[0].{bytesBase64Encoded,mimeType}`) are ✅ correct
for as long as the surface exists.

### 🔴 `gemini-3-pro-image-preview` was shut down 2026-06-25

`resolve_model()` (`image/google.rs:89`) rewrites `gemini-3-pro-image` →
`gemini-3-pro-image-preview`. Per the changelog, `gemini-3-pro-image-preview`
and `gemini-3.1-flash-image-preview` were deprecated 2026-05-28 and **shut down
2026-06-25**; `gemini-3-pro-image` and `gemini-3.1-flash-image` went GA
2026-05-28. The `-preview` suffix we append is the dead id — the catalog id
`gemini-3-pro-image` is already the correct native id and should be passed
through untouched.

### ⚠️ `gemini-2.5-flash-image` is on the clock

Shutdown **2026-10-02** (~7 weeks), replacement `gemini-3.1-flash-image`. It is
currently our default fallback for every unrecognised id (`image/google.rs:91`).

### 🆕 Models missing from the catalog

`gemini-3.1-flash-image` (Nano Banana 2, GA 2026-05-28),
`gemini-3.1-flash-lite-image` (Nano Banana 2 Lite). These are the models
everything else is being migrated *to*.

### ⚠️ `aspect_ratio_from_size()` can emit unsupported ratios

`ImageConfig.aspect_ratio` supported values are exactly
`1:1, 2:3, 3:2, 3:4, 4:3, 9:16, 16:9, 21:9`. `image/google.rs:107-127` maps
ratios in `[1.1, 1.25)` to `"5:4"` and `[0.85, 0.95)` to `"4:5"` — neither is
in the supported set. A caller passing `size: 1200x1000` gets a rejected
request. (`2:3` and `3:2` are supported and we never emit them.)

### ⚠️ `responseModalities: ["image"]` is lowercase

`image/google.rs:262-264` sends `["image"]`. The canonical wire value is the
proto enum name — `Modality.IMAGE = 'IMAGE'` in the SDK, and the docs use
`["TEXT", "IMAGE"]`. Lowercase is at best undocumented behaviour. Worth
normalising to `"IMAGE"`.

### 🆕 `imageConfig` fields we never send

`imageConfig.imageSize` (`1K` | `2K` | `4K`, default `1K`) — this is how
resolution is controlled on `gemini-3-pro-image` and there is no other way to
ask for 2K/4K. Also `imageConfig.personGeneration`
(`ALLOW_ALL|ALLOW_ADULT|ALLOW_NONE`). Note that on the `generateContent` surface
`personGeneration` nests **under `imageConfig`**, not at `generationConfig` top
level, which is where our `extras.extra` shallow-merge would put it
(`image/google.rs:287-293`) if a catalog entry ever allowlisted it.

### 🆕 A whole new API surface: `/v1beta/interactions`

The image-generation guide now documents the **Interactions API**
(`POST https://generativelanguage.googleapis.com/v1beta/interactions`) rather
than `generateContent`, with `response_format: {type, mime_type, aspect_ratio,
image_size}`, `generation_config.thinking_level`, and output at
`interaction.output_image.data`. `generateContent` still works for image models,
but the documented path has moved. Worth a spike before the next model wave.

### ⚠️ Auth is inconsistent with the video adapter

`image/google.rs:306` authenticates with `?key=<api_key>` in the query string;
`video/google.rs:44` uses the `x-goog-api-key` header. Both work, but the docs
and SDK use the header, and query-string keys leak into logs and proxies.

### ✅ Verified correct

- `POST {base}/models/{model}:generateContent` and `:predict` — both paths match.
- `contents[].parts[]` with `text`, `inlineData.{mimeType,data}`, and
  `fileData.{fileUri,mimeType}` — all match `generateContent`.
- `generationConfig.imageConfig.aspectRatio` nesting is correct (a top-level
  `generationConfig.aspectRatio` is ignored, exactly as the code comment says).
- `candidateCount` for `n > 1` is correct — confirmed by commit `e8dd8ef`,
  and `numberOfImages` really is Imagen-`:predict`-only.
- Response parse `candidates[0].content.parts[].inlineData.{data,mimeType}` ✅.

---

## 4. Google — video (Veo) 🔴

Adapter: `litegen-core/src/providers/video/google.rs` · Catalog: `models/google.yaml`
Docs: <https://ai.google.dev/gemini-api/docs/deprecations> ·
`googleapis/python-genai@main` `models.py`
(`_GenerateVideosParameters_to_mldev`, `_GenerateVideosConfig_to_mldev`,
`_Image_to_mldev`) for the exact Developer-API wire body.

### 🔴 Three of six catalog models were shut down 2026-06-30

| Catalog id | Shutdown | Replacement |
|---|---|---|
| `google/veo-2.0-generate-001` | 2026-06-30 | `veo-3.1-generate-preview` |
| `google/veo-3.0-generate-001` | 2026-06-30 | `veo-3.1-generate-preview` |
| `google/veo-3.0-fast-generate-001` | 2026-06-30 | `veo-3.1-fast-generate-preview` |

All three are still advertised. The remaining three
(`veo-3.1-generate-preview`, `veo-3.1-fast-generate-preview`,
`veo-3.1-lite-generate-preview`) are the current line and ✅ resolve correctly —
`resolve_model()` just strips the `google/` prefix, so they pass through intact.

### 🔴 Image-to-video sends the wrong field shape

`video/google.rs:120-129` builds keyframes as:

```json
{ "inlineData": { "mimeType": "image/png", "data": "<b64>" } }
```

`_Image_to_mldev` in the SDK emits, for the Gemini Developer API:

```json
{ "bytesBase64Encoded": "<b64>", "mimeType": "image/png" }
```

`inlineData` is the `generateContent` shape, not the `:predictLongRunning`
`instances[]` shape. This affects **both** `instances[0].image` (first frame)
and `instances[0].lastFrame`. Every Veo image-to-video and last-frame request we
send is therefore either rejected or silently degraded to text-to-video — the
catalog advertises `image_to_video: true` on all six Veo entries.

### 🆕 `instances[0].referenceImages` is unimplemented

`_GenerateVideosConfig_to_mldev` maps `reference_images` →
`instances[0].referenceImages`. This is the Veo 3.1 headline feature, and our
own catalog description for `veo-3.1-generate-preview` literally says "adds
reference images" — but `ref_inputs` only declares `first_frame` / `last_frame`.

### 🆕 Other Developer-API parameters we never send

`parameters.sampleCount` (number of videos), `parameters.enhancePrompt`,
`instances[0].video` (video extension), and `webhookConfig` (push instead of
poll). Conversely the SDK confirms `seed`, `generateAudio`, `fps`,
`outputGcsUri`, `mask`, `compressionQuality` are **Vertex-only** and raise on
the Developer API — we correctly never send them.

### 🆕 `gemini-omni-flash-preview`

Released 2026-06-30 for "high-speed video generation and conversational video
editing" — 3-10s 720p, served through the Interactions API, not
`:predictLongRunning`. Not in the catalog.

### ⚠️ Returned `video.uri` is still auth-gated

Already documented as a known follow-up in the adapter's own header comment
(shared with Bedrock Nova Reel). Flagging that it is still open: unlike the Sora
adapter — which downloads `/content` with the bearer header and returns bytes —
Veo hands the caller a `generativelanguage.googleapis.com` URI they cannot fetch
without our API key.

### ✅ Verified correct

- `POST {base}/models/{model}:predictLongRunning` — matches the documented curl.
- `x-goog-api-key` header — matches the documented curl verbatim.
- Body envelope `{"instances":[...],"parameters":{...}}` ✅.
- `instances[0].prompt` ✅; `parameters.aspectRatio`, `parameters.durationSeconds`,
  `parameters.resolution`, `parameters.negativePrompt`, `parameters.personGeneration`
  all match `_GenerateVideosConfig_to_mldev` exactly.
- Poll `GET {base}/{operation_name}`, completion on `done`, and the output path
  `response.generateVideoResponse.generatedSamples[0].video.uri` ✅.

---

## 5. Fal — image ✅

Adapter: `litegen-core/src/providers/image/fal.rs` · Catalog: `models/fal.yaml`
Verified against fal's per-endpoint OpenAPI, which is the authoritative input
schema: `https://fal.ai/api/openapi/queue/openapi.json?endpoint_id={endpoint}`.

All seven mapped endpoints resolve (HTTP 200 on their OpenAPI documents):
`fal-ai/flux/dev`, `fal-ai/flux/schnell`, `fal-ai/flux-pro/v1.1`,
`fal-ai/fast-sdxl`, `fal-ai/stable-diffusion-v35-medium`, `fal-ai/recraft-v3`,
`fal-ai/aura-flow`. (A bogus endpoint id returns 404, so 200 is a real signal.)

### ✅ Verified correct

- `POST https://fal.run/{endpoint}` sync surface, `Authorization: Key <token>` ✅.
- `image_size: {width, height}` matches the `ImageSize` schema verbatim
  (`width`/`height` integers, exclusive-min 0, max 14142).
- `num_images`, `seed`, `guidance_scale`, `num_inference_steps` all appear in
  `FluxDevInput` with matching names.
- Response parse `images[].url` matches `FluxDevOutput.images` → `Image.url`.

### 🆕 `image_size` also accepts named presets

`anyOf: [ImageSize, enum: square_hd | square | portrait_4_3 | portrait_16_9 |
landscape_4_3 | landscape_16_9]`, default `landscape_4_3`. We only ever send the
object form, so a caller asking for an aspect ratio without a pixel size gets
fal's default rather than their ratio.

### 🆕 Endpoint-level fields we never send

`output_format` (png/jpeg), `acceleration`, `enable_safety_checker`, and
`sync_mode`. `sync_mode: true` returns the image inline instead of a CDN URL,
which would remove the second HTTPS GET in `fetch_image_bytes`.

### ⚠️ Unconditional fields not in every endpoint's schema

`generate()` adds `negative_prompt`, `strength`, `style` and `image_url`
whenever the corresponding extra is set, but `FluxDevInput` has none of them.
Today this is latent rather than live — `models/fal.yaml` declares only
`seed` / `guidance_scale` / `steps` / `size` for the Flux entries, so those
fields never populate. It becomes a real bug the moment a catalog entry widens
its params.

*(Correction: an earlier draft said "every entry has `extra_allowlist: []`".
That is wrong — `fal/flux-pro` allows `safety_tolerance` and `output_format`.
Both are genuine `fal-ai/flux-pro/v1.1` input fields, so the conclusion holds,
but the blanket statement did not.)*

### 🆕 Newer fal endpoints absent from the catalog

Confirmed live: `fal-ai/flux-2-pro`, `fal-ai/nano-banana`,
`fal-ai/bytedance/seedream/v4/text-to-image`. Our newest Flux mapping is
`flux-pro/v1.1`.

---

## 6. Fal — video 🔴

Adapter: `litegen-core/src/providers/video/fal.rs` · Catalog: `models/fal.yaml`

### 🔴 The only catalog video model cannot do image-to-video

`models/fal.yaml` exposes exactly one video model, `fal/video`, declaring
`capabilities: { text_to_video: true, image_to_video: true }` with a
`ref_inputs` `init` role in `url` form. `resolve_endpoint()` has no case for
`fal/video`, so it hits the `_ =>` fallback (`video/fal.rs:86`) and resolves to
**`fal-ai/ltx-video`**. That endpoint's schema is:

```
LtxVideoInput: [guidance_scale, seed, num_inference_steps, negative_prompt, prompt]
```

There is no `image_url` field. The image-to-video variant is a *different*
endpoint — `fal-ai/ltx-video/image-to-video`, whose schema does contain
`image_url` (verified, HTTP 200). So every `fal/video` request with a reference
image silently produces a text-to-video result.

### ⚠️ `duration` and `aspect_ratio` are not accepted by most mapped endpoints

`generate()` (`video/fal.rs:137-144`) always sends `duration` and
`aspect_ratio`. Checking each endpoint's schema:

| Endpoint | `duration` | `aspect_ratio` | `prompt` |
|---|---|---|---|
| `fal-ai/ltx-video` | ✗ | ✗ | ✓ |
| `fal-ai/kling-video/v1/standard/image-to-video` | ✓ | ✗ | ✓ |
| `fal-ai/stable-video` | ✗ | ✗ | **✗** |
| `fal-ai/fast-animatediff/turbo/text-to-video` | ✗ | ✗ | ✓ |

`fal-ai/stable-video` takes `[cond_aug, fps, seed, motion_bucket_id, image_url]`
— it has no `prompt` at all, yet `prompt` is the one field we send
unconditionally. The catalog's `duration_seconds: {min: 2, max: 10}` on
`fal/video` is therefore decorative.

`fast-animatediff` wants `num_frames` + `video_size` + `fps`, none of which we
send.

### ⚠️ Endpoint map is two generations behind

`kling-video/v1` and `v1.5`, `minimax-video` / `video-01-live`,
`fast-animatediff`, `stable-video`, `ltx-video`. All still resolve, but the
current equivalents are live and unmapped — confirmed 200:
`fal-ai/kling-video/v2.5-turbo/pro/image-to-video`,
`fal-ai/minimax/hailuo-02/standard/text-to-video`, `fal-ai/veo3`.

### ✅ Verified correct

- Queue submit `POST https://queue.fal.run/{endpoint}`, status
  `GET .../requests/{id}/status`, result `GET .../requests/{id}` — all three
  match the paths in each endpoint's own OpenAPI document.
- `Authorization: Key <token>` ✅.
- Submit response fields `request_id`, `status_url`, `response_url` match
  `QueueStatus` (which also offers `cancel_url`, `queue_position`, `logs`,
  `metrics` — all unused).
- Status enum `IN_QUEUE | IN_PROGRESS | COMPLETED` matches `QueueStatus.status`
  verbatim. (We also branch on `"FAILED"`, which is not in the enum — fal signals
  failure with an HTTP error status. Harmless, but dead.)
- Result parse `video.url` matches the `File` ref on `LtxVideoOutput.video` ✅.

---

## 7. Replicate — image ✅

Adapter: `litegen-core/src/providers/image/replicate.rs` · Catalog: `models/replicate.yaml`
Docs: <https://replicate.com/docs/reference/http>

### ✅ Verified correct

- Versioned create `POST /v1/predictions` with `{version, input}` ✅.
- Official-model create `POST /v1/models/{owner}/{name}/predictions` with
  `{input}` and no `version` ✅ — both paths are still current.
- `Authorization: Bearer <token>` ✅ — this is what the docs now show (the older
  `Token <token>` form is legacy; we already use the current one).
- Poll `GET /v1/predictions/{id}`, statuses
  `starting | processing | succeeded | failed | canceled` ✅ all five match.
- Health check `GET /v1/account` ✅.
- Every referenced model slug resolves (HTTP 200 on `replicate.com/{slug}`):
  `black-forest-labs/flux-pro`, `flux-dev`, `flux-schnell`, `stability-ai/sdxl`,
  `stability-ai/stable-diffusion-3`, `stability-ai/stable-diffusion`.
- The pinned `stability-ai/stable-diffusion` hash `ac732df83cea…` matches the
  "Version 2.1" hash the model page currently advertises.

### 🆕 `Prefer: wait` synchronous mode

`Prefer: wait` (or `Prefer: wait=<seconds>`, default 60) makes create block and
return the finished prediction. We create-then-poll on a loop
(`image/replicate.rs:111-150`); for fast image models this header removes the
whole polling round-trip.

### 🆕 `version` now accepts `owner/model` and `owner/model:version`

The create-a-prediction reference documents `version` as accepting
`owner/model`, `owner/model:version_id` **or** a bare `version_id`. Our
two-path split in `prediction_url()` still works but is no longer required.

### ⚠️ `stream` is deprecated

Marked deprecated in the request body reference. We never send it — noted only
so it does not get added.

### 🆕 Current-generation official models absent from the catalog

Confirmed live: `google/nano-banana`, `bytedance/seedream-4`,
`black-forest-labs/flux-1.1-pro`, `openai/gpt-image-1`. Our newest is
`black-forest-labs/flux-pro`.

---

## 8. Replicate — video ⚠️

Adapter: `litegen-core/src/providers/video/replicate.rs` · Catalog: `models/replicate.yaml`

### 🔴 `stability-ai/stable-video-diffusion` has been removed from Replicate

`replicate.com/stability-ai/stable-video-diffusion` returns **HTTP 404**. Two
entries in `resolve_model_version()` point at it — `replicate/svd` and
`replicate/svd-xt` (`video/replicate.rs:69-88`), both pinned to version
`3f0457e4619daac…`. Every other referenced slug resolves 200.

Mitigating: `models/replicate.yaml` exposes only one video id, `replicate/video`,
which falls through to `lucataco/animate-diff`. The dead entries are reachable
only via an explicit `model_mapping` override — but they are still wrong.

### ⚠️ `replicate/modelscope` does not map to ModelScope

`video/replicate.rs:99-106` labels the entry "ModelScope — text-to-video" and
maps it to `deforum/deforum_stable_diffusion`, which is Deforum, a different
model. The slug resolves (200), so this is a naming defect, not an outage.

### ⚠️ The whole video registry is 2023-era

AnimateDiff, SVD, Zeroscope v2 XL, Deforum. The default for `replicate/video` is
AnimateDiff. Current official video models on Replicate — all confirmed 200 —
include `minimax/hailuo-02`, `kwaivgi/kling-v2.1` and `google/veo-3`, none of
which we map.

### ✅ Verified correct

- Create `POST /v1/predictions` with `{version, input}`, `Authorization: Bearer`,
  poll `GET /v1/predictions/{id}` — same verified surface as the image adapter.
- Status handling matches the documented enum.

---

## 9. Runway — image ⚠️

Adapter: `litegen-core/src/providers/image/runway.rs` · Catalog: `models/runway.yaml`
Verified against Runway's published OpenAPI: <https://docs.dev.runwayml.com/openapi.json>
(the `/v1/text_to_image` request body is a 9-variant discriminated union, one
variant per model, so the per-model field lists below are exact).

### ⚠️ Two advertised aspect ratios produce invalid `ratio` values

The `ratio` enum for both `gen4_image` and `gen4_image_turbo` is exactly:

```
1024:1024  1080:1080  1168:880   1360:768  1440:1080  1080:1440
1808:768   1920:1080  1080:1920  2112:912  1280:720   720:1280
720:720    960:720    720:960    1680:720
```

`resolve_ratio()` (`image/runway.rs:84-91`) maps `"3:2"` → **`1808:1152`** and
`"2:3"` → **`1152:1808`**. Neither string is in the enum. Both `3:2` and `2:3`
are listed in `allowed` for `runway/gen4_image` and `runway/gen4_image_turbo` in
`models/runway.yaml`, so a caller can select them and get a 400. The enum has no
true 3:2 pair at all — the fix is to drop them from `allowed`.

### ⚠️ `seed` upper bound is off by one

Spec: `seed` maximum `4294967295`. Catalog: `max: 4294967294`. Cosmetic, but it
silently rejects one legal value.

### 🆕 Seven newer text-to-image models on the same endpoint

`/v1/text_to_image` now accepts `gpt_image_2`, `gemini_image3_pro`,
`gemini_image3.1_flash`, `seedream5_pro`, `seedream5_lite`,
`grok_imagine_image_2`, `gemini_2.5_flash` in addition to our two. Several carry
fields we have no plumbing for — `quality`, `background`, `outputCount`,
`outputFormat`, `grounding`, `edit`. Runway has effectively become a
multi-vendor router.

### 🆕 New generation surface `/v1/generate/image`

A `configId` + `input` shape backed by the Model Router (changelog 2026-07-23).
Not used.

### ✅ Verified correct

- `POST {base}/text_to_image`, base `https://api.dev.runwayml.com/v1` ✅.
- `X-Runway-Version: 2024-11-06` — the spec declares this header **required**
  with `const: "2024-11-06"`, so our pinned value is exactly right.
- `Authorization: Bearer` ✅.
- Body fields `promptText`, `model`, `ratio`, `seed`, and
  `referenceImages[].{uri,tag}` all match the `gen4_image` variant verbatim;
  `maxItems: 3` matches our `ref_inputs.max_total: 3`.
- `gen4_image_turbo` lists `referenceImages` as **required** with `minItems: 1`,
  and the catalog correctly sets `init.min_count: 1` for that model only.
- Task polling `GET /v1/tasks/{id}` and the `SUCCEEDED`/`FAILED`/`CANCELLED`
  branches match the response union's status constants.

---

## 10. Runway — video 🔴

Adapter: `litegen-core/src/providers/video/runway.rs` · Catalog: `models/runway.yaml`

### 🔴 We send a model identifier Runway removed on 2026-07-30

`resolve_model()` (`video/runway.rs:57-62`) returns `"gen3a_turbo"` for
`runway/gen-3`, for `runway/gen-3-turbo`, **and** for the `_ =>` fallback — every
possible input. The API changelog for 2026-07-30 reads:

> Gen-3 Alpha Turbo (`gen3a_turbo`) and Gen-4 Aleph (`gen4_aleph`) are no longer
> available via the Runway API. Requests that use these model identifiers will
> fail.

Confirmed structurally: `gen3a_turbo` does not appear in the `/v1/image_to_video`
model union, whose 12 variants are `gen4.5`, `gen4_turbo`, `veo3.1`,
`veo3.1_fast`, `hailuo3`, `happyhorse_1_0`, `seedance2`, `seedance2_fast`,
`seedance2_mini`, `gemini_omni_flash`, `seedance2_5`, `grok_imagine_1_5`.
The whole Runway video provider is dead. Replacements: `gen4.5` (quality) or
`gen4_turbo` (speed).

### 🔴 The advertised `ratio` values are also gone

Both catalog entries declare `allowed: ["1280:768","768:1280"]` with default
`1280:768`, and `video/runway.rs:148` hardcodes `1280:768` as the fallback.
The `ratio` enum for `gen4_turbo` / `gen4.5` is:

```
1280:720  720:1280  1104:832  832:1104  960:960  1584:672
```

`1280:768` and `768:1280` are gen3-era values and appear nowhere. So even after
swapping the model id, every request still fails until the ratios are updated
(`1280:768` → `1280:720`, `768:1280` → `720:1280`).

### ✅ Correction: endpoint selection is already right

An earlier draft of this audit claimed the adapter only ever posts to
`/v1/image_to_video`. That is wrong — `video/runway.rs:121-128` picks
`image_to_video` when an `init` ref is present and `text_to_video` otherwise,
which matches Runway's split. Both endpoints exist in the spec and both accept
`gen4_turbo`/`gen4.5`. Note `promptImage` is in `image_to_video`'s `required`
list for `gen4_turbo`, so the branch is load-bearing, not cosmetic.

### ⚠️ `CANCELLED` is not handled

`video/runway.rs:255-258` maps `SUCCEEDED`/`FAILED`/`RUNNING`/`THROTTLED` and
falls through to `Pending`. The task response union's status constants are
`PENDING | THROTTLED | CANCELLED | RUNNING | FAILED | SUCCEEDED` — a cancelled
task is reported as pending and polls until timeout. (The image adapter gets
this right.)

### 🆕 `duration` bounds

Spec: integer 2-10 for `gen4_turbo`/`gen4.5`. Catalog: `min: 5, max: 10`. We
under-advertise the 2-4s range.

### ✅ Verified correct

- `POST {base}/image_to_video`, `GET {base}/tasks/{id}` ✅.
- `X-Runway-Version: 2024-11-06` ✅ (matches the spec's `const`).
- `Authorization: Bearer` ✅.
- Field names `promptText`, `promptImage`, `ratio`, `duration`, `seed` are all
  correct for the current models — only their *values* are stale.
- `promptImage` accepting an HTTPS URL or a `data:image/png;base64,…` URI
  matches the spec's `anyOf` ✅.

---

## 11. Luma — image ✅

Adapter: `litegen-core/src/providers/image/luma.rs` · Catalog: `models/luma.yaml`
Docs: <https://docs.lumalabs.ai/reference/generateimage.md> (Luma publishes the
raw OpenAPI inline in its `.md` reference pages).

### ✅ Verified correct

- `POST https://api.lumalabs.ai/dream-machine/v1/generations/image` ✅.
- `Authorization: Bearer <key>` ✅.
- `ImageModel` enum is exactly `photon-1 | photon-flash-1`, with default
  `photon-1` — our catalog ships precisely those two ids and `resolve_model()`
  falls back to `photon-1`, matching the documented default.
- Ref roles map onto the documented reference fields: `image_ref`, `style_ref`,
  `character_ref`, `modify_image_ref` ✅.
- Poll on `completed` / `failed` ✅.

### 🆕 Endpoints not implemented

`POST /generations/image/reframe` (reframe an image) and `GET /credits`.

---

## 12. Luma — video ⚠️

Adapter: `litegen-core/src/providers/video/luma.rs` · Catalog: `models/luma.yaml`
Docs: <https://docs.lumalabs.ai/reference/creategeneration.md> ·
<https://docs.lumalabs.ai/docs/video-generation.md>

### ⚠️ Two catalog models are not in the documented `VideoModel` enum

The spec embedded in the reference page is unambiguous:

```json
"VideoModel": { "type": "string", "enum": ["ray-2", "ray-flash-2"], "default": "ray-2" }
```

and the Models table in the video guide lists only:

| name | model param |
|---|---|
| Ray 2 Flash | `ray-flash-2` |
| Ray 2 | `ray-2` |

`models/luma.yaml` ships `luma/ray-3` and `luma/ray-hdr-3`, and
`resolve_model()` (`video/luma.rs:66-69`) also knows `ray-3-14` and
`ray-hdr-3-14`. None appear anywhere in Luma's published API surface. Ray 3
exists as a product, so this may be an undocumented id rather than a dead one —
but it cannot be verified from the docs and needs one live call to settle.
`luma/dream-machine-1.5` → `ray-1-6` is in the same position, and is the more
likely genuine casualty.

### ✅ Verified correct

- `POST {base}/generations`, `GET {base}/generations/{id}` ✅.
- `duration` is sent as a **string** (`format!("{}s", …)`) which matches
  `VideoModelOutputDuration` (`enum: ["5s","9s"]`, string type) — this is the
  exact mistake we make on Sora, and it is done right here.
- `resolution` values `540p | 720p | 1080p | 4k` match
  `VideoModelOutputResolution` ✅.
- `aspect_ratio` enum `1:1, 16:9, 9:16, 4:3, 3:4, 21:9, 9:21` — the catalog's
  allowed lists are a subset ✅.
- `keyframes.frame0` / `keyframes.frame1` with `{type: "image", url}` ✅ matches
  the documented image-to-video and start/end-frame examples.
- `loop`, `concepts`, `callback_url` are all real fields (unused, allowlist-able).

### 🆕 Endpoints not implemented

`POST /generations/{id}/upscale` (up to 4K), `POST /generations/{id}/audio`
(add audio), `POST /generations/video/reframe`, `POST /generations/video/modify`
(style transfer / prompt editing), `GET /generations` (list), `GET /concepts`.

---

## 13. BFL — image 🔴

Adapter: `litegen-core/src/providers/image/bfl.rs` · Catalog: `models/bfl.yaml`
Verified against BFL's live OpenAPI: <https://api.bfl.ai/openapi.json>.

### 🔴 `bfl/flux-pro` has no endpoint

`resolve_model()` strips the `bfl/` prefix and uses the remainder as the path
segment, so `bfl/flux-pro` posts to `POST /v1/flux-pro`. That path is **absent**
from the OpenAPI document. The FLUX.1 [pro] generate route is gone; what remains
of the 1.0 family is only `/v1/flux-pro-1.0-fill`, `/v1/flux-pro-1.0-expand` and
their `-finetuned` variants. The docs index has no FLUX.1 [pro] generation page
either.

### ✅ The other five catalog models are live

`/v1/flux-pro-1.1`, `/v1/flux-dev`, `/v1/flux-pro-1.1-ultra`,
`/v1/flux-kontext-pro`, `/v1/flux-kontext-max` all present.

### ⚠️ Kontext is now positioned as legacy

Every Kontext docs page carries "For new projects, we recommend FLUX.2 with
multi-reference support and up to 4MP output." Not deprecated, but no longer the
recommended path.

### 🆕 Two whole model generations landed since we implemented

**FLUX.2** — `/v1/flux-2-pro` (documented as "the recommended default model for
image editing and generation"), `/v1/flux-2-max`, `/v1/flux-2-flex`,
`/v1/flux-2-klein-9b`, `/v1/flux-2-klein-4b`, plus `-preview` channels. The
`Flux2Inputs` schema supports **eight** reference images
(`input_image` … `input_image_8`) against Kontext's four.

**FLUX 3** — `/v1/flux-3-video`: video generation with synchronised audio, up to
full-HD and 20 seconds, with modes `t2v` / `i2v` / `v2v` / `draft_enhance`. BFL
is a video provider now and we model it as image-only.

**FLUX Tools** — `/v1/flux-tools/outpainting-v1`, `erase-v1`, `deblur-v1`,
`vto-v1`, `vto-v2`, `video-upscale-v1`.

### ⚠️ Latent field mismatches in the ref handling

`generate()` (`image/bfl.rs:188-197`) writes `input_image` for any ref whose role
is not `redux`/`image_prompt`, and for **every** URL-form ref. `input_image`
exists only on the Kontext and FLUX.2 schemas — `FluxPro11Inputs`,
`FluxDevInputs` and `FluxUltraInput` have `image_prompt` and no `input_image`.
Today the catalog constrains `bfl/flux-pro-1.1` to a single `redux` role in
base64 form, so this never fires; it breaks as soon as roles are widened.

### 📄 Docs host has moved

Our `@see` links point at `docs.bfl.ai`; every canonical URL in BFL's own
`llms.txt` is now `docs.bfl.ml`.

### ✅ Verified correct

- `x-key: <api_key>` auth header ✅ (matches the documented curl verbatim).
- Submit response `{id, polling_url}` matches `AsyncResponse` ✅, and preferring
  the returned `polling_url` over a hand-built `/v1/get_result?id=` is exactly
  what the integration guide prescribes.
- `GET /v1/get_result?id=` fallback matches the documented utility endpoint ✅.
- Result parse `result.sample` ✅ (`ResultResponse.result`).
- Status handling: the enum is
  `Task not found | Pending | Reasoning | Generating | Request Moderated | Content Moderated | Ready | Error`.
  We branch on `Ready`, `Error`, `Content Moderated`, `Request Moderated`,
  `Task not found` and treat everything else as keep-polling — so the two newer
  states, `Reasoning` and `Generating`, are handled correctly by construction.
- Request fields `prompt`, `width`, `height`, `aspect_ratio`, `seed`,
  `output_format`, `image_prompt` all match the per-model input schemas, and the
  catalog's `extra_allowlist` (`output_format`, `prompt_upsampling`,
  `safety_tolerance`) is exactly right for `FluxPro11Inputs`.

---

## 14. Ideogram — image ✅

Adapter: `litegen-core/src/providers/image/ideogram.rs` · Catalog: `models/ideogram.yaml`
Verified against <https://developer.ideogram.ai/openapi.json>.

### ✅ Verified correct

Everything in the v3 path matches the spec exactly:

- `POST {base}/v1/ideogram-v3/generate` ✅ (path present in the spec).
- `Api-Key` header auth ✅.
- Fields `prompt`, `aspect_ratio`, `resolution`, `rendering_speed`,
  `negative_prompt`, `num_images`, `seed`, `style_type` — all present.
- `rendering_speed` enum `FLASH | TURBO | DEFAULT | QUALITY` ✅ (we send TURBO /
  QUALITY for the `-turbo` / `-quality` catalog ids).
- `style_type` enum `AUTO | GENERAL | REALISTIC | DESIGN | FICTION` — the
  catalog's `enum_values` is character-for-character identical.
- `ideogram_aspect()` rewrites `16:9` → `16x9`, and the spec's `aspect_ratio`
  enum is indeed the `x` form (`1x3, 3x1, 1x2, 2x1, 9x16, 16x9, 10x16, 16x10,
  2x3, 3x2, 3x4, 4x3, 4x5, 5x4, 1x1`). Every value in the catalog's `allowed`
  list maps into it ✅.
- Ref field names `style_reference_images` / `character_reference_images` ✅,
  and `max_count: 3` matches.
- `extra_allowlist` entries `magic_prompt`, `style_codes`, `style_preset`,
  `color_palette` all exist on the endpoint ✅.

### ⚠️ The spec documents multipart only

`/v1/ideogram-v3/generate` declares a single content type,
`multipart/form-data`. We send `application/json` whenever there are no
reference images (`image/ideogram.rs:202-217`). This has presumably been working,
but it is undocumented — worth one live check, and worth switching to multipart
unconditionally if it is not guaranteed.

### 🆕 Ideogram 4.0 is out and we are on 3.0

`/v1/ideogram-v4/generate` (plus `/async/generate`, `generate-transparent`,
`remix`, `describe`, `magic-prompt`). Note the request shape **changed**: the
prompt field is `text_prompt` (or a structured `json_prompt` object), not
`prompt`. Resolutions go up to `3328x1248` / `2048x2048`, against v3's
`1536x640` ceiling.

### 🆕 Other unimplemented endpoints

`/v1/text-to-image/p-image-ideogram` (P-Image, sync + async),
`/v1/ideogram-v3/{inpaint,remix,reframe,replace-background,layerize-text}`,
`/v1/remove-background`, `/v1/remove-object`, `/v1/async/ad-resizer`, and the
custom-model-training suite. v3 also has `custom_model_uri` and
`enable_copyright_detection` fields we never send.

---

## 15. Recraft — image ✅

Adapter: `litegen-core/src/providers/image/recraft.rs` · Catalog: `models/recraft.yaml`
Docs: <https://www.recraft.ai/docs/api-reference/endpoints.md>

### ✅ Verified correct

- `POST https://external.api.recraft.ai/v1/images/generations` ✅.
- `Authorization: Bearer RECRAFT_API_TOKEN` ✅ verbatim.
- Fields `prompt`, `model`, `n`, `size`, `style`, `negative_prompt`,
  `response_format` all match the documented parameter table.
- The `model` enum includes every id we ship — `recraftv4_1`, `recraftv4_1_pro`,
  `recraftv3`, `recraftv3_vector`, `recraftv2` — and `recraftv4_1` is the
  documented default.
- **The style gating is exactly right.** The docs mark `style` and `style_id`
  compatibility as "V2 / V3 styles"; `image/recraft.rs:124-130` restricts the
  field to native ids starting `recraftv2`/`recraftv3` and explains why. Nice
  catch that is still accurate.
- `extra_allowlist` entries `style_id`, `controls`, `text_layout` are all real
  parameters, and `text_layout` is correctly only on the V3/V4 entries.

### 🆕 Unimplemented

- `random_seed` — Recraft's seed parameter, supported on all models. We never
  send a seed at all.
- `/v1/images/generations/raster` and `/v1/images/generations/vector` — variants
  that server-side enforce the output type.
- `POST /v1/images/imageToImage` — image-to-image. Our `generate()` takes
  `_materialized` and ignores it, so Recraft is text-to-image only.
- `POST /v1/styles` (create a style reference from up to 5 images, returns the
  `style_id` we already allowlist).
- Models absent from the catalog: `recraftv4_1_utility`,
  `recraftv4_1_utility_pro`, `recraftv4_pro`, and the `_vector` counterparts.
- 🆕 Every image-taking endpoint now accepts JSON with URL/data-URL fields
  (`image_url`, `mask_url`, `image_urls`) as an alternative to multipart.

---

## 16. Stability — image 🔴

Adapter: `litegen-core/src/providers/image/stability.rs` · Catalog: `models/stability.yaml`
Verified against Stability's live OpenAPI: <https://api.stability.ai/v2alpha/openapi>
(served as `info.version: v2beta`; this is the document their docs site renders).

### 🔴 Both SD3 model values were replaced

`/v2beta/stable-image/generate/sd3` declares:

```
model: enum = [sd3.5-large, sd3.5-large-turbo, sd3.5-medium]
```

`resolve_model()` sends `sd3-large` for `stability/sd3-large` and `sd3-turbo`
for `stability/sd3-turbo` (`image/stability.rs:74-77`). Neither is in the enum.
The mapping is `sd3-large` → `sd3.5-large` and `sd3-turbo` →
`sd3.5-large-turbo`; `sd3.5-medium` is a third option we do not expose.

### ⚠️ `model` is sent to `/core` and `/ultra`, which have no such field

`image/stability.rs:180-182` adds `.text("model", model_name)` unconditionally
for every v2 route. The `core` and `ultra` request schemas contain only
`prompt`, `aspect_ratio`, `negative_prompt`, `seed`, `style_preset`,
`output_format` (+ `image`/`strength` on ultra) — there is no `model` field on
either.

### ⚠️ Image-to-image never engages on sd3

The sd3 schema has `mode: enum = [text-to-image, image-to-image]`, and the
`image` + `strength` fields only apply in `image-to-image` mode. We never send
`mode`. `stability/sd3-large` advertises `capabilities.image_to_image: true`
with a `ref_inputs` `field_map: { init: image }`, so a caller can attach a ref
image, we upload it as the `image` part, and the request still runs as
text-to-image.

### ⚠️ `style_preset` is accepted then dropped

`models/stability.yaml` declares a `style` param on `stability/core` and
`stability/ultra` whose `enum_values` are exactly the API's `style_preset`
values. `grep` finds no `style_preset` anywhere in `stability.rs` — the field is
never written to the form. The validator accepts the caller's style and the
provider silently ignores it.

### ⚠️ `aspect_ratio_from_size()` can emit unsupported ratios

The v2 enum is `21:9, 16:9, 3:2, 5:4, 1:1, 4:5, 2:3, 9:16, 9:21`.
`image/stability.rs:115-128` can return `"4:3"` and `"3:4"`, neither of which is
in it. Latent today — no Stability catalog entry declares a `size` param, so the
derivation path is unreachable — but it is wrong.

### ✅ Verified correct

- `POST {base}/v2beta/stable-image/generate/{core|sd3|ultra}` — all three paths
  present in the spec ✅, and confirmed live (HTTP 401, i.e. routed, not 404;
  a bogus `…/generate/sd3.5-flash` returns 404 by contrast).
- `Accept: image/*` for raw bytes ✅ — matches the documented `accept` header
  enum and its `image/*` default.
- `Authorization: Bearer` ✅, `multipart/form-data` ✅ (the only content type the
  spec declares for these routes).
- Fields `prompt`, `aspect_ratio`, `negative_prompt`, `seed`, `output_format`,
  `strength` all match; `output_format` `png` is in the enum ✅.
- Catalog `aspect_ratio.allowed` lists match the API enum exactly on all four v2
  models ✅.
- The v1 path for `stability/sdxl`
  (`/v1/generation/stable-diffusion-xl-1024-v1-0/text-to-image`) is still live
  (HTTP 401). Stability's spec preamble confirms v1 is maintained but frozen:
  "All AI services on other APIs (gRPC, REST v1, RESTv2alpha) will continue to
  be maintained, however they will not receive new features or parameters."
  `text_prompts[]`, `cfg_scale`, `steps`, `samples`, `width`/`height` and
  `artifacts[].base64` remain correct for that surface.

### 🆕 Large unimplemented surface

`stable-image/edit/{erase,inpaint,outpaint,remove-background,replace-background-and-relight,search-and-recolor,search-and-replace}`,
`stable-image/control/{sketch,structure,style,style-transfer}`,
`stable-image/upscale/{conservative,creative,fast}`, `3d/stable-fast-3d`,
`3d/stable-point-aware-3d`, and the `audio/stable-audio-2` family.

---

## 17. MiniMax — image ✅

Adapter: `litegen-core/src/providers/image/minimax.rs` · Catalog: `models/minimax.yaml`
Verified against the OpenAPI blocks MiniMax embeds in
<https://platform.minimax.io/docs/api-reference/image-generation-t2i.md> and
`…/image-generation-i2i.md`.

### ✅ Verified correct — this one matches field-for-field

- `POST {base}/image_generation` (i.e. `/v1/image_generation`) ✅ — the same path
  serves both text-to-image and image-to-image.
- `Authorization: Bearer` ✅.
- `model` enum for t2i is exactly `[image-01]`; i2i is `[image-01, image-01-live]`.
  Our catalog ships `minimax/image-01` ✅.
- `aspect_ratio` enum
  `1:1, 16:9, 4:3, 3:2, 2:3, 3:4, 9:16, 21:9` — the catalog's `allowed` list is
  identical, including `21:9` ✅.
- `response_format` enum `[url, base64]`; we send `base64` ✅ (and the docs warn
  that `url` expires in 24h, so `base64` is the right choice).
- `n` range 1-9 ✅.
- `subject_reference: [{type: "character", image_file: …}]` matches
  `ImageSubjectReference` ✅.
- `prompt_optimizer` (the sole `extra_allowlist` entry) is a real field ✅.
- Prompt max length 1500 in the catalog matches the documented limit ✅.

### 🆕 Not exposed

`image-01-live` (i2i only), `seed`, and explicit `width`/`height`
(512-2048, divisible by 8; `aspect_ratio` wins if both are sent).

---

## 18. MiniMax — video ✅

Adapter: `litegen-core/src/providers/video/minimax.rs` · Catalog: `models/minimax.yaml`
Verified against `…/video-generation-t2v.md`, `…-i2v.md`, `…-s2v.md`.

### ✅ Every catalog model is in the documented enum

| Catalog id | Documented in |
|---|---|
| `minimax/MiniMax-Hailuo-2.3` | t2v enum, i2v enum |
| `minimax/MiniMax-Hailuo-02` | t2v enum, i2v enum |
| `minimax/T2V-01-Director` | t2v enum |
| `minimax/S2V-01` | s2v enum |

- `POST /v1/video_generation`, `GET /v1/query/video_generation?task_id=`,
  `GET /v1/files/retrieve?file_id=` ✅ — all three still documented.
- Field names `model`, `prompt`, `duration`, `resolution`, `first_frame_image`,
  `last_frame_image`, and `subject_reference[{type: "character", image_file}]`
  all match ✅.
- The two-step completion (task → `file_id` → `files/retrieve` →
  `file.download_url`) matches the documented download flow ✅.
- Resolution values `512P/720P/768P/1080P` ✅.

### ⚠️ Duration/resolution pairs are constrained per model

The docs give a matrix: `MiniMax-Hailuo-2.3` and `MiniMax-Hailuo-02` accept
`6` or `10` seconds at 768P and only `6` at 1080P; "other models" accept `6`
only, at 720P. We forward `duration` and `resolution` independently without
enforcing the pairing, so some combinations will be rejected by the vendor
rather than by our validator.

### 🆕 A v2 API exists alongside v1

`POST /v2/video_generation` with a multimodal `content` array
(`text` / `image_url` / `video_url` / `audio_url` items, each with a `role` like
`first_frame` / `last_frame` / `reference_image`), model **`MiniMax-H3`**, 2K
output, 4-15s duration, and `ratio: "adaptive"`. v1 is still fully documented so
we are not broken — but H3 is the current model and is only reachable through
v2. Also new: `/v2/query/…`, list/cancel/delete, H3-Context-IR (structured
prompt generation), video regeneration to 2K, and the Video Agent template API.
`MiniMax-Hailuo-2.3-Fast` (i2v) is also unexposed.

---

## 19. ByteDance — image ✅

Adapter: `litegen-core/src/providers/image/bytedance.rs` · Catalog: `models/bytedance.yaml`
Docs: <https://docs.byteplus.com/en/docs/ModelArk/1824718>

### ✅ Verified correct

- `POST {base}/images/generations` on `https://ark.ap-southeast.bytepluses.com/api/v3`
  — the OpenAI-compatible synchronous image surface ✅.
- `Authorization: Bearer <ARK_API_KEY>` ✅.
- Body `{model, prompt, response_format: "b64_json", size}` ✅ and response
  `data[].b64_json` ✅ — matches the OpenAI-compatible contract ModelArk exposes.

### ⚠️ Model snapshots are dated and behind

`seedream-4-0-250828` and `seedream-3-0-t2i-250415` are pinned date snapshots.
ModelArk's model list has moved on — Seedream 4.5 and Seedream 5.0 Pro are both
current elsewhere (they appear in Runway's and Leonardo's routed catalogs).
Nothing is broken today, but the snapshots will age out.

---

## 20. ByteDance — video ⚠️

Adapter: `litegen-core/src/providers/video/bytedance.rs` · Catalog: `models/bytedance.yaml`
Docs: <https://docs.byteplus.com/en/docs/ModelArk/1520757> (create) ·
<https://docs.byteplus.com/en/docs/ModelArk/1521309> (retrieve)

### ✅ Verified correct

- `POST {base}/contents/generations/tasks` → `{id}`;
  `GET {base}/contents/generations/tasks/{id}` ✅ — both routes still documented
  under "Create a video generation task" / "Retrieve a video generation task".
- `Authorization: Bearer` ✅.
- Body `{model, content: [...]}` with `type: "text"` and
  `type: "image_url", image_url: {url}` blocks ✅.
- Poll `status == "succeeded"` and output at `content.video_url` ✅.

### ⚠️ Model snapshots are a generation behind

`doubao-seedance-1-0-pro-250528` and `doubao-seedance-1-0-lite-i2v-250428`.
ModelArk now documents **Seedance 2.0** ("Dreamina Seedance 2.0 series") and
"Seedance 1.0 Pro Fast". Our pinned 1.0 snapshots should still resolve, but the
catalog is two releases behind.

### 🆕 Unimplemented

`callback_url`, list-tasks and cancel/delete-task endpoints.

---

## 21. Vidu — video ⚠️

Adapter: `litegen-core/src/providers/video/vidu.rs` · Catalog: `models/vidu.yaml`
Docs: <https://platform.vidu.com/docs/model-map.md> ·
<https://platform.vidu.com/docs/text-to-video.md>

### ✅ All three catalog models still exist

`viduq1`, `vidu2.0` and `viduq2-pro` all appear in the current Model Map. The
adapter's surface is right, and its endpoint selection (routing to
`reference2video` / `start-end2video` / `img2video` / `text2video` based on the
ref roles present) is a genuinely good match for how Vidu splits its API.

- `POST {base}/ent/v2/{text2video|img2video|start-end2video|reference2video}` ✅.
- `Authorization: Token <key>` ✅ — note this is `Token`, not `Bearer`, and we
  get it right.
- Fields `model`, `prompt`, `images`, `duration`, `resolution`, `aspect_ratio`,
  `seed`, `movement_amplitude` ✅ all documented.
- Poll `GET {base}/ent/v2/tasks/{id}/creations`, `state: created | queueing |
  processing | success | failed`, output `creations[].url` ✅.
- `aspect_ratio` values `16:9, 9:16, 3:4, 4:3` ✅ (the catalog additionally
  allows `1:1`, which the docs do not list).

### ⚠️ Two models advertise text-to-video that the Model Map says they lack

| Model | Model Map: Text-to-Video | Catalog `capabilities` |
|---|---|---|
| `viduq1` | ✔️ | `text_to_video: true` ✅ |
| `vidu2.0` | **—** | `text_to_video: true` ❌ |
| `viduq2-pro` | **—** | `text_to_video: true` ❌ |

For those two, a prompt-only request routes to `/ent/v2/text2video` with a model
that does not support it.

### ⚠️ Duration and resolution ranges are wrong

- `viduq1`: docs say **5s only** and **1080p only**. Catalog:
  `duration_seconds {min: 4, max: 5}` and `resolution [360p, 720p, 1080p]`.
- `vidu2.0`: docs say **4s or 8s** (discrete). Catalog: continuous `4.0-8.0`,
  so 5/6/7 are accepted by us and rejected by Vidu.
- `viduq2-pro`: docs say 1-10s and 540p/720p/1080p — our 4-8s / 720p-1080p is a
  safe subset ✅.

### 🆕 An entire newer model series

**ViduQ3**: `viduq3-pro`, `viduq3-mix`, `viduq3-drama`, `viduq3-ad`,
`viduq3-turbo` — 1-16s, up to 1080p, with simultaneous audio+video output and
smart scene cuts. Also missing: `viduq2`, `viduq2-turbo`, `viduq1-classic`.
Unimplemented endpoints include video-extension, multi-frame, upscale-pro,
lip-sync, motion-sync, digital-human and the one-click film templates.

---

## 22. PixVerse — video ⚠️

Adapter: `litegen-core/src/providers/video/pixverse.rs` · Catalog: `models/pixverse.yaml`
Docs: <https://docs.platform.pixverse.ai/text-to-video-882970m0.md> ·
`…/model-overview-2140345m0.md`

### ✅ Verified correct — a clean match

- `POST {base}/openapi/v2/video/text/generate`,
  `…/video/img/generate`, `…/video/transition/generate` ✅.
- `POST {base}/openapi/v2/image/upload` (multipart) → `Resp.img_id`, then
  referencing that integer in the generate call ✅ — exactly the documented
  two-step flow.
- `GET {base}/openapi/v2/video/result/{video_id}` ✅.
- `API-KEY` header + per-request `Ai-trace-id` ✅ (both documented).
- `{ErrCode, ErrMsg, Resp}` envelope ✅; `Resp.video_id`, `Resp.status`,
  `Resp.url` ✅.
- Body fields `model`, `prompt`, `quality`, `duration`, `aspect_ratio`,
  `img_id` ✅ — and the docs' own curl example still sends `"model": "v4.5"`,
  confirming our catalog ids remain valid.

### ⚠️ The catalog predates the current model line

We ship `pixverse/v3.5`, `v4.5`, `v5`. PixVerse's Model Overview now recommends
only two models for new integrations — **C1** (cinematic, reference-based) and
**V6** (general, multi-clip, video extension) — and files everything else under
"Legacy models: existing integrations that depend on older model behavior".

### 🆕 Unimplemented endpoints

Extend, Modify, Restyle, Swap, Mimic (motion control), Fusion
(reference-to-video), Multi-transition, Sound effects, Lip-sync, Avatar, image
templates, and webhook delivery.

---

## 23. Leonardo — image ✅

Adapter: `litegen-core/src/providers/image/leonardo.rs` · Catalog: `models/leonardo.yaml`
Docs: <https://docs.leonardo.ai/reference/creategeneration> ·
<https://docs.leonardo.ai/docs/list-of-models.md>

### ✅ Verified correct

- `POST https://cloud.leonardo.ai/api/rest/v1/generations` → 
  `sdGenerationJob.generationId`; poll `GET /generations/{id}` until
  `generations_by_pk.status == "COMPLETE"`; read
  `generations_by_pk.generated_images[0].url` ✅ — the whole async shape matches.
- `POST /init-image` presigned-S3 upload for reference images ✅.
- `modelId` as a UUID ✅, and `extra_allowlist` entries `alchemy`, `photoReal`,
  `presetStyle`, `contrast` are all real Leonardo body parameters.

### 🆕 Leonardo is now a multi-vendor router and we expose one model

`models/leonardo.yaml` ships a single image entry, `leonardo/diffusion-xl`.
The current "Commonly Used Models" list includes Lucid Origin, Lucid Realism,
Phoenix 1.0, Flux Dev / Schnell / Kontext pro / **Flux.2 Pro**, Seedream 4.0 /
4.5 / **5.0 Pro**, Nano Banana and Nano Banana Pro, **GPT Image-1.5** and
**GPT Image 2**, Ideogram 3.0, P-Image-Ideogram, and Krea 2 Turbo.

---

## 24. Leonardo — video ⚠️

Adapter: `litegen-core/src/providers/video/leonardo.rs` · Catalog: `models/leonardo.yaml`

### ✅ Verified correct

- `POST /generations-image-to-video` → poll `GET /generations/{id}` until
  `generations_by_pk.status == "COMPLETE"`, output at
  `generated_images[0].motionMP4URL` ✅.
- Enum-style values (`MOTION2`, `VEO3`, `KLING2_1`, `RESOLUTION_720` …) match
  Leonardo's screaming-case convention ✅.
- 📄 The cited reference page
  <https://docs.leonardo.ai/reference/createimagetovideogeneration> now returns
  **404** — the docs were reorganised under `docs.leonardo.ai/docs/…` and
  `/v1.0_relaunch/…`. The `@see` links need updating.

### ⚠️ Model enum is well behind

We know `MOTION2`, `MOTION2FAST`, `VEO3`, `VEO3FAST`, `KLING2_1`, `KLING2_5`,
and the catalog exposes `leonardo/motion2`, `leonardo/veo3`, `leonardo/kling2.1`.
Leonardo's current video line-up documents Kling 2.6, **Kling 3.0**, **Kling 3.0
Turbo**, Kling O1, Kling O3, Grok Imagine 1.5, Gemini Omni Flash, Happy Horse
1.1, and Seedance 1.0 / 1.0 Pro.

Worth a specific check: `leonardo/veo3` routes to Google Veo 3, and Veo 3.0 was
shut down on Google's own API on 2026-06-30 (see §4). Whether Leonardo still
serves `VEO3` needs one live call.

### ✅ Image-only video is correctly modelled

`generations-image-to-video` requires a start-frame `imageId`, and the adapter
enforces it — `video/leonardo.rs` returns
`InvalidRequest("leonardo video requires a start-frame reference image")` when
no ref resolves. All three catalog entries declare
`capabilities: { image_to_video: true }` and nothing more, so the catalog and
the adapter agree. Text-to-video simply is not offered through this provider.

---

## 25. Kling — image ✅

Adapter: `litegen-core/src/providers/image/kling.rs` · Catalog: `models/kling.yaml`
Docs: <https://app.klingai.com/global/dev/document-api/> (auth: JWT per
`…/quickStart/userManual`)

### ✅ Verified correct

- `POST {base}/v1/images/generations` → `data.task_id`; poll
  `GET {base}/v1/images/generations/{task_id}` until
  `data.task_status == "succeed"` ✅.
- JWT bearer auth built from the access-key/secret pair
  (`providers/auth/kling_jwt.rs`) ✅.
- `model_name` body field ✅ (not `model` — easy to get wrong, and we have it
  right). Catalog ships `kling-v2` and `kling-v1-5`.

### ⚠️ Behind the current model line

Kling's current API models are v2.1, v2.5 Turbo, v2.6 and v3.0 — visible in
every downstream router (Leonardo documents Kling 2.6 / 3.0 / 3.0 Turbo / O1 /
O3; fal serves `fal-ai/kling-video/v2.5-turbo/pro/…`; Replicate serves
`kwaivgi/kling-v2.1`). Kling's own developer docs sit behind an app login, so
the exact `model_name` strings could not be read first-hand — this row is
verified structurally, not model-by-model.

---

## 26. Kling — video ⚠️

Adapter: `litegen-core/src/providers/video/kling.rs` · Catalog: `models/kling.yaml`

### ✅ Verified correct

- `POST {base}/v1/videos/text2video` and `…/image2video` → `data.task_id`;
  poll `GET {base}/v1/videos/{kind}/{task_id}` ✅.
- Output `data.task_result.videos[0].url` ✅.
- `model_name` + `prompt` body ✅; JWT auth ✅.
- The `video-` prefix convention in the catalog ids (stripped in
  `resolve_model()`) correctly keeps image and video entries distinct when they
  share a `model_name` — a real design decision, still sound.

### ⚠️ Same model-currency gap as the image side

Catalog: `kling/video-kling-v2-1`, `kling/video-kling-v1-6`. Current: v2.5
Turbo, v2.6 (adds native audio in one inference call), v3.0.

---

## 27. Bedrock — image ✅

Adapter: `litegen-core/src/providers/image/bedrock.rs` · Catalog: `models/bedrock.yaml`
Docs: <https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_InvokeModel.html>

### ✅ Verified correct

- `POST https://bedrock-runtime.{region}.amazonaws.com/model/amazon.nova-canvas-v1:0/invoke` ✅.
- SigV4 request signing (`providers/auth/sigv4.rs`) ✅.
- Body `{taskType: "TEXT_IMAGE", textToImageParams: {text, negativeText},
  imageGenerationConfig: {width, height, cfgScale, seed, numberOfImages}}` ✅ —
  matches the Nova Canvas request schema.
- `amazon.nova-canvas-v1:0` is the current Nova Canvas model id ✅.

---

## 28. Bedrock — video ✅

Adapter: `litegen-core/src/providers/video/bedrock.rs` · Catalog: `models/bedrock.yaml`
Docs: <https://docs.aws.amazon.com/nova/latest/userguide/video-gen-access.html> ·
<https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_StartAsyncInvoke.html>

### ✅ Verified correct — matches the AWS docs line for line

- `POST /async-invoke` with `{modelId, modelInput, outputDataConfig}` →
  `{invocationArn}`; poll `GET /async-invoke/{invocationArn}` ✅.
- `modelId: "amazon.nova-reel-v1:1"` — the userguide states verbatim: "For
  Amazon Nova Reel, this is `amazon.nova-reel-v1:1`" ✅. Our catalog id is
  `bedrock/amazon.nova-reel-v1:1` ✅.
- `modelInput: {taskType: "TEXT_VIDEO", textToVideoParams: {...},
  videoGenerationConfig: {durationSeconds, fps, dimension, seed}}` ✅.
- `outputDataConfig.s3OutputDataConfig.s3Uri` ✅.
- Image-to-video: `textToVideoParams.images: [{format: "png", source: {bytes}}]`
  ✅ — matches the documented `imageSource` schema exactly.
- Status values `Completed | InProgress | Failed` ✅.

### ⚠️ The catalog advertises durations the `TEXT_VIDEO` task cannot serve

The docs are emphatic that for `TEXT_VIDEO` the *only* supported values are
`durationSeconds: 6`, `fps: 24`, `dimension: "1280x720"`, and that input images
must be exactly 1280x720. `models/bedrock.yaml` declares
`duration_seconds: {min: 6.0, max: 120.0}` — but 12-120s is the
`MULTI_SHOT_AUTOMATED` task, which the adapter never emits. Every request over
6 seconds is therefore rejected by Bedrock.

### 🆕 Long-form task types unimplemented

`MULTI_SHOT_AUTOMATED` (up to 2 minutes from a 4000-char prompt, duration a
multiple of 6 between 12 and 120) and `MULTI_SHOT_MANUAL` (per-shot prompts and
images, base64 or `s3Location`). Also `ListAsyncInvokes`, and the
`video-generation-status.json` / per-shot `shot_NNNN.mp4` outputs.

### ⚠️ Output is an S3 URI, not a fetchable URL

Same open follow-up flagged in the Veo section: the poll returns
`outputDataConfig.s3OutputDataConfig.s3Uri`, which the caller cannot fetch
without our AWS credentials.

---

## 29. Hunyuan — image ✅

Adapter: `litegen-core/src/providers/image/hunyuan.rs` · Catalog: `models/hunyuan.yaml`
Docs: <https://cloud.tencent.com/document/product/1729/105969> ·
<https://www.tencentcloud.com/document/product/845/32207> (TC3-HMAC-SHA256)

### ✅ Verified correct

- RPC-style Tencent Cloud call: `POST /` to `hunyuan.tencentcloudapi.com` with
  the action in the `X-TC-Action` header ✅.
- Actions `SubmitHunyuanImageJob` → `QueryHunyuanImageJob` ✅.
- `X-TC-Version: 2023-09-01` ✅ matches the documented `apiVersion`.
- TC3-HMAC-SHA256 signing with `SecretId`/`SecretKey` ✅.
- Region constraint (`ap-guangzhou`) recorded in the adapter's own doc comment,
  matching the docs' note that only that region is supported.

---

## 30. Hunyuan — video ✅

Adapter: `litegen-core/src/providers/video/hunyuan.rs` · Catalog: `models/hunyuan.yaml`
Docs: <https://github.com/TencentCloud/tencentcloud-sdk-nodejs/blob/master/src/services/vclm/v20240523/vclm_client.ts>

### ✅ Verified correct

- `POST /` to `vclm.tencentcloudapi.com` ✅.
- Actions `SubmitImageToVideoJob` → `DescribeImageToVideoJob` ✅ — these are the
  exact method names in the official Tencent Cloud Node SDK's `vclm` client.
- `X-TC-Version: 2024-05-23` ✅ matches the SDK's `apiVersion`.
- Response `Response.Status == "DONE"` and `Response.ResultVideoUrl` ✅.
- TC3 signing shared with the image adapter ✅.

### ⚠️ Image-to-video only

`SubmitImageToVideoJob` requires an input image; `hunyuan/hunyuan-video` cannot
serve a text-only request. Worth confirming the catalog's `capabilities` reflect
that.

---

## Remediation

Applied 2026-08-16. Every row below has a test that failed before the fix and
passes after it. Catalog-level facts live in
`litegen-core/tests/catalog_conformance.rs` (a fast integration test that needs
only the built lib); wire-shape facts live in the per-adapter `#[cfg(test)]`
modules next to the code they constrain.

### 🔴 Broken → fixed

| Finding | Fix | Test |
|---|---|---|
| `dall-e-2`/`dall-e-3` shut down 2026-05-12 — the only two image models we shipped | `models/openai.yaml`: replaced with `openai/gpt-image-2` and `openai/gpt-image-1` | `no_retired_model_ids_are_advertised`, `replacement_models_are_advertised` |
| `resolve_model_name()` fell back to the dead `dall-e-2` for any unknown id | pass unknown native ids through verbatim | `unknown_model_ids_are_not_rewritten_to_a_dead_model` |
| `response_format` sent to GPT image models, which reject it | gate on `is_gpt_image()` | `omits_response_format_for_gpt_image_models` |
| GPT image models always return base64 regardless of what was asked | prefer `b64_json` whenever present | `decodes_b64_json_for_gpt_image_even_when_caller_asked_for_url` |
| ref images silently dropped (generations has no image input) | route to `POST /v1/images/edits` (multipart) when a ref is present | `reference_image_routes_to_the_edits_endpoint` |
| `imagen-3.0-generate-002` shut down 2025-11-10 | dropped `google/imagen-3`; added `gemini-3.1-flash-image` + `-lite` | `no_retired_model_ids_are_advertised`, `catalog_ships_the_current_nano_banana_2_model` |
| `gemini-3-pro-image` rewritten to the `-preview` alias, dead since 2026-06-25 | stop appending `-preview` | `gemini_3_pro_image_is_not_rewritten_to_the_retired_preview_id` |
| Veo 2.0 / 3.0 / 3.0-fast shut down 2026-06-30 | removed from `models/google.yaml` | `catalog_ships_no_retired_veo_models` |
| Veo keyframes sent as `{"inlineData":…}` — the `generateContent` shape, wrong for `:predictLongRunning` | emit `{bytesBase64Encoded, mimeType}` for `image` **and** `lastFrame` | `submits_veo_job_and_sends_api_key_header`, `last_frame_uses_bytes_base64_encoded_shape` |
| `gen3a_turbo` removed from Runway's API 2026-07-30, and returned for *every* model id | `gen4_turbo` / `gen4.5`; catalog ids `runway/gen4-turbo`, `runway/gen4.5` | `sends_gen4_turbo_not_the_retired_gen3a_turbo`, `sends_gen4_5_model_id_verbatim` |
| Runway video ratios `1280:768` / `768:1280` are gen3-era and now rejected | catalog + default switched to the six-value gen4 enum | `default_ratio_is_a_member_of_the_gen4_enum`, `runway_video_aspect_ratios_are_in_the_gen4_enum` |
| Stability sent `sd3-large` / `sd3-turbo`, no longer in the `model` enum | map to `sd3.5-large` / `sd3.5-large-turbo` (and accept `sd3.5-medium`) | `sd3_catalog_ids_map_to_the_current_sd35_model_values` |
| `bfl/flux-pro` → `POST /v1/flux-pro`, a path that no longer exists | dropped; added `bfl/flux-2-pro` | `no_retired_model_ids_are_advertised`, `replacement_models_are_advertised` |
| `fal/video` resolved to `fal-ai/ltx-video`, which has no `image_url` — image-to-video impossible | per-endpoint `EndpointSpec` with a distinct `image_to_video` route | `reference_image_routes_to_the_image_to_video_endpoint` |
| `stability-ai/stable-video-diffusion` removed from Replicate (404) | deleted the `svd` / `svd-xt` entries | `no_model_id_resolves_to_the_removed_stable_video_diffusion` |

### ⚠️ Spec mismatches → fixed

| Finding | Fix | Test |
|---|---|---|
| Sora `seconds` sent as a number; `VideoSeconds` is a string enum | `seconds.to_string()` in the JSON branch | `seconds_is_sent_as_a_string_in_the_json_body` |
| `responseModalities: ["image"]` — canonical wire value is the proto enum name | `["IMAGE"]` | `response_modalities_uses_the_canonical_uppercase_enum_name` |
| Google size-derivation emitted `5:4` / `4:5`, not in `ImageConfig.aspect_ratio` | re-banded; near-square collapses to `1:1` | `every_derived_aspect_ratio_is_supported_by_image_config` |
| Stability size-derivation emitted `4:3` / `3:4`, not in the v2 enum | re-banded to `5:4` / `4:5` | `every_derived_aspect_ratio_is_in_the_v2_enum` |
| Runway image mapped `3:2`→`1808:1152` and `2:3`→`1152:1808`, neither in the enum | dropped `3:2`/`2:3` from `allowed` (the enum has no true 3:2 pair) | `runway_image_aspect_ratios_are_all_mappable` |
| `model` sent to Stability's `/core` and `/ultra`, which have no such field | only send it on the `sd3` route | `model_field_is_only_sent_to_the_sd3_route` |
| Stability image-to-image never engaged — `mode` was never sent | send `mode: image-to-image` when an `image` part is attached | `image_to_image_sends_mode_image_to_image` |
| Stability `style` accepted by the validator then dropped | forward as `style_preset` | `style_is_forwarded_as_style_preset` |
| `imageSize` / `personGeneration` merged at `generationConfig` top level | nest under `generationConfig.imageConfig` | `image_config_keys_are_nested_under_image_config` |
| fal sent `duration` / `aspect_ratio` / `prompt` to endpoints whose schemas lack them | `EndpointSpec` gates each field per endpoint | `does_not_send_duration_or_aspect_ratio_to_ltx_video`, `stable_video_endpoint_receives_no_prompt` |
| Runway video never handled `CANCELLED` — a cancelled task polled until timeout | treat as `Failed` alongside `FAILED` | `cancelled_task_reports_failed_rather_than_pending` |
| Recraft never sent a seed; the field is `random_seed` | send `random_seed`; declare `seed` on all five models | `seed_is_sent_as_random_seed`, `recraft_models_declare_a_seed_param` |
| Vidu `vidu2.0` / `viduq2-pro` advertised text-to-video the Model Map says they lack | `capabilities: { image_to_video: true }` | `vidu_models_only_advertise_text_to_video_when_supported` |
| Vidu `viduq1` advertised 4-5s and 360p/720p; it is 5s at 1080p only | tightened the ranges | `vidu_q1_duration_and_resolution_match_the_model_map` |
| `replicate/modelscope` mapped to `deforum/deforum_stable_diffusion` under a wrong name | renamed to `replicate/deforum` (old id still accepted) | covered by the resolve test above |
| 📄 BFL `@see` links pointed at `docs.bfl.ai`; canonical host is `docs.bfl.ml` | updated | — (doc comment) |
| 📄 Leonardo `@see` pointed at a page that now 404s | repointed at the live model list | — (doc comment) |
| Sora catalog entries gave no hint of the 2026-09-24 shutdown | `DEPRECATED:` prefix in `description` | — (catalog copy) |

### Deliberately not fixed

- **New-model coverage.** FLUX.2's other four models, FLUX 3 video, Ideogram
  4.0, ViduQ3, PixVerse C1/V6, Runway's and Leonardo's routed third-party
  catalogs, Replicate's current official models, MiniMax's v2 `/v2` surface,
  Bedrock's `MULTI_SHOT_*` task types. These are feature work, not conformance
  bugs — nothing currently shipped is broken by their absence. The audit
  sections above enumerate them.
- **Re-hosting auth-gated outputs** (Veo `video.uri`, Bedrock `s3Uri`). Already
  a tracked cross-provider follow-up, and the right fix is shared plumbing
  rather than two more per-adapter downloads.
- **Google's `/v1beta/interactions` API.** `generateContent` still works for
  image models; moving surfaces deserves its own spike.
- **fal image latent field mismatches.** `negative_prompt` / `strength` /
  `style` / `image_url` are written unconditionally but are unreachable today —
  every fal image catalog entry has `extra_allowlist: []` and declares only
  params the endpoints accept. Flagged rather than changed, because a fix
  without a reachable failing test is speculative.

---

## Second-round verification pass

After the fixes landed, every claim in this document was re-checked against the
code rather than against the first draft. That found **four errors in the audit
itself** and **four additional defects** the first pass had missed.

### Errors in the original audit (corrected in place)

| Claim | Reality |
|---|---|
| "Runway video only ever posts to `/v1/image_to_video`" | Wrong — `video/runway.rs:121-128` already branches to `/text_to_video` when no `init` ref is present. |
| "Recraft's catalog advertises image-to-image it does not implement" | Wrong — all five `models/recraft.yaml` entries declare `capabilities: { text_to_image: true }` and never claim otherwise. |
| "Leonardo video advertises text-to-video" | Wrong — all three entries declare `image_to_video` only, and the adapter returns `InvalidRequest` without a start frame. Catalog and adapter already agreed. |
| "Every fal image entry has `extra_allowlist: []`" | Wrong — `fal/flux-pro` allows `safety_tolerance` and `output_format`. Both are real `flux-pro/v1.1` fields, so the conclusion survived, but the blanket claim did not. |

### Defects the first pass missed

| Finding | Evidence | Fix | Test |
|---|---|---|---|
| Stability sent `aspect_ratio` on image-to-image requests | spec: "This parameter is only valid for **text-to-image** requests" | skip it when a ref image is attached | `image_to_image_sends_mode_image_to_image` |
| Stability sent `strength` on text-to-image requests, and omitted it on image-to-image | spec: `strength` is "only valid for **image-to-image**", and i2i "requires the `prompt`, `image`, and `strength` parameters" | send only in i2i, defaulting to `0.5` | `strength_is_not_sent_for_text_to_image`, `image_to_image_defaults_strength_when_omitted` |
| `hunyuan/hunyuan-video` advertised `text_to_video`, but the adapter only calls `SubmitImageToVideoJob` | Tencent's `vclm_client.ts` has both `SubmitImageToVideoJob` and `SubmitTextToVideoJob`; we implement only the former | narrowed to `image_to_video`, `first_frame` now required | `hunyuan_video_advertises_only_what_the_adapter_implements` |
| `bedrock/amazon.nova-reel-v1:1` advertised 6-120s, but the adapter only emits `taskType: TEXT_VIDEO` | AWS: "durationSeconds — 6 is the only supported value currently" for `TEXT_VIDEO`; 12-120s is `MULTI_SHOT_AUTOMATED` | clamped to 6s | `bedrock_nova_reel_duration_matches_the_text_video_task` |

Both of the last two were narrowed rather than implemented: `SubmitTextToVideoJob`
and `MULTI_SHOT_AUTOMATED` are real, documented endpoints we could add, but
adding an unexercised vendor integration is feature work, not conformance
repair. The catalog now promises exactly what the adapters do.

### A test that was passing for the wrong reason

`does_not_send_duration_or_aspect_ratio_to_ltx_video` asserted that
`aspect_ratio` was absent from the body — but the shared `make_extras()` helper
sets `aspect_ratio: None`, so that half of the assertion would have passed
against the *unfixed* code too. The test now sets both fields explicitly and
asserts `duration_seconds > 0.0` up front, so neither assertion can go vacuous.

### Consistency sweep

Retired model ids were referenced outside `models/`, where they would have kept
advertising dead models to users:

- `apps/landing/src/config/models.generated.ts` — regenerated via
  `node scripts/sync-models.mjs` (82 models from 18 provider files); it had two
  `dall-e` entries.
- `README.md`, `litegen.example.yaml`, `sdks/python/README.md`,
  `sdks/python/examples/*.py`, `sdks/python/litegen/{client,__init__}.py` —
  example snippets repointed from `openai/dall-e-3` → `openai/gpt-image-2` and
  `runway/gen-3` → `runway/gen4-turbo`.
- `litegen-core/tests/live_providers.rs` — the gated live Veo test pointed at
  `google/veo-3.0-generate-001`; repointed to `veo-3.1-generate-preview`.
- The SDK's own mocked HTTP tests (`sdks/python/tests/test_client.py`,
  `sdks/typescript/test/*.test.ts`) also use those ids, but as opaque strings
  against a mock server — they never touch the catalog, so they were left alone.

### One more defect, found by auditing the tests themselves

`runway/image`'s `resolve_ratio()` had two problems the catalog fix alone
papered over:

- The `3:2` → `1808:1152` and `2:3` → `1152:1808` arms were still present.
  Removing those ratios from `allowed` made them unreachable, but the code could
  still emit an illegal value if the catalog ever changed back.
- A caller-supplied `size` was forwarded **verbatim** as `"{w}:{h}"` — so
  `size: 1000x800` would have produced `ratio: "1000:800"`, which is not in
  Runway's enum. Unreachable today (no Runway image model declares a `size`
  param), but wrong.

`resolve_ratio()` now snaps any input to the nearest member of a named
`RATIOS` constant and is *incapable* of returning a non-member.
`resolve_ratio_only_ever_returns_enum_members` calls the real function across
every advertised ratio plus unset / unknown / free-form / degenerate
(`100x0`, `not-a-size`) inputs.

The integration test that previously checked this was rewritten: it had
hardcoded its **own copy** of the adapter's mapping table, so it verified the
catalog against a duplicate rather than against the code. It now only asserts
the narrow catalog fact (`3:2` / `2:3` are not advertised), and the behavioural
guarantee lives in the unit test that can actually call `resolve_ratio`.

### The consistency sweep was itself incomplete

The first sweep for retired ids outside `models/` was piped through `head -30`,
which silently truncated it — so it reported "clean" while three integration
tests and the entire landing/dashboard surface still pinned dead ids. The
untruncated re-run found:

| Location | Why it mattered | Action |
|---|---|---|
| `litegen-core/tests/multitenant_api.rs` (×4) | posted `openai/dall-e-3` to the real API — **3 tests failing** with `404 model_not_found` | repointed to `openai/gpt-image-2` |
| `apps/landing/src/config/site.ts` (×7) | quickstart curl + both SDK snippets on the **public marketing site** | `gpt-image-2` / `gen4-turbo` |
| `apps/landing/src/components/Hero.tsx` | hero code sample, above the fold | `gpt-image-2` |
| `dashboard/e2e/compare-playground.spec.ts` | selected `openai/dall-e-3` **and** `bfl/flux-pro`, both now absent, and asserted on `style` (dall-e-3-only) | reworked onto `openai/gpt-image-1` + `bfl/flux-pro-1.1`, asserting on `quality`; verified the param sets (`{size, quality}` / `{seed, size}`) still give the shared-vs-exclusive overlap the test is about |
| `sdks/typescript/examples/*.ts` | runnable examples | `gpt-image-2` / `gen4-turbo` |

Deliberately left alone:

- `apps/price-api/**` — a separate pricing service with its own seed data,
  scrapers and e2e suite (`openai.seed.ts`, `runway.seed.ts`, `google.seed.ts`
  all list retired ids). It is decoupled from litegen-core and tracks vendor
  *pricing pages* rather than the generation catalog; migrating it is its own
  piece of work.
- Generated SDK surface (`sdks/*/src/generated/**`, `_generated/**`) — retired
  ids appear only in docstring examples emitted by the generator.
- Mock-only SDK tests (`sdks/python/tests`, `sdks/typescript/test`) — the ids
  are opaque strings against a mock server and never reach the catalog.
- `apps/landing/scripts/derive-models.test.mjs` lines 14/29 — an inline
  synthetic fixture, not the real catalog.
- `.playwright-mcp/` — untracked scratch page snapshots.

### Cross-language test sweep

Running the non-Rust suites caught one more place a retired id was pinned, and
surfaced one pre-existing failure unrelated to this work:

- `apps/landing/scripts/derive-models.test.mjs` asserted
  `openai/dall-e-3 should be present` against the **real** `models/` directory,
  so it went red the moment the catalog was corrected. Repointed at
  `openai/gpt-image-1` with its three standard sizes. **12/12 green.**
- `sdks/typescript` — `test/contract.test.ts` fails on four `allowed-models`
  endpoints that have no SDK method
  (`GET`/`PUT /v1/{apps,orgs}/{}/allowed-models`). **Pre-existing and unrelated:**
  `sdks/openapi.json` and the TypeScript SDK are untouched by this work; the
  endpoints were added in `52664a2`. The other 20 TS tests pass.
- `sdks/python` — `pytest` is not installed in this environment, so the Python
  SDK suite could not be run. Its changes here are limited to docstrings and
  example snippets.

---

## Verification

```
cargo test --lib --tests        # litegen-core
  lib .......................... 492 passed;  0 failed
  app_storage_db ................. 1 passed;  0 failed
  catalog_conformance ........... 13 passed;  0 failed   (new)
  live_providers ................. 0 passed; 21 ignored   (credential-gated)
  multitenant_api ............... 30 passed;  0 failed
  unit_tests .................... 31 passed;  0 failed
  EXIT=0
```

Baseline before this work was **458 lib tests passing**; the suite is now
**492 lib + 13 catalog-conformance**, i.e. **+47 tests**, all green.

Also verified:

- `apps/landing` — `node --test scripts/*.test.mjs` → **12/12**; `tsc --noEmit` clean.
- `sdks/typescript` — `tsc --noEmit` clean; `npm test` **20/21**, the single
  failure being the pre-existing `allowed-models` contract gap described above.
- `dashboard` — `tsc --noEmit` clean.

Not executed here:

- `dashboard/e2e/compare-playground.spec.ts` — Playwright, needs a running app
  and a seeded master key. It was edited (retired model ids → live ones) and its
  assumptions were re-checked against the catalog by hand: `openai/gpt-image-1`
  declares `{size, quality}` and `bfl/flux-pro-1.1` declares `{seed, size}`,
  which preserves the shared-vs-exclusive param overlap the test exercises.
- `sdks/python` — no `pytest` in this environment.
- `litegen-core/tests/live_providers.rs` — 21 tests, all `ignored` without
  provider credentials. These are the only tests that would exercise a real
  vendor endpoint, so **no fix in this pass has been confirmed against a live
  API** — every claim rests on published specs plus mocked-transport tests.


---

## Deploy blocker found while shipping this (2026-08-16)

`node deploy.js landing` would have **overwritten litegen.ai with a month-old
copy of the site**, silently. Found while preparing the deploy for this work;
recorded here because the failure mode is invisible from the deploy log.

`apps/landing/next.config.ts` sets `output: 'export'` and
`distDir: process.env.NEXT_DIST_DIR || '.next'`. With `output: 'export'`,
**`distDir` is the export destination.** `deploy.js` ran the build with
`NEXT_DIST_DIR=.next-prod`, so the export landed in `apps/landing/.next-prod/`
— while the directory the deploy actually tars up, `apps/landing/out/`, kept
whatever an earlier build had left in it.

The guard in place only checked existence:

```js
if (!fs.existsSync(landingOut)) { throw ... }   // passes on a stale directory
```

Measured on this checkout after a **successful** (`EXIT=0`) production build:

| | `out/` (shipped) | `.next-prod/` (freshly built) |
|---|---|---|
| `en.html` mtime | Jul 17 17:03 | Aug 16 10:01 |
| `dall-e-3` | present | 0 occurrences |
| `gpt-image-2` | absent | present |

Because the remote unpack is destructive — `find landing-dist -mindepth 1 -delete`
before extracting — shipping that `out/` replaces the live site wholesale. It
would have reverted a month of landing changes *and* re-published the retired
`openai/dall-e-3` that this very audit removed.

Two changes to `deploy.js`:

1. `NEXT_DIST_DIR: 'out'` — point the export at the directory that gets shipped.
   This still satisfies the original reason the variable was set at all (keeping
   the production build off a running `next dev`'s `.next`).
2. A **freshness assertion**: capture `Date.now()` before the build and fail if
   the shipped directory's mtime predates it. Existence is not freshness; a
   misrouted export now aborts loudly instead of publishing stale HTML.
