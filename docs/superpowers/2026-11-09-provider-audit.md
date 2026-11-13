# Provider audit, 2026-11-09

This audit checks every provider litegen carries against the vendor's own documentation,
as published on 2026-11-09. It answers four questions:

1. Are the prices we publish right?
2. Do our comments cite each vendor's API reference correctly?
3. Do we carry each vendor's current models?
4. Do our requests match each vendor's current API format?

**Scope:**
- 22 vendors;
- 226 catalog rows in `models/*.yaml` (mock excluded);
- 37 vendor adapters under `litegen-core/src/providers/`;
- 22 drift definitions;
- every surface that publishes a price: the price service, the landing site and the README.

**Method:**
1. The drift check (`scripts/model-drift/check.mjs`), run on a scratch copy of the
   snapshots, gave the leads.
2. Ten auditors each took one group of vendors. Each fetched every `docs_url`, every
   `@see` URL and every pricing page first-hand. They checked every quoted vendor string,
   price, field, enum, bound and status value against today's pages.
3. I re-checked first-hand every finding that moves money or may fail requests. These
   are marked **(re-checked)**.

The audit changed no code, catalog row or snapshot, and ran no live generation, so it
spent no money. The per-row evidence, with every price string and every vendor model
list, is in `.superpowers/sdd/2026-11-09-provider-audit/<group>.md`. The drift run is in
the same folder, under `drift-run/`.

**Labels:**

| Label | Meaning |
|---|---|
| 🔴 | broken today |
| 💲 | our price differs from the vendor's |
| ⚠️ | mismatch |
| 📄 | citation dead, moved or stale |
| 🆕 | vendor offers it, we don't carry it |
| ⏳ | announced shutdown |
| ✅ | verified correct |

**Confidence:**
- *verified*: read in an official vendor source today;
- *inferred*: follows from official sources but isn't stated outright;
- *unverifiable*: login wall, unpublished, or needs a live call.

## Fixes applied the same day

The user asked to "fix what we can with absolute confidence". Below is what was fixed,
what was left, and why. Every change rests on a vendor page read on 2026-11-09. Each
fix has a test that failed before the change and passes after it.

### Fixed

| Area | Change | Proof |
|---|---|---|
| Billing (§1) | `openai/gpt-image-2`: low $0.006, high $0.211, OpenAI's 1024² table (were ×0.5 / ×2.0) | `varying_rows_price_the_variant_asked_for`, `an_openai_image_is_quoted_by_its_quality_and_its_size`, `gpt_image_2_estimate_is_openais_table_by_quality` |
| Billing | Ideogram 3.0 with a character reference: $0.15 / $0.10 / $0.20, as `references.character` rules | `varying_rows_price_the_variant_asked_for` |
| Billing | `google/gemini-2.5-flash-image` $0.039 (was $0.02); `gemini-3.1-flash-lite-image` $0.0336 (was $0.034) | same |
| Billing | Vidu: Q3 Turbo $0.30 at 720p (was $0.275), 540p $0.035/s, 1080p $0.065/s; Q3 Pro 540p and 1080p; Q2 Pro Fast 1080p (16 credits + 4/s) | same |
| Billing | Luma Ray 2 (`ray-2`, `dream-machine`): 1080p $0.95, 4k $1.05 | same |
| Requests (§3) | Vidu Q3 Turbo has no `reference` role, the adapter refuses one, and duration is 1–16 s (its three endpoints) | `vidu_q3_turbo_serves_every_endpoint_the_adapter_routes_to`, `viduq3_turbo_refuses_reference_images_before_any_call` |
| Requests | Vidu Q1: no `audio` on the reference `images` form; no `movement_amplitude` or `bgm` on text2video | `viduq1_reference_images_send_no_audio_switch`, `viduq1_text2video_sends_no_movement_amplitude_or_bgm` |
| Requests | MiniMax H3-Lite reference-to-video sends the row's ratio, not `adaptive` | `h3_lite_references_never_ride_on_the_adaptive_default` |
| Requests | TokenHub Kling Image O1 and 3.0 Omni text-only requests send 16:9, not `auto`. This is an adapter constant, not a row default, because the dashboard fills row defaults in on image requests too | `kling_omni_text_to_image_never_rides_on_the_auto_default` |
| Requests | Replicate and BFL label results by their bytes. The router already relabelled by bytes (`router.rs:1727`), so callers never saw the old `png` label | `a_result_is_labelled_with_the_type_its_bytes_are` (both) |
| Bounds | Ideogram 4.5 prompt ≤ 10000; AnimateDiff seed ≤ 2147483647; fal AuraFlow `guidance_scale` ≤ 20 | three new conformance facts |
| Names | `stability/sd3-large` and `sd3-turbo` display "Stable Diffusion 3.5 Large" and "Large Turbo" | `stability_sd3_rows_are_named_for_the_sd3_5_models_they_run` |
| Citations (§4) | Six agents repaired the 78 citation findings, comments only, plus dead or redirected `docs_url` values (5 Google rows, now on aistudio.google.com; Vidu 2.0). Each new quote was checked by script against a page fetched that day | `.superpowers/sdd/2026-11-09-provider-audit/fixes-*.md`; `every_google_and_openai_row_names_its_docs_url` now pins Google's new host |
| Citations | Price citations added: Stability ×4, Recraft ×3, Ideogram 3.0 ×3, BFL `flux-dev` (`bfl.ai/pricing`) | — |
| Drift (§6) | All four failed sources repaired: ByteDance's heading; OpenAI's YAML (the parser reads explicit keys); Stability v1 from its markdown pages; Kling `llms.txt`. Also the duplicate `dola-seedream-5-0-pro` acknowledgement and the Vidu 2.0 comment | `node --test` 88/88. Each provider ran clean twice on scratch snapshots, and the tree's snapshots were untouched |
| Price service | Seeds regenerated: 5 rows changed, coverage OK, 226/226 equal the catalog. Luma and Runway `pricingUrl` fixed; the `fal/video` note corrected | `coverage:check`, Jest 31/31 |
| README | Stability names; `fal/video` is LTX Video; `replicate/video` is AnimateDiff | — |

