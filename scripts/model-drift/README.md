# Provider model drift check

Vendors ship new models, rename ids, retire old ones and change request
parameters without telling us. This check compares what every provider
**publishes** against what we **carry** in `models/*.yaml`, once a week.

```bash
node scripts/model-drift/pipeline.mjs         # the whole weekly run, on a copy of snapshots/ (start here)
node scripts/model-drift/check.mjs --json --snapshots "$D"   # the check alone, into a folder of your own
node scripts/model-drift/review.mjs --fresh "$D"            # what changed in "$D" against snapshots/, by section
node scripts/model-drift/quotes.mjs           # quoted vendor strings in models/*.yaml vs the pages they cite
node --test 'scripts/model-drift/test/*.test.mjs'
```

## The pipeline: one command

`pipeline.mjs` runs every step and prints what to do next, in order. It never
writes `snapshots/`: it copies them into `<out>/snapshots` and checks there
(other sessions may have edits in `snapshots/`, and a run rewrites every
snapshot it fetches). `<out>` defaults to
`.superpowers/sdd/<date>-drift/run-<time>/` (the local date and time), which
git ignores and a restart keeps.

| Step | Output | What you do with it |
|---|---|---|
| 1. check (`check.mjs` logic) | `report.txt`, `drift.json` | the findings below |
| 2. **repair** | the "Repair first" block | every FAILED source, before anything else (see below) |
| 3. review (`review.mjs`) | `review.md` | each changed snapshot section as JSON paths (`~ path: old → new`, `+ path[] value`), sections that only moved between files left out, and never-reviewed sections printed whole |
| 4. quotes (`quotes.mjs`) | `quotes.txt`, `quotes.json` | quotes no longer on their page (a price or rule changed: fix the row and the quote) and cited pages that died (fix the `@see`) |
| 5. adopt | the `cp` / `rm` lines | copy each snapshot into `snapshots/` only after reviewing it, file by file |

Pipeline exit codes: 0 clean · 1 drift or changed quotes · 2 a source,
definition or cited page is broken, or the command line is (a `--provider`
list that names no provider or one the catalog doesn't know, or a
`--max-lines` that is not a whole number: a usage message, nothing runs) · 3
the pipeline itself crashed (a bug in these scripts: fix it, run the node
tests, rerun).

### Repairing a broken source is part of the run

A source breaks when the vendor moves or restructures a page. The run does not
stop there: the report ends with a **Repair first** block that gives, for each
FAILED item, the commands to reproduce it (`curl -sIL` on its URL), where to
look for the move (the vendor's `llms.txt`, the adapter's `@see` URLs, the
sitemap or changelog), which definition to edit, and the verify line (two
passes into one fresh folder, then the node tests). Fix the definition, rerun
the pipeline, and only then read the drift for that provider. A cited page
that died in `quotes.txt` is repaired the same way, in the row's `@see`.

### Quotes a script cannot verify

`quotes-known.txt` lists quotes a script cannot check (a JS-rendered page, a
login wall, a quote that is not the vendor's), one per line:
`<model id> | <fragment>  # why`. The quote check skips them, and reports a
line that no longer applies (found again, or no such quote) as stale.

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
- **`gone: (parsed, body) => ids`** (optional): the ids a source asked the
  vendor about directly and heard are unknown. That answer outranks a listing
  in another source, so a carried one reads `missing upstream` and none reads
  `new`. A source whose every id is gone (a fal chunk that left the registry
  whole) still answered: its ids are missing, not a FAILED source. fal's find
  mode uses it: on 2026-11-16 its catalog page still listed
  `fal-ai/stable-diffusion-v35-medium` while find mode, the queue OpenAPI and
  the model page all answered 404.
- **`fetch: (url, get) => response`** (optional): for a vendor API that needs
  more than one request to answer. `get(url)` fetches like the plain source
  (headers, timeout, retries) and returns `{ status, contentType, body }` or
  `{ error }`; the hook returns the same shape. fal's find mode answers 404
  for a whole batch once one id has left its registry, so its carried sources
  ask id by id after such a 404.
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
