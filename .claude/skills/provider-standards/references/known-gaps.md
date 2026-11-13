# Known gaps and legacy patterns

This file lists core behaviors and legacy adapter patterns that a new
integration has to work around or report, re-checked against the code on
2026-11-03. Don't copy them, and don't paper over them inside a single adapter.

Several sessions are changing this code in parallel. Re-check an entry before
you rely on it. Most entries come with a grep that shows the current state,
because hand-made lists go stale. When you find an entry fixed, delete it from
this file.

## Contents

- [Billing](#billing)
- [Surfaces that lag the catalog](#surfaces-that-lag-the-catalog)

## Billing

- **Some variants bill at the default request's price.** Rules price most
  rows' variants (docs: `catalog-yaml.md` "Pricing"), but these can't be
  priced by variant today: an edit's input megapixels on BFL FLUX.2 (BFL
  bills them at its per-MP rate; a reference's size is unknown at the
  quote — output megapixels are priced by range since 2026-11-03), a FLUX 3
  Image Klein edit that keeps a large reference's size (its tier, up to
  $0.036; billed at 1K), FLUX 3 Image `resolution: match_input` (billed at
  its 2K ceiling, $0.10), FLUX 3 sizes within a few percent of a tier edge
  (BFL's equal-area bins vary by ratio; litegen's edges are 768², 1024²,
  1536², 2048²), Leonardo's rows (API prices only in its logged-in calculator), Luma ray-2
  and dream-machine below 1080p and at 9 s, ray-flash-2 and photon (Luma's API page
  lists only Ray3.2 and Uni-1.1; Ray 2's 5 s 1080p and 4k prices, from its
  upscale changelog, are rules since 2026-11-09), PixVerse v3.5 and v4.5 (off PixVerse's price page),
  Stability SDXL above 30 steps (0.9 credits x steps/30), PixelLab (a range
  by size, billed at its top), Vidu Q2 Pro at 540p for 1 s, and TokenHub's
  rows (TokenHub bills the tokens each answer reports, `tokenhub_usage`;
  litegen bills its price table's per-output figure, so input-image tokens
  beyond it go unbilled). *When a
  vendor publishes the price:* add the rule and a case to
  `varying_rows_price_the_variant_asked_for`.

## Surfaces that lag the catalog

`cargo clippy --all-targets --all-features -- -D warnings` passes (since
2026-11-03; `src/mesh/`'s style lints are allowed at `pub mod mesh;`). Keep it
so: a new finding is fixed, not allowed.

- Bedrock video (Luma Ray 2) writes its MP4 to the customer's S3 bucket,
  named by `s3_output_uri`: the operator's provider options, or a
  top-level `s3_output_uri` in a BYO credential stored through the API. The
  dashboard's credential form has no field for it yet (its fields go into each
  `credential_sets` entry, and `CredentialEntry` has no such field), so a
  dashboard-only BYO Bedrock credential can't run Ray 2: the request is
  refused before any call, naming the field. Adding it is an OpenAPI change
  (`CredentialEntry`) plus a `provider_catalog()` field.
- The landing diagram (`infra-flow/nodes.ts`) shows 17 of the 22 providers:
  Tripo, Meshy, PixelLab, TokenHub and Bedrock have no node, because a node needs a hand-placed
  position and a logo, both the user's call, and its shader streams only
  image and video tiles. They are on the Providers list.
- Providers registered with no models in a family: OpenAI and Google video.
  `proxy/registry.rs` still builds them, so an operator's runtime model can
  use them. (Bedrock video serves Luma Ray 2 since 2026-11-10, its image
  adapter Stability's models since 2026-11-04.) Google's video adapter has
  no current model: every Veo model on the
  Gemini API is shut down (the 3.1 previews on 2026-10-22, Veo 3.0 and 2.0 on
  2026-06-30, per ai.google.dev/gemini-api/docs/deprecations, read
  2026-11-03), so a runtime Veo row fails at Google; the named replacement,
  `gemini-omni-1.1-flash`, needs the Interactions API, which the adapter does
  not speak.
