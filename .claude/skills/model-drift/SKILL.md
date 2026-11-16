---
name: model-drift
description: Use when running the weekly provider model-drift check, when scripts/model-drift/check.mjs exits non-zero, when a vendor docs page or spec the check reads 404s or moved, or when asked whether AI vendors shipped models or request parameters that models/*.yaml does not reflect.
---

# Weekly provider model drift

## Overview

`scripts/model-drift/check.mjs` diffs what every vendor publishes (model ids,
request schemas) against `models/*.yaml`. The script is deterministic. You are
the judgment layer: repair what broke, correct typings the vendor's schema
proves wrong, and bring the user only the decisions that are theirs.
Details: `scripts/model-drift/README.md`.

## Run

One command runs the whole pipeline, on a copy of the snapshots:

```bash
node scripts/model-drift/pipeline.mjs; echo "exit=$?"
```

It never writes `scripts/model-drift/snapshots/` (other sessions may have
edits there): it copies them into `<out>/snapshots`, checks there, and writes
into `<out>` (default `.superpowers/sdd/<date>-drift/run-<time>/`):

- `report.txt` / `drift.json`: the check (what `check.mjs --json` gives; each
  provider lists `noBaseline` and `deprecated` beside `new`, `missing` and
  `changed`);
- `review.md`: only the snapshot sections that changed, as JSON paths, plus
  the never-reviewed sections to read whole (`review.mjs`). Read this, not a
  raw diff: it leaves out sections that only moved between files;
- `quotes.txt` / `quotes.json`: every quoted vendor string in models/*.yaml
  checked against the pages its comment block cites (`quotes.mjs`): a quote no
  longer on its page means the vendor changed a price or a rule; a page that
  failed means the citation died;
- `summary.txt` (also printed): what to do next, in order, ending with the
  `cp`/`rm` lines that adopt each reviewed snapshot.

Exit codes: 0 clean. 1 drift or a changed quote. 2 a source, definition or
cited page is broken, or the command line is. 3 the pipeline itself crashed:
a bug in scripts/model-drift (fix it, run the node tests, rerun). Repair every
exit-2 item first, then rerun: drift from a broken provider means nothing.
`--provider a,b` narrows a rerun; `--no-quotes` skips the page fetches.

## Who decides what

| Finding | You do it now | Bring to the user |
|---|---|---|
| `FAILED` source (HTTP error, HTML shell, 0 ids, `(canary)`) | Repair the definition (recipe below) | — |
| `FAILED (config)` | Map the model in `providers/<p>.mjs` from the adapter's model-resolution code | — |
| `FAILED (typings)` | Add or re-scope a source whose snapshot holds that model's request schema (`untyped` only when the vendor publishes none) | — |
| `changed` snapshot, or a schema showing our typings are wrong | Fix it (typings recipe) | — |
| `no baseline: <key>` | The snapshot was never reviewed: read all of `<p>/<key>.txt` in the folder the run used, and run the typings recipe on it | — |
| `new` id | Verify it in the vendor docs: endpoint, price, and whether our adapter serves it unchanged | Add or skip |
| `missing` | Confirm it in the vendor changelog or deprecations page. A model a source lists as deprecated is reported as `deprecated upstream`, not `missing` | Remove or replace |
| `deprecated upstream: <model> (<ids>)` | The vendor still serves it but marks it deprecated. Confirm it, then set the row's `deprecated_at` (and `replaced_by` when the vendor's successor is a catalog model), or its `retires_at` plus the `SCHEDULED_SHUTDOWNS` entry once there is a date | Whether to move callers to the successor |
| Our `pricing` differs from the vendor's published price | Note both prices, with the pricing URL | Update or keep |
| A quote in `quotes.txt` is no longer on its page | Re-read the page; fix the row (price, bound, enum) and the quote, with today's date | A price change on a carried model |
| A cited page failed in `quotes.txt` | Find where it moved and fix the `@see` (and the quote) | — |
| A quote a script can't verify (script-rendered page, login wall, not the vendor's words) | Add it to `scripts/model-drift/quotes-known.txt` with the reason | — |

A wrong typing on a model we carry is a bug. Fix it; don't just report it.

Acknowledge an id only in two cases. The first is structural noise, such as a
dated alias or a shutdown the vendor documents. The second is after the user
says to skip it. Either way, record the reason.

"Litegen carries this vendor or model direct" is never a reason. The user
wants every hub's route carried as its own row (2026-11-04: "like how litellm
can still use openrouter"), so a model TokenHub, fal, Replicate or Bedrock
resells is a `new` id to carry. `test/aggregator-routes.test.mjs` fails on a
new direct-route skip; its `PENDING` list names the definitions still being
reversed.

## Repairing a failed source

Repair is a step of every run, not a separate job: the report ends with a
"Repair first" block that prints, for each FAILED item, its URL to reproduce,
where to look, the definition to edit and the verify line. Follow it, then
rerun the pipeline. A cited page that died in `quotes.txt` gets the same
treatment in the row's `@see`.

1. Reproduce: `curl -sIL -m 30 <url>`.
2. Find where it moved. Look in the vendor's `llms.txt` first, then the adapter's
   `@see` URLs, then the docs sitemap or changelog, then a web search.
3. Prefer a machine-readable source, such as OpenAPI JSON or a `.md` twin of the
   page. Keep extraction scoped to the endpoints the adapter calls.
4. Verify:
   - Run the check twice into one scratch folder: `D=$(mktemp -d)`, then
     `check.mjs --provider <p> --snapshots "$D"` twice. The first run must
     show no `FAILED` (it lists every source under `no baseline`, since the
     folder starts empty). The second must show neither `changed` nor
     `no baseline`.
   - Run `node --test 'scripts/model-drift/test/*.test.mjs'`.

If several providers failed, dispatch one subagent per provider, in parallel.

## Typings recipe

1. Read `<out>/review.md`. Its "new, read whole" sections are every section
   with no reviewed baseline (a `no baseline` source, or an endpoint that
   moved into a chunk file with no earlier version anywhere); its "changed"
   sections give only the paths that changed. Then run
   `git status --porcelain scripts/model-drift/snapshots/`: an untracked
   (`??`) snapshot, which an earlier run wrote and nobody committed, has never
   been reviewed either, so read it whole too. For each model we carry,
   compare the vendor schema against `models/<p>.yaml`: enums, min and max
   bounds, capabilities, and prompt limits. For a big run, split the packet by
   provider (`review.mjs --fresh <out>/snapshots --provider a,b`) across
   parallel reviewers.
2. For each mismatch, first update the matching fact in
   `litegen-core/tests/catalog_conformance.rs` and watch it fail. Then fix the
   yaml.
3. Run both suites:
   - `cargo test --manifest-path litegen-core/Cargo.toml --test catalog_conformance`
   - `cargo test --manifest-path litegen-core/Cargo.toml --lib capabilities::registry`

   If an adapter hard-codes the old value, fix the adapter with a unit test too.

## Finish

1. Adopt the reviewed snapshots with the `cp`/`rm` lines the summary prints,
   one file at a time, only for files you reviewed. Then, when the user asks
   for a commit, commit only your own paths (other sessions share this tree):
   `git add <paths> && git commit -m "chore(model-drift): weekly run YYYY-MM-DD" -- <paths>`.
   The snapshots are next week's baseline, so they must be committed.
2. Report in this shape:
   1. **Fixed:** each repaired source and each corrected typing, one line each
      with the vendor evidence (URL).
   2. **Decide:** a table with columns provider · id · what it is ·
      adapter-ready (y/n) · vendor price · your recommendation.
   3. **Heads-up:** announced shutdowns or renames that haven't taken effect
      yet, with dates.
   4. **Can't express:** vendor rules the catalog schema can't represent
      (for example "6s or 10s only"), with what we accept instead.
   5. **Unverified:** anything you changed but couldn't test, such as when
      cargo can't run. Say this plainly.
