# Provider model drift check

Vendors ship new models, rename ids, retire old ones and change request
parameters without telling us. This check compares what every provider
**publishes** against what we **carry** in `models/*.yaml`, once a week.

```bash
node scripts/model-drift/check.mjs            # report + refresh snapshots/
node scripts/model-drift/check.mjs --json     # machine-readable (the /model-drift skill uses this)
node scripts/model-drift/check.mjs --provider runway,openai --no-write
node --test 'scripts/model-drift/test/*.test.mjs'
```

In Claude Code, run **`/model-drift`** instead. The skill
(`.claude/skills/model-drift/SKILL.md`) runs the check, investigates anything
broken, fixes the definitions, and brings the findings back to you.

No API keys are needed. Every source is a public spec or docs page.

## Exit codes

| code | meaning | what happens next |
|---|---|---|
| 0 | clean | nothing |
| 1 | drift: new, missing, deprecated upstream or changed items, or a source with no baseline | review each item (below) |
| 2 | needs investigation: a source failed, or a definition is out of sync with `models/*.yaml`; or a bad command line | fix the definition first (the skill does this); fix the command |

A bad command line (a flag without its value, `--provider=` or `--provider ,`
naming no provider, an empty `--snapshots`, an unknown flag such as
`--dry-run`, or a stray word) stops with a usage message before anything is
fetched or written. It never falls back to checking every provider into
`snapshots/`.

## What the report means

- **`new upstream`**: the vendor lists a model id we don't carry and haven't
  acknowledged. Either add it to `models/<provider>.yaml`, with typings and
  pricing verified against the vendor docs, or add it to the definition's
  `acknowledged` list with the reason we skip it. Which one is a product
  decision.
- **`missing upstream`**: a vendor id we map no longer appears in any source.
  A model maps to every id its adapter sends (one per endpoint it routes to,
  such as text-to-video and image-to-video), so one endpoint going away is
  reported even while the others remain. Check the vendor's changelog or
  deprecations page. Retired means remove or replace the model, or drop the
  capability that used the endpoint. Renamed means fix the mapping, and the
  adapter too if it sends the old id. A model a source lists as deprecated
  (still served) is not missing; see the next item.
- **`deprecated upstream`**: a source lists a vendor id we map as deprecated
  but still served (fal's registry status `deprecated`, through the source's
  `deprecated` reader), and `models/*.yaml` records neither `deprecated_at`
  nor `retires_at` for the model. Set `deprecated_at` to the date the vendor
  deprecated it, and `replaced_by` when the vendor names a successor (whether
  to move callers to it is a product decision). Once either date is set the
  item stops appearing, and the model stays served.
