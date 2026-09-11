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
| 1 | drift: new, missing or changed items | review each item (below) |
| 2 | needs investigation: a source failed, or a definition is out of sync with `models/*.yaml` | fix the definition first (the skill does this) |

## What the report means

- **`new upstream`**: the vendor lists a model id we don't carry and haven't
  acknowledged. Either add it to `models/<provider>.yaml`, with typings and
  pricing verified against the vendor docs, or add it to the definition's
  `acknowledged` list with the reason we skip it. Which one is a product
  decision.
- **`missing upstream`**: a model we carry no longer appears in any source.
  Check the vendor's changelog or deprecations page. Retired means remove or
  replace the model. Renamed means fix the mapping, and the adapter too if it
  sends the old id.
- **`changed`**: a source's snapshot differs from the committed one. Run
  `git diff scripts/model-drift/snapshots/` to see what changed upstream, such
  as new enum values, new bounds or new endpoints. Compare that against the
  typings in `models/<provider>.yaml`, then commit the snapshot to accept it.
  An **untracked** snapshot has never been reviewed. Compare the whole file,
  not a diff, before committing it. Committed snapshots are a reviewed
  baseline, so committing one unreviewed would bury any mismatch it shows.
- **`FAILED <source>`**: the source didn't give a usable list. Possible causes:
  - an HTTP error
  - an HTML page where a spec or markdown was expected (the page moved, or now
    serves a JS app shell)
  - a parse or extractor error
  - no ids found
  - **`(canary)`**: sources returned ids, but none of ours. The page was
    restructured.
  - **`(config)`**: the definition and `models/*.yaml` disagree.

  The vendor moved something, so the definition needs updating. Never "fix" a
  failure by deleting the source or by loosening the pattern until it matches
  anything.

## How it works

`providers/<provider>.mjs` holds one definition per vendor:

```js
export default {
  provider: 'runway',
  // litegen model id → the id the vendor documents (what the adapter sends)
  models: { 'runway/gen4-turbo': 'gen4_turbo' /* … every model in models/runway.yaml */ },
  sources: [{
    key: 'openapi',                                    // names snapshots/runway/openapi.txt
    url: 'https://docs.dev.runwayml.com/openapi.json',
    expect: 'json',                                    // json | text | html (HTML where text was expected = soft 404)
    extract: (spec) => openapiModelIds(spec, { paths: ENDPOINTS }),   // or a RegExp over the body
    snapshot: (spec, _body, { ours }) => openapiExcerpt(spec, { paths: ENDPOINTS, ours }),
  }],
  acknowledged: [{ pattern: /-\d{4}-\d{2}-\d{2}$/, reason: 'dated snapshot alias' }],
};
```

- **Scope ids to what our adapter can call.** Extract model ids from the
  request schemas of the endpoints the adapter uses, or from the model
  families we carry for aggregators like fal and Replicate. Don't pull every
  string on the page.
- **Snapshots are typings evidence.** `openapiExcerpt` keeps structural keys
  only, since prose churns every week. For unions discriminated by `model`, it
  keeps only the branches for models we carry. `yamlPrune(markdownExcerpt(…))`
  does the same for Mintlify `.md` pages that embed OpenAPI. Sources without a
  `snapshot` fall back to their sorted id list.
- **`index: true` sources** list docs pages or endpoints (for example
  `llms.txt`). A new page shows up as a changed snapshot, never as a model id.
- **Source preference:** an official machine-readable spec or list first. Next
  is a docs site's markdown twin (`llms.txt` → `<page>.md`). Last is
  server-rendered HTML.

The test `every model in models/*.yaml is mapped…` fails when a model is added
without a mapping, so the check can't silently skip it.

## Adding a provider

1. Add `providers/<name>.mjs`. Map every model in `models/<name>.yaml`,
   mirroring the adapter's model-resolution code in
   `litegen-core/src/providers/{image,video}/<name>.rs`.
2. Run `node scripts/model-drift/check.mjs --provider <name> --snapshots "$(mktemp -d)"`
   twice. The first run must not exit 2, and the second must show no `changed`.
3. Run the tests and commit the definition. Its snapshots get committed by
   the first `/model-drift` run, after that run reviews them against
   `models/<name>.yaml`.