**Verification:**
- The full `cargo test --tests` run (56 suites) had 9 failures in tests outside this
  work, all timing-sensitive. All 9 pass when rerun alone.
- Of the three load-sensitive mesh tests run alone, one,
  `many_distinct_key_time_arrays_write_in_linear_time`, still fails at 2.37–2.76 s
  against a 2 s limit, in the standalone mesh crate too. Load averages were 10–28,
  and no mesh file changed.
- `cargo clippy --all-targets --all-features -- -D warnings` is clean.
- The landing derive tests pass (25/25).
- Not verified live: no Vidu or Recraft key exists on this machine, and nothing was
  spent.

### Not fixed

| Item | Why |
|---|---|
| New models (§5): GPT Image 2.6 (OpenAI's named successors; the drift check now reports both), Omni 1.2, Kling 4.0 Flash, Luma Ray 3.2/Uni-1, the hub resales… | Whether to carry a model is the user's call |
| Recraft V2/V3 `style` ids | Needs one live call. If the old ids still work, switching breaks callers; if they don't, keeping them fails requests |
| FLUX 3 Max video bills list price during fal's promo; Vidu bills $0.005 a credit while only RMB is published | Price policy, so the user's call |
| MiniMax T2V-01-Director and S2V-01 | No published price: retire or cite, the user's call |
| Unpublished prices (§8) and the inferred ones (Klein Base above 1 MP, Seedream 5.0 Pro non-square 2K, OpenAI non-square sizes, Ultra Redux) | Not certain |
| GPT Image 2 `quality: auto` bills at medium | OpenAI publishes no price for `auto` |
| Vendor contradictions: Recraft remove-background field name, BFL `safety_tolerance`, the Tripo WebP pages, Flash Lite ratios, Ideogram 3.0 JSON vs multipart, Vidu Q1 image-to-video | The vendor disagrees with itself |
| Meshy rig 320,000 faces against Meshy's 380,000 triangles | Under-advertises only; the quote is now current and the value is kept |
| PixVerse V5.5/V5.6 and the other skips whose reasons the hub policy overturned | The user's call |

### Corrections found while fixing

Each of these was checked against the code or the vendor page.
- **Vidu "base64 refs are always `image/png`":** false. `base64_data_uri` sniffs the
  bytes first.
- **Replicate's "(litegen says jpeg)":** not a contradiction. It meant litegen's
  `jpeg` is sent as Replicate's `jpg` (`replicate.rs:411`), and is reworded.
- **Recraft and Ideogram forward `n`:** unreachable, as §7 says.
- **Stale quotes that weren't stale:**
  - the Seedream 4.0 page still names `seedream-4-0-250828`;
  - Leonardo's "1.75 times" and OpenAI's `user` quotes are still verbatim;
  - BFL's ultra page still shows 2752×1536 samples;
  - the 75 % promo text is still on fal's keyframes page.
- **Stale line numbers:** several audit line numbers predate the fixes. The fix logs
  give current ones.

## Bottom line

**1. Prices.**
- **The published tables all come from the catalog.** Every price litegen publishes
  comes from `models/*.yaml` and matches it exactly:
  - the price service seeds all 226 rows at `base_cost_usd`, with 0 differences, and its
    `coverage:check` passes;
  - the landing site's generated model data and README's provider table match the catalog;
  - the user-facing docs hard-code no prices.
- **Against the vendors:**
  - Every row whose vendor publishes a price matches it for the default request, except
    the rows in §1 and §2.
  - Five places bill below the vendor's cost, and a sixth may. The worst is GPT Image 2
    at high quality, billed at half price.
  - About 20 rows can't be checked from public pages (§8).

**2. Citations.**
- **`docs_url`:** every row has one. Fetched today, every one lands on the right content
  except Vidu 2.0's, which is a soft 404.
- **The comments have rotted in places.** The scorecard counts 78 citation findings, from
  dead or moved URLs to stale quotes, uncited prices and false statements. The largest:
  - Stability's OpenAPI spec, cited 21 times, now sits behind a sign-in.
  - Google's 14 URLs redirect to a new domain.
  - Vidu's legacy pages now describe other models.
  - 11 prices carry no citation at all.
- None of this changes what litegen does. It does break the "re-verify by opening the
  cited URL" workflow these comments exist for (§4).

**3. Models.**
- No carried model is past an announced shutdown date, and every carried model is still
  served. fal marks AuraFlow deprecated, with no date, and its row already records that.
- Vendors have shipped models we don't carry:
  - OpenAI's GPT Image 2.6 pair, which OpenAI names as its successors;
  - Google's Omni 1.2 video;
  - Kling 4.0 Flash;
  - Luma's new API (Ray 3.2, Uni-1);
  - BFL's own FLUX 3 Video;
  - Stability's SD3.5 Flash;
  - hundreds of resold routes on fal, Runway, Leonardo and Replicate (§5).

**4. API format.**
- Across all 22 vendors, these match each vendor's current schema: hosts, auth, paths,
  request fields, enums, bounds and status values.
- The exceptions:
  - one likely break: Vidu Q3 Turbo reference-to-video;
  - one high-risk default: Recraft V2/V3 `style`;
  - two inferred failures, one on TokenHub and one on MiniMax;
  - a few minor mismatches (§3).

  The likely break and the high-risk default each need one live call to confirm.

"Absolute confidence" has limits here:
- some vendors publish no price, or show it only behind a login;
- a few behaviours only a live call can settle.

§8 lists them all.

## Scorecard

| Provider | Rows | Prices vs vendor | Citations | Models | API format |
|---|---|---|---|---|---|
| openai | 3 image | bases ✅ 3/3 · 💲 GPT Image 2 quality rules (re-checked) · 💲 size rule (inferred) | 📄 6 | 🆕 GPT Image 2.6 ×2 | ✅ |
| google | 5 image | ✅ 3 · 💲 2.5 Flash Image (re-checked) · 💲 3.1 Flash Lite +1.2 % | 📄 11 | ⏳ 2.5 Flash Image 2027-03-15 (set) · 🆕 Omni 1.1/1.2 video | ⚠️ 2 |
| bfl | 11 image | ✅ 11/11 | 📄 5 | 🆕 FLUX 3 Video, klein 4B, Fill/Expand | ⚠️ 2 |
| stability | 10 image | ✅ 10/10 | 📄 3, incl. the spec behind a login (re-checked) | 🆕 SD3.5 Flash, 15 endpoints | ⚠️ 1 |
| bedrock | 3 image | ✅ 3/3 | 📄 1 | 🆕 Luma Ray 2, 4 edit tools | ✅ |
| ideogram | 11 image | ✅ 11/11 bases · 💲 character refs (re-checked) | 📄 3 | 🆕 7 | ⚠️ 4 |
| recraft | 11 image | ✅ 11/11 | 📄 2 | 🆕 11 endpoints | ⚠️ style default (re-checked) · ⚠️ 1 |
| pixellab | 1 image | unverifiable | 📄 1 | ✅ | ⚠️ 1 minor |
| leonardo | 1 image, 4 video | unverifiable (login) | 📄 2 | 🆕 v2 API, ~40 resold ids | ⚠️ 1 (known) |
| runway | 2 image, 2 video | ✅ 4/4 | 📄 2 | 🆕 23 new, 24 stale skips | ✅ |
| luma | 2 image, 3 video | 💲 Ray 2 at 1080p/4K (known gap) · rest unpublished | 📄 1 | 🆕 Agents API | ✅ (API now "legacy") |
| kling | 2 image, 5 video | ✅ 7/7 | 📄 4 | 🆕 5 | ✅ |
| minimax | 1 image, 9 video | ✅ 8 · 2 unpublished | 📄 2 | 🆕 2 minor | ⚠️ 1 (inferred) |
| bytedance | 5 image, 3 video | ✅ 8/8 | 📄 7 | 🆕 4 | ✅ |
| hunyuan | 1 image, 2 video | ✅ (Tencent contradicts itself on I2V) | 📄 2 | ✅ | ✅ |
| tokenhub | 7 image, 24 video | ✅ 31/31 (1 may over-bill) | 📄 2 minor | 🆕 3 + 3D | ⚠️ 1 (inferred) |
| vidu | 13 video | 💲 Q3 Turbo · 💲 1080p tiers · credit rate unpublished | 📄 6 | 🆕 minor | 🔴 1 likely (re-checked) · ⚠️ 4 |
| pixverse | 7 video | ✅ 5/5 published (v3.5, v4.5 unpublished) | 📄 3 | 🆕 V5.5/V5.6, fusion, extend | ✅ |
| replicate | 10 image, 1 video | ✅ 10/11 · SDXL marginal | 📄 6 | 🆕 12 | ⚠️ 3 |
| fal | 51 image, 5 video, 6 3D | ✅ image 49/51 (2 inferred) · 3D 6/6 · 💲 Max video promo | 📄 7 | ⏳ AuraFlow (recorded) · 🆕 ~100 | ⚠️ 4 minor |
| tripo | 1 image, 2 3D | ✅ 3/3 | ✅ | 🆕 6 | ⚠️ 1 minor |
| meshy | 2 3D | credits ✅, dollars unpublished | 📄 1 | 🆕 6 | ⚠️ 1 |

## 1. Billing below the vendor's cost

Changing a carried model's price is your decision (`provider-standards`, "Decisions that
belong to the user"). Each item gives my recommendation; none has been applied. Every fix
below fits the existing pricing rules, so none needs core work. A rule can key on a param,
on `megapixels`, or on a reference role through `references.<role>`
(`capabilities/pricing.rs:43-54`).

1. **GPT Image 2's quality rules are wrong (re-checked).**
   - **Ours:** `models/openai.yaml:195-198` bills low at ×0.5 and high at ×2.0 of the
     $0.053 medium image. The comment's "low half, high twice" has no vendor source.
   - **Vendor:** OpenAI's GPT Image 2 row at 1024×1024 reads "Low $0.006 · Medium $0.053 ·
     High $0.211" (https://developers.openai.com/api/docs/guides/image-generation.md,
     "Historical output costs").
   - **Effect:** a high-quality image bills $0.106 against a $0.211 cost, short $0.105 per
     image. A low-quality image bills $0.0265 against $0.006.
   - **The 2.5 rows are right:** their multipliers come from OpenAI's token counts.
   - **Recommend:** low ×0.1132 and high ×3.981.