- **An announced, future shutdown:** set the row's `retires_at` to the date,
  and add the same id and date to `SCHEDULED_SHUTDOWNS` in
  `litegen-core/tests/catalog_conformance.rs` (a test requires the two to
  match). The model stays advertised while the vendor still serves it. From
  the day after the date, the test fails until the model is removed. For fal,
  a shutdown date or replacement it announces shows up as a `changed`
  carried-endpoint snapshot (`retires_at …`, `replacement …` in the
  endpoint's heading).
- **`changed`**: a source's snapshot differs from the committed one. Run
  `git diff scripts/model-drift/snapshots/` to see what changed upstream, such
  as new enum values, new bounds or new endpoints. Compare that against the
  typings in `models/<provider>.yaml`, then commit the snapshot to accept it.
- **`no baseline`**: the snapshots folder the run used had no earlier snapshot
  of this source (a new source, a new provider, or a fresh `--snapshots`
  folder), so nothing was compared. The run wrote the first one, unless
  `--no-write`. Read the whole file (`<provider>/<key>.txt` in that folder)
  against `models/<provider>.yaml`, as a diff would show nothing, then commit
  it. Committed snapshots are a reviewed baseline, so committing one
  unreviewed would bury any mismatch it shows.
- **`FAILED <source>`**: the source didn't give a usable list. Possible causes:
  - an HTTP error
  - an HTML page where a spec or markdown was expected (the page moved, or now
    serves a JS app shell)
  - a parse or extractor error
  - no ids found
  - **`(canary)`**: sources returned ids, but none of ours. The page was
    restructured.
  - **`(config)`**: the definition and `models/*.yaml` disagree.
  - **`(typings)`**: a carried vendor id appears in no typings snapshot, so
    nothing compares that model's request schema with `models/*.yaml`. Add or
    re-scope a source that covers it. Only when the vendor publishes no
    request schema at all, list the model in the definition's `untyped` with
    the reason.

  The vendor moved something, so the definition needs updating. Never "fix" a
  failure by deleting the source or by loosening the pattern until it matches
  anything.

## How it works

`providers/<provider>.mjs` holds one definition per vendor:

```js
export default {
  provider: 'runway',
  // litegen model id → the id the vendor documents (what the adapter sends);
  // an array when the adapter routes the model to several endpoints — every one must stay listed
  models: { 'runway/gen4-turbo': 'gen4_turbo' /* … every model in models/runway.yaml */ },
  sources: [{
    key: 'openapi',                                    // names snapshots/runway/openapi.txt
    url: 'https://docs.dev.runwayml.com/openapi.json',
    expect: 'json',                                    // json | text | html (HTML where text was expected = soft 404)
    extract: (spec) => openapiModelIds(spec, { paths: ENDPOINTS }),   // or a RegExp over the body
    snapshot: (spec, _body, { ours }) => openapiExcerpt(spec, { paths: ENDPOINTS, ours }),
  }],
  acknowledged: [{ pattern: /-\d{4}-\d{2}-\d{2}$/, reason: 'dated snapshot alias' }],
  // only for a model whose vendor publishes no request schema (rare): litegen id → reason
  untyped: {},
  // only when the id search would take a model's typings from the wrong schema, or none: litegen id → source keys
  typingsFrom: {},
};
```

- **Scope ids to what our adapter can call.** Extract model ids from the
  request schemas of the endpoints the adapter uses, or from the model
  families we carry for aggregators like fal and Replicate. Don't pull every
  string on the page.
- **Every carried model needs typings coverage.** Each vendor id must appear
  in at least one typings snapshot (a non-index source with its own
  `snapshot`), or the run fails with `(typings)`. An id list or an index page
  does not count: it says the model exists, not what it accepts. A non-index
  source whose snapshot is a lineup table or an id list says so with
  `typings: false` (vidu's Model Map, Leonardo's `model-ids`). An index source
  whose snapshot is a request schema (its ids are field names, say) counts
  when it sets `typings: true`.
- **`typingsFrom` pins a model's typings to named sources** (litegen id →
  source keys). Use it only where the id search would find the wrong schema,
  or none:
  - a free-form model field, whose schema never names the id (Leonardo's
    `POST /generations` takes diffusion-xl's UUID in `modelId`);
  - a vendor id that another carried model sends to a different endpoint
    (`pixverse/v5-multi-transition` sends `v5`, as `pixverse/v5` does, to
    `multi_transition`).

  A pinned model's typings come from those sources only, and a pin doesn't
  require its vendor id: the model counts as covered once the pinned sources
  succeed. So if PixVerse dropped `v5` from `multi_transition`, the report
  would show `changed: multi-transition` (its `# model values` list), but no
  `missing` (`v5` is still listed for the other endpoints) and no
  `(typings)`. Read that diff. Every pinned key must be a typings source of
  the same definition, and every pinned model must be mapped; otherwise the
  run fails with `(config)`, and so does the test `every model in
  models/*.yaml is mapped…`.
- **Snapshots are typings evidence.** `openapiExcerpt` keeps structural keys
  only, since prose churns every week. For unions discriminated by `model`, it
  keeps only the branches for models we carry. `yamlPrune(markdownExcerpt(…))`
  does the same for Mintlify `.md` pages that embed OpenAPI. Sources without a
  `snapshot` fall back to their sorted id list.
- **`index: true` sources** list docs pages or endpoints (for example
  `llms.txt`). A new page shows up as a changed snapshot, never as a model id.
- **`deprecated: (parsed, body) => ids`** (optional, beside `extract`): the
  ids a source lists as deprecated but still served. They are kept out of
  `extract`'s ids, never read as missing, and reported as `deprecated
  upstream` until the catalog records the deprecation. fal's carried-endpoint
  sources use it for the registry status `deprecated`.
- **Source preference:** an official machine-readable spec or list first. Next
  is a docs site's markdown twin (`llms.txt` → `<page>.md`). Last is
  server-rendered HTML.

The test `every model in models/*.yaml is mapped…` fails when a model is added
without a mapping, so the check can't silently skip it.

## Adding a provider

1. Add `providers/<name>.mjs`. Map every model in `models/<name>.yaml`,
   mirroring the adapter's model-resolution code in
   `litegen-core/src/providers/{image,video}/<name>.rs`.
2. Run the check twice into one scratch folder:

   ```bash
   D="$(mktemp -d)"
   node scripts/model-drift/check.mjs --provider <name> --snapshots "$D"   # run 1
   node scripts/model-drift/check.mjs --provider <name> --snapshots "$D"   # run 2
   ```

   Run 1 must not exit 2. It exits 1 and lists every source under
   `no baseline`, since the folder starts empty. Run 2 must show neither
   `changed` (the snapshots are stable) nor `no baseline`.
3. Run the tests and commit the definition. Its snapshots get committed by
   the first `/model-drift` run, after that run reviews them against
   `models/<name>.yaml`.
