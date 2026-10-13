# Model drift and pricing evidence

Vendors add models, rename them, retire them, change request schemas and
change prices, and they don't tell us. The weekly `/model-drift` run
(`.claude/skills/model-drift/SKILL.md`, `scripts/model-drift/README.md`)
catches those changes one at a time, but only for what the integration hooked
up when it was built. This file is about building integrations so that drift
shows up early and is cheap to act on.

`scripts/model-drift/README.md` is the source of truth for the definition
format. It is being extended while this is written, so re-read it when your
work touches a definition.

## Contents

- [What the weekly run can see](#what-the-weekly-run-can-see)
- [Writing a definition](#writing-a-definition)
- [Verifying a definition](#verifying-a-definition)
- [Shutdown lifecycle](#shutdown-lifecycle)
- [Pricing evidence](#pricing-evidence)
- [Acting on drift decisions](#acting-on-drift-decisions)

## What the weekly run can see

| Drift | How it shows up | What you must have done when integrating |
|---|---|---|
| New vendor model | `new upstream` id | Scoped the sources to the families and endpoints you carry, so new ids in them appear and unrelated ids don't |
| Retired or renamed model | `missing upstream` | Mapped every vendor id the adapter sends, as an array for multi-endpoint models, so even one endpoint vanishing is reported |
| Schema change (enum, bound, field) | the snapshot is `changed` | Snapshotted each carried endpoint's request schema (typings coverage) |
| Price change | the snapshot is `changed`, or a manual comparison | Kept price lines in a snapshot where the fetched page has them, **and** quoted the price with URL and date in the YAML |
| A source moved | `FAILED`, `(canary)` | Picked a stable, machine-readable source |
| Definition out of sync with YAML | `FAILED (config)` and the definitions test | Mapped every catalog row |
| Model with no schema coverage | `FAILED (typings)` | Covered every carried id, or listed a schema-less one in `untyped` with a reason |

## Writing a definition

Each vendor has one file, `scripts/model-drift/providers/<p>.mjs`, covering
every family the vendor has:

```js
// <Vendor> — which sources, and why these. Mapping mirrors
// litegen-core/src/providers/{image,video}/<p>.rs `resolve_model`.
import { openapiExcerpt } from '../extract.mjs';

const MODELS = {
  'acme/photo-2': 'photo-2',                                   // what the adapter sends
  'acme/clip-1': ['clip-1-t2v', 'clip-1-i2v'],                 // one model, two endpoints
};
const ENDPOINTS = ['/v2/images', '/v2/videos/text', '/v2/videos/image'];

export default {
  provider: 'acme',
  models: MODELS,
  sources: [
    {
      key: 'openapi',                                          // → snapshots/acme/openapi.txt
      url: 'https://docs.acme.example/openapi.json',
      expect: 'json',                                          // json | text | html
      extract: (spec) => /* ids from the request schemas of ENDPOINTS */ [],
      snapshot: (spec, _body, { ours }) => openapiExcerpt(spec, { paths: ENDPOINTS, ours }),
    },
  ],
  acknowledged: [
    { pattern: /-\d{4}-\d{2}-\d{2}$/, reason: 'dated snapshot alias of a carried model' },
    { id: 'photo-1-mini', reason: 'skipped 2026-10-12: superseded by photo-2; user decision' },
  ],
  untyped: {},                                                 // only when the vendor publishes no request schema
};
```

- **Mirror the adapter.** The header comment says which adapter function the
  mapping mirrors. Each mapped value is exactly the string the adapter puts
  on the wire: the path segment, the `model` field, the enum value.
- **Write every catalog id as a quoted string literal** in `models`, such as
  `'acme/photo-2': 'photo-2'`. A Rust test,
  `every_model_is_mapped_in_its_drift_definition` in `catalog_conformance.rs`,
  checks the mapping by reading the `.mjs` text, so it fails on an id built
  from a template string or a loop.
- **Scope ids to what the adapter can call.** Pull ids from the request
  schemas of the endpoints the adapter uses, or, for aggregators such as fal
  and Replicate, from the families you carry. Never scrape every string on a
  page.
- **Prefer this order of sources:** an official machine-readable spec or list,
  then the docs site's markdown twin (`llms.txt`, then `<page>.md`), then
  server-rendered HTML. JS-rendered pages give you an empty shell, which
  shows up as `FAILED`.
- **Snapshot typings, not prose.** `openapiExcerpt` keeps structural keys and
  only the `model`-discriminated branches you carry.
  `yamlPrune(markdownExcerpt(…))` does the same for Mintlify `.md` pages that
  embed OpenAPI. Prose churns weekly, and a snapshot that churns trains
  people to ignore it.
- **Put price lines in the snapshot when the page you already fetch has
  them.** The Replicate definition's `priceLines` and Recraft's price table
  are the models. Don't add fetches just for prices against a rate-limited
  API; fal's definition keeps its fetches few for that reason.
- **Acknowledge with a reason and a date.** A skipped vendor model gets an
  `acknowledged` entry saying why ("skipped 2026-10-12: …"). Structural noise
  (dated aliases, `/stream` transport routes) gets a pattern.
- **Index sources** (`index: true`) list pages or endpoints, such as
  `llms.txt`. A new page shows up as a changed snapshot, never as a model id.

## Verifying a definition

```bash
node --test 'scripts/model-drift/test/*.test.mjs'
node scripts/model-drift/check.mjs --provider <p> --snapshots "$(mktemp -d)"   # run 1: no FAILED, exit ≠ 2
node scripts/model-drift/check.mjs --provider <p> --snapshots <same dir>       # run 2: no `changed`
```

The second run must show nothing `changed`. If it does, the snapshot isn't
stable: there is prose, ordering or a timestamp in it. Fix the snapshot
function.

Leave `scripts/model-drift/snapshots/` alone. Committed snapshots are the
reviewed baseline. The first weekly run after your change reviews your new
snapshot in full (an untracked snapshot gets a whole-file review) and commits
it. Committing an unreviewed snapshot buries any mismatch it shows.

## Shutdown lifecycle

1. **The vendor announces a date.** Add `(id, "YYYY-MM-DD", "what happens;
   successor")` to `SCHEDULED_SHUTDOWNS` in
   `litegen-core/tests/catalog_conformance.rs`, with an `@see` to the
   deprecations page. Prefix the row's description with `DEPRECATED: <date>`.
   The model stays advertised while the vendor still serves it.
2. **The day after the date,** `models_past_their_announced_shutdown_are_removed`
   fails. Remove the row and its drift mapping, and move the id to
   `no_retired_model_ids_are_advertised`. Run the consistency sweep in
   touchpoints.md, section C.
3. **If the vendor still lists the id,** acknowledge it so it doesn't come
   back as "new".

`scheduled_shutdowns_name_models_still_in_the_catalog` fails if you remove a
row but leave its schedule entry behind.

## Pricing evidence

The weekly run compares our prices with vendor pages by hand. Make that
comparison mechanical:

```yaml
    # "$0.055 per second (720p)" on text2video / img2video / start-end2video.
    # Default request (720p, 5 s): $0.055 × 5 = $0.275. 1080p is $0.12/s.
    # @see https://platform.acme.example/docs/pricing.md (2026-10-12)
    pricing: { base_cost_usd: 0.275 }
```

- **Quote the vendor's exact price string.** Next week's reader then diffs
  strings instead of re-deriving units.
- **Show the arithmetic** from the vendor's unit (per second, per megapixel,
  credits) to the default request.
- **Use one URL with a date.** Use the machine-readable pricing source if the
  vendor has one (`llms.txt`, `pricing.md`, a pricing API).
- **List the expensive variants** the row allows: higher resolution, longer
  duration, texture, add-ons. Flat billing under-charges them, and listing
  them makes the gap visible. Put clean multipliers in `variable_pricing` as
  well.
- **Mark an unverifiable price**: "price unverified: published only behind a
  login".
- **Changing a carried model's price is the user's decision.** Report both
  prices with the URL.

`apps/price-api` is a separate pricing service with its own seeds and
scrapers, and its own coverage gate (`npm run coverage:check`) against
`models/*.yaml`. It lags the catalog and is out of scope unless the user asks.
`base_cost_usd` in the YAML is what litegen-core actually bills.

## Acting on drift decisions

Drift decisions are recorded in `docs/superpowers/<date>-model-drift-decisions.md`,
as tick boxes the user fills in. When you implement one:

- An **added** model follows touchpoints.md, section A. Tick the box with the
  date and the ids you added.
- A **skipped** model gets an `acknowledged` entry with the decision's reason.
- A **removed** model follows touchpoints.md, section C.
- **Systemic issues** a decision mentions (section 6 of the first log) are
  core work. Don't fix them inside an adapter; report them.