2. **Ideogram 3.0 with a character reference (re-checked).**
   - **Ours:** the three 3.0 rows accept one `character` reference
     (`models/ideogram.yaml:52, 104, 156`) and bill a flat $0.06 / $0.03 / $0.09.
   - **Vendor:** Ideogram's catalog bills `api_ideogram_v3_character_generation` at
     DEFAULT "0.15", TURBO "0.1", QUALITY "0.2" USD
     (https://ideogram.ai/api/usage-pricing/catalog).
   - **Effect:** each character request is short $0.07–$0.11.
   - **Recommend:** `{ when: { references.character: '*' }, usd: <tier> }` on each row.
3. **Gemini 2.5 Flash Image (re-checked; open since 2026-10-12).**
   - **Ours:** $0.02 (`models/google.yaml:155`).
   - **Vendor:** Google lists "$0.039 per image" standard and $0.0195 batch
     (https://ai.google.dev/gemini-api/docs/pricing.md, which redirects to
     aistudio.google.com).
   - **Effect:** every image bills at about half its cost until the model retires on
     2027-03-15.
   - **Recommend:** $0.039.
4. **Vidu resolution tiers and Q3 Turbo.**
   - **Ours:** the rows bill only the 720p per-second rate, so 1080p bills as 720p:
     - Q3 Pro at 1080p: "24 credits/second", $0.60 per 5 s, billed $0.50
       (`models/vidu.yaml:250-255`);
     - Q2 Pro Fast at 1080p: "16 credits for the 1st second, +4 credits/second
       thereafter", $0.16, billed $0.08 (`:301-308`);
     - Q3 Turbo at 1080p: 13 credits/s, $0.325, billed $0.275.
   - **Q3 Turbo's base is still $0.275** against "12 credits/second" at 720p = 60 credits
     (`:194-200`, open since 10-12). At our $0.005 per credit that is $0.30.
   - **Vendor:** https://platform.vidu.com/docs/pricing.md.
   - **Recommend:** `resolution`-keyed rules, and a Q3 Turbo base of 60 credits.
   - The credit rate itself is in §2.
5. **Luma Ray 2 at 1080p and 4K (known gap, now with vendor numbers).**
   - **Ours:** `luma/ray-2` and `luma/dream-machine` (which sends Ray 2) bill a flat
     $0.40 / $0.35.
   - **Vendor:** "Create cost (for 5s Ray 2 video) … 1080p 0.95 … 4k 1.05"
     (https://docs.lumalabs.ai/changelog/upscale.md).
   - **Recommend:** rules for those two cells. Luma publishes no 9 s or 540p/720p price.
6. **fal FLUX 3 Image Klein Base above 1 MP (inferred).**
   - **Vendor:** fal's page says both "$0.035 per megapixel of output" and "up to
     1024x1024 counts as 1 megapixel, 2048x2048 as 1.56".
   - **Ours:** we follow the second, so 2048² bills $0.0546. The per-megapixel reading
     gives $0.147.
   - **Recommend:** settle it with one billed run before changing anything.

## 2. Billing above the vendor's cost, or unpublished

These cost litegen nothing. They are listed so the published prices stay honest.

- **fal FLUX 3 Max video:**
  - **Ours:** the $0.85 list price, 5 s × $0.17.
  - **Vendor:** fal runs "50% off … through November 22, 2026", which makes the default
    request $0.425 (https://fal.ai/models/blackforestlabs/flux-3-max/text-to-video/llms.txt).
  - **Recommend:** keep list. The promotion ends in two weeks.
- **Vidu's credit rate:**
  - **Ours:** every row converts at $0.005 per credit.
  - **Vendor:** Vidu publishes only "1 | 0.03125" RMB per credit (re-checked). At this
    week's rate (0.14987 USD per CNY, the rate TokenHub's rows use) that is about $0.0047.
  - **Effect:** about 7 % over cost.
  - **Recommend:** state the RMB source and the conversion in `models/vidu.yaml`. Keep
    $0.005 or convert, your call.
- **OpenAI non-square sizes (inferred).** `multiply_per_megapixel_above` scales the
  price with pixels. OpenAI's tokens follow an aspect grid instead: GPT Image 2 medium at
  1024×1536 is $0.041 against $0.053 square (re-checked), and we bill $0.0795.
- **Smaller over-billing:**
  - Gemini 3.1 Flash Lite Image bills $0.034 against $0.0336.
  - TokenHub Seedream 5.0 Pro bills the 0.6 CNY tier. A non-square 2K output of at most
    2.61 MP costs 0.3 CNY (inferred).
  - fal FLUX1.1 Ultra Redux bills from a "2752 × 1536" BFL sample that is no longer
    published, up to $0.011 over (inferred).
- **Replicate SDXL:**
  - **Vendor:** the estimate is now "approximately $0.0043 … 232 runs per $1".
  - **Ours:** $0.005 covers the default, but no longer covers the 100-step maximum the
    comment claims (about $0.0086, inferred).

## 3. Requests that may fail, and wire-format mismatches

**Likely to fail.** Each was checked against the vendor's schema; one live call each would
settle it.

1. **Vidu Q3 Turbo reference-to-video (re-checked).**
   - **Ours:** the adapter sends `images` to `/ent/v2/reference2video`
     (`video/vidu.rs:174-214`), and the row allows 1–7 `reference` images
     (`models/vidu.yaml:228-236`).
   - **Vendor:** for `viduq3-turbo`, Vidu documents only "Subjects-Reference to Video",
     where `subjects` is "Required". The `images` form is documented for `viduq3` only
     ("Accepted values: `viduq3`")
     (https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/reference-to-video.md).
   - **Fix:** send the subjects form, or drop the `reference` role from Q3 Turbo.
2. **Recraft V2/V3 `style` default (re-checked).**
   - **Ours:** three rows use the old base-style ids, and each default goes out on every
     request:
     - `recraftv3` and `recraftv2` default to `realistic_image`, and also offer
       `digital_illustration`, `vector_illustration` and `any`
       (`models/recraft.yaml:28-32`);
     - `recraftv3_vector` defaults to `vector_illustration`.

     `recraftv2_vector` already uses curated names.
   - **Vendor:** neither Recraft's model page nor its styles page mentions these values
     any more. Generate's `style` is now "A curated style, e.g. `Photorealism`,
     `Illustration`, `Vector art`", default `Recraft V3 Raw`
     (https://www.recraft.ai/docs/api-reference/models/recraft-v3.md, `styles.md`).
     Whether the API still accepts the old ids is unknown.
   - **Fix:** switch to curated names. This is a breaking API change for callers, so
     decide whether to accept both.
3. **TokenHub Kling Image O1 and 3.0 Omni, text only (inferred).**
   - **Ours:** the rows send no `aspect_ratio` by default (`models/tokenhub.yaml:340-343`,
     `:371-374`), so TokenHub falls back to `auto`.
   - **Vendor:** the guide says "文生图场景（未传 image_list）不支持 auto", meaning
     text-to-image without `image_list` doesn't support `auto`.
   - **Fix:** a `16:9` default.
4. **MiniMax H3-Lite reference-to-video without a ratio (inferred).**
   - **Ours:** the request reaches MiniMax as `adaptive`.
   - **Vendor:** "`MiniMax-H3-Lite` does not support `adaptive` yet"
     (https://platform.minimax.io/docs, v2-create).
   - **Fix:** a concrete default ratio.

**Verified mismatches, low impact:**
- **Ideogram 4.5** has no prompt cap. The vendor caps at `maxLength 10000`, so longer
  prompts 400 upstream (`models/ideogram.yaml:336`).
- **Ideogram 3.0 sends a JSON body** to a multipart-only endpoint. Ideogram's own sample
  does the same, so the risk is low.
- **Ideogram's multipart encoding** of `style_codes` and `color_palette` is undocumented.
- **Recraft remove background:** the docs say the field is `file`, the OpenAPI says
  `image`, and we send `file`.
- **Vidu Q1:**
  - the allowlist passes `movement_amplitude` and `bgm` to text2video, which has neither;
  - `audio` is sent on the reference `images` form, which has no `audio` field;
  - Q1 and Q2 Pro Fast image-to-video have no documented operation;
  - base64 refs are always labelled `image/png` (`vidu.rs:78`).
- **Content-type labels:**
  - Replicate labels every image `image/png`, but the FLUX and SD3 models default to WebP
    (`image/replicate.rs:309`, open since 10-12).
  - BFL labels `image/png` even when JPEG or WebP is asked (`image/bfl.rs:263`).
- **Bounds:**
  - Replicate AnimateDiff seed: the vendor's maximum is 2147483647, ours 4294967294.
  - Replicate FLUX 3 Image allowlists `safety_tolerance` without the vendor's maximum of 4.
  - BFL `safety_tolerance`: the spec says 0–7, the prose says 5 (vendor contradiction,
    known).
- **Under-advertised limits:**
  - Meshy rig: 320,000 faces against Meshy's "380,000 triangles" (also pinned in
    `tests/model3d_conformance.rs`).
  - fal AuraFlow `guidance_scale` is capped at 10 against 20.
  - FLUX 3 Max offers 2 of 4 resolutions; 2K and 4K are unpriced at fal.
- **Stability:** rows named "Stable Diffusion 3 Large/Turbo" send `sd3.5-large` and
  `sd3.5-large-turbo`.
- **Tripo:** its files page says "JPEG, PNG", its multiview page "PNG, JPEG, WebP", and we
  upload WebP.

**Checked and fine:**
- Kling's host and Bearer auth.
- Runway's `X-Runway-Version: 2024-11-06` and its ratio enums.
- Luma's Dream Machine schema.
- MiniMax and Hailuo enums.
- TokenHub's host, paths and status enums.
- Hunyuan's three Tencent actions, all still documented.
- fal's `fal.run` host, `Key` auth and its error envelopes.
- Tripo's and Meshy's OpenAPI and statuses.
- Bedrock's InvokeModel bodies.

The two 2026-10-12 issues re-checked here are closed:
- Seedance `duration` is now an int, rounded.
- PixVerse no longer allowlists fields its schemas lack.

## 4. Citations in comments

**Broken in bulk:**

| Vendor | What broke | Where | Replacement or note |
|---|---|---|---|
| Stability (re-checked) | `https://docs.stability.ai/openapi.json` (and `openapi-v1.json`) now 302 to a login page | 13 `@see` in `models/stability.yaml`, 8 in `image/stability.rs` | v2: `https://api.stability.ai/v2alpha/openapi` (200, unauthenticated, `version: v2beta`). v1: the markdown twins (`api-reference/sdxl-10/*.md`) |
| Google | All 14 `ai.google.dev` URLs 301 to `aistudio.google.com`; `ai.google.dev/api/imagen` is dead | `models/google.yaml`, `image/google.rs:82,177,198,351` | Content intact at the new domain; the image guide now leads with the Interactions API (§5) |
| Vidu | Legacy `/docs/<page>.md` URLs now serve other models' pages; 6 quoted strings are gone, among them an adapter `Verbatim` | `models/vidu.yaml`, `video/vidu.rs` header | `/docs/api-reference/video-models/<model>/<operation>.md` |
| Vidu | `/docs/model-map` is a soft 404, and is Vidu 2.0's `docs_url` | `models/vidu.yaml:55` | `/docs/overview/model-map` |
| Replicate | Four HTTP-reference anchors moved | `image/replicate.rs`, `video/replicate.rs` | `#predictions.create`, `#models.predictions.create`, `#predictions.get`, `#account.get` |
| ByteDance | Nine numeric ModelArk URLs now 302 to slugs; image adapter "Verbatim" quotes are paraphrases; the video adapter cites a third party (apidog) | `image/bytedance.rs`, `video/bytedance.rs` | Cite the slug pages; replace the third-party citation |

**Price service `pricingUrl` (`apps/price-api/src/providers/<p>/<p>.config.ts`):**

| Provider | Problem | Replacement |
|---|---|---|
| Luma | Redirects to the Agents app, no prices | `https://lumalabs.ai/api` |
| Runway | Points at consumer plans; Runway says "Credit costs are documented only in /guides/pricing.md" | `https://docs.dev.runwayml.com/guides/pricing.md` |
| Ideogram | A JS shell | `https://ideogram.ai/api/usage-pricing/catalog` |
| Stability | A redirect | `https://docs.stability.ai/pricing` |

**Prices with no citation:**
- Stability's four generate rows;
- Recraft V3, V3 Vector and V2;
- Ideogram's three 3.0 rows;
- BFL `flux-dev`, whose $0.025 is on `https://bfl.ai/pricing` but not on the cited docs table.

**Comments that state something false:**
- BFL is called "image-only", but BFL ships FLUX 3 Video (`image/bfl.rs:20`,
  `models/bfl.yaml:3`).
- The Kontext rows say BFL's default output is JPEG; the spec says PNG.
- MiniMax's China host is named `api.minimaxi.com`; the docs moved to
  `platform.minimax.cn` with `https://api.minimax.cn`.
- `model3d/fal.rs` says fal keeps payloads "30 days"; fal now says 90.
- Ideogram's Nano Banana rows say "No limit on Ideogram's side"; the cap is 10000.
- The Replicate rows say "default webp (litegen says jpeg)" next to `default: webp`.
- `vidu.mjs` says "vidu2.0 has no endpoint page".
- `image/bedrock.rs:95-97` attributes prices to pages that carry none.

**Everything else:** the rest of the 78 findings are stale quotes, moved anchors, and old
or missing citation dates. Kling's capability map and PixVerse's move to enums account for
several. The scorecard counts them per provider, and the group reports list each one with
the vendor's current words. Several YAML price citations are dated 2026-09-11 for prices
that have changed since. Most are Vidu's.

**Verified intact:**
- every TokenHub citation (15 pages);
- every fal image `@see`, `docs_url` and price string (about 276 pages);
- every Tripo verbatim;
- Runway's spec and pricing quotes;
- Kling's prices and request schemas;
- Hunyuan's actions;
- Bedrock's request pages.

## 5. Models

### Shutdowns

| Model | Status | Note |
|---|---|---|
| `google/gemini-2.5-flash-image` | Retires 2027-03-15 | `retires_at` set; successor `gemini-3.1-flash-lite-image` |
| fal `fal-ai/aura-flow` | "deprecated" in fal's registry, no date (re-checked) | The row records `deprecated_at: 2026-10-27`. The drift check's "missing" is its extractor dropping deprecated endpoints, not a removal |
| Luma Dream Machine API | Now called "the legacy Dream Machine video API" | All 5 Luma rows use it; no end date published |

Nothing carried is past a shutdown date. Shutdowns of models we don't carry, for context:

| Date | Shutdown | Successor |
|---|---|---|
| 2026-11-11 | ByteDance Seedance 1.5 Pro | Seedance 2.0 Mini, carried |
| 2026-11-12 | TokenHub `hy-image-v3` | — |
| 2026-11-24 | TokenHub `seedream-image-v5.0-lite` | — |
| 2026-11-30 | Meshy `meshy-t1` / `lowpoly` | — |
| 2026-12-01 | OpenAI `gpt-image-1.5`, `gpt-image-1-mini`, `chatgpt-image-latest` | `gpt-image-2.6-*` |
| 2026-12-03 | Google `gemini-omni-1.1-flash` | routes to 1.2 |

Three earlier shutdowns are confirmed:
- AWS Nova Reel and Nova Canvas reached end of life on 2026-09-30. `video/bedrock.rs`
  builds only Nova Reel and has no row.
- Every Veo model on the Gemini API is shut down.
- OpenAI's Videos API and Sora shut down on 2026-09-24.

### New models we don't carry

Direct vendors:

| Vendor | Models |
|---|---|
| OpenAI | `gpt-image-2.6-sunburst` and `-flare` (the changelog's top entry, same $8/$30 token rates). Our 2.5 quality factors would under-bill 2.6 by about 3.3× at medium |
| Google | `gemini-omni-1.2-flash`: video, GA, about $0.07/s at 720p. Needs the Interactions API, which the adapter doesn't speak. Google's image guide also now leads with the Interactions API (`generateContent` is still served) |
| Kling | 4.0 Flash: 720p, 3–20 s, "0.6 Units ($0.094) /s", one path arm away. Also 3.0 Omni and O1 video, Image 3.0 Omni and Image O1 ("16 Units ($0.056)" at 4K), and the 2.1 multi-image endpoint |
| Luma | The Agents API (`docs.agents.lumalabs.ai`): `ray-3.2` (5 s: $0.15 / $0.30 / $1.20 at 540p / 720p / 1080p), `uni-1`, `uni-1-max` |
| BFL | FLUX 3 Video, direct. Its qhd/uhd rates contradict each other between two BFL pages. Also FLUX.2 klein 4B and Fill, Expand and the FLUX Tools. The "needs mask inputs" skip is weaker now that fal's fill, erase and outpaint rows exist |
| Stability | SD3.5 Flash: 2.5 credits, but not yet in the `model` enum. Also 15 Stable Image endpoints the drift check never surfaces: erase, recolor, relight, control, upscalers, SF3D, SPAR3D |
| ByteDance | Seedance 2.5 ($10.70 per M tokens), Seedance 2.0 and 2.0 Fast, Seedream 5.0 (unpriced); all acknowledged |
| MiniMax | H3-Regeneration and Context-IR endpoints; `prompt_expansion_mode` on H3-Max |
| PixVerse | V5.5 and V5.6, skipped as "legacy" though PixVerse's capability map lists V5.6 as current and both are priced (re-decide). Also fusion (reference-to-video, V6 Omni with up to 10 refs), Video Extend and Video Expand |
| Recraft | 11 endpoints we carry only via fal or not at all: image-to-image, inpaint, outpaint, generate background, erase, variations…; params `style_match` and `style_reference_urls` |
| Ideogram | `nano-banana-2-1`, now priced at $0.0336 / $0.0504 / $0.0756, so its skip reason is stale. Also 3.0 Flash ($0.03), 3.0/4.0 transparent, `ideogram-vector-1`, 4.5 replace background, and resold video |
| Tripo | Direct generation: H3.1, v3.0, v2.5, P1, P2, and Gaussian splat |
| Meshy | Direct generation: 6 models (Meshy 6 and 7 lines…), plus Text to Motion |
| TokenHub | HiDream O1 1.5 (0.6 CNY) and its edit model (0.65 CNY), both listed and priced with no API guide yet. Kling Video v4 Flash (1 CNY/s at 720p). 3D: HY-3D 3.0/3.1/Express, resold Tripo, Hi3D, Rodin and Hy-World, all acknowledged. README's 3D line understates this |

Hubs: under the 2026-11-04 policy, a resold model is carried as its own row:

| Hub | New routes |
|---|---|
| Runway | 23 new ids in the OpenAPI, priced in the group report: Kling 4 Flash, Kling 3.0 and O3, GPT Image 2.6, Gemini Omni 1.2, Ideogram 4.x, Seedream 5 Flash, Bria FIBO, Recraft V4.1… Plus 24 `RESOLD_IDS` skipped as "carried direct", a reason the policy overturned |
| Leonardo | The v2 API (`/api/rest/v2/generations`) lists about 85 ids, about 40 of them resold: Kling, Seedance, Nano Banana, GPT Image, FLUX 3, Ideogram, Recraft, Wan, Hailuo, LTXV, Meshy 7, Rodin |
| Replicate | 12 BFL models: FLUX1.1 Ultra, Kontext pro/max, Krea dev, FLUX.2 pro/max/flex/dev/klein, FLUX 3 video |
| fal | About 40 image endpoints (the Bria/FIBO family, legacy Bria, BiRefNet v1) and about 60 3D endpoints (Hunyuan3D v2 turbo/mini, v3, v3.1 rapid; Rodin v2/v2.5; TRELLIS multi and TRELLIS.2; Meshy 7.1; Tripo H3.1/P2; Hitem3D; SAM 3D), with today's prices in the group reports. Also FLUX 3 edit and extend video (backlogged 11-04; the reason still holds) |
| Bedrock | Luma Ray 2 (`luma.ray-v2:0`, us-west-2, $0.75/s at 540p and $1.50/s at 720p), and Bedrock's copies of the four Stability edit tools we carry direct |

## 6. Drift tooling

These fixes are for the next weekly run.

**FAILED sources, and where their content moved:**

| Source | Cause | Fix |
|---|---|---|
| `bytedance/image-api` | The heading is now `### Body parameters` under `## Request parameters`, at the same URL | Update the heading the extractor looks for |
| `openai/sdk-spec` | The official file (still at its URL, last commit 2026-11-08) now uses YAML explicit keys (`? /organization/workload_identity_providers/…`) that `extract.mjs`'s `parseYaml` can't read | Read `openai-openapi/openapi.json`, the same version, 2.3.0 |
| `stability/openapi-v1` | The login wall in §4 | Use the v2 spec and the markdown twins |

**Also:**
- Kling's `pages` source works only through a redirect. Use
  `https://kling.ai/document-api/llms.txt`.
- **Orphan snapshots:**
  - `snapshots/hunyuan/tokenhub-hy-{image,video}.txt`: the sources moved to
    `tokenhub.mjs` on 2026-11-04;
  - `snapshots/leonardo/kling21-guide.txt`.
- **Most "changed" snapshots are extractor changes** from the uncommitted definition edits:
  - BFL +492 lines and Ideogram +330 come from new carried models;
  - fal is regrouped;
  - Replicate and Bedrock got new schema pages.

  Recraft's +701 is vendor-side: the spec added `application/json` beside multipart.
  The weekly run reviews all of them whole.
- **Skip reasons the hub policy overturned:**
  - Runway `RESOLD_IDS`;
  - Bedrock Luma Ray 2;
  - Replicate FLUX;
  - Ideogram `nano-banana-2-1`;
  - PixVerse V5.5 and V5.6;
  - BFL Fill/Expand.
- `dola-seedream-5-0-pro-260628` is both carried and acknowledged in `bytedance.mjs`.

## 7. Corrections: claims checked and rejected

- **"Recraft and Ideogram forward `n`, so the vendor bills N images."** This can't happen.
  The validator refuses `n > 1` on every image request, strict or lax
  (`api/middleware/validator.rs:1456`, called at `:272`). Both adapters' `n` handling is
  dead code.
- **README's "any fal video endpoint (`fal/video`)" and "any Replicate video model
  (`replicate/video`)".** Both rows are pinned:
  - `fal/video` sends fal's LTX Video endpoints at $0.02 (`extra_allowlist: []`,
    `video/fal.rs:217`);
  - `replicate/video` sends one AnimateDiff version at $0.10.

  Billing is right. The README text and the "(generic)" display names mislead, and so does
  the price service's note calling `fal/video` "price depends on model".
- **Kling auth.** It is a Bearer API key since 2026-11-03, not the JWT the audit brief
  assumed.
- **Vidu's pricing URL.** The catalog and the price service both cite
  `https://platform.vidu.com/docs/pricing.md`, which serves the same page as
  `/docs/overview/pricing.md`. There is no mismatch.

## 8. What this audit could not verify

**Prices nobody publishes openly:**

| Provider | Gap |
|---|---|
| Leonardo | All 5 rows; prices are only in its logged-in calculator |
| PixelLab | Per size; only a billed call's `usage.usd` reports it |
| Meshy | Dollars per credit; API credit packs are behind a login |
| MiniMax | T2V-01-Director ($0.43) and S2V-01 ($0.65) have no price anywhere. The drift definition already skips the other `*-01` models as "superseded and unpriced". Recommend: retire both, or cite a source |
| Hunyuan | Image-to-video: Tencent's table says 1.2 CNY per credit, its worked example 2.0. At 2.0 the 720p row would be about $1.50, not $0.894 |
| fal | SDXL, AuraFlow and BiRefNet v2 publish "$0 per compute seconds", so ours are estimates |
| Luma | Photon, Ray Flash 2, and Ray 2 below 1080p |
| PixVerse | v3.5 and v4.5 (known) |

**Settled only by a live call:**
- the requests that may fail in §3;
- fal Klein Base's price above 1 MP;
- whether `gpt-image-2.6` honours `xhigh` and `max`.

**Pages a script can't read:**
- `platform.openai.com` (403 to scripts);
- Tencent's TC3 signing page (bot wall);
- Kling's HTML pages (JS shells; the `.md` twins were read instead);
- Leonardo's pricing (403).

## Evidence

Group reports, with full per-row price and model tables, are in
`.superpowers/sdd/2026-11-09-provider-audit/`:
- `fal-image.md`
- `fal-video-3d.md` (fal video and 3D, Tripo, Meshy)
- `tokenhub.md`
- `vidu-pixverse.md`
- `minimax-bytedance-hunyuan.md`
- `bfl-replicate.md`
- `recraft-ideogram.md`
- `stability-bedrock-pixellab.md`
- `kling-luma-runway.md`
- `google-openai-leonardo.md`

The drift run (JSON, fresh snapshots, diffs) is in `drift-run/`. The pages the auditors
fetched sit in the session scratchpad, which a restart clears. Every claim above names its
URL, so it can be fetched again.
