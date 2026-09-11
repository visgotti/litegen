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

```bash
node scripts/model-drift/check.mjs --json > "$TMPDIR/drift.json"; echo "exit=$?"
```

Exit codes: 0 means clean. 1 means drift. 2 means a source or definition is
broken. The run rewrites `scripts/model-drift/snapshots/`. Handle every exit-2
item first, then re-run. Drift results from a broken provider mean nothing.

## Who decides what

| Finding | You do it now | Bring to the user |
|---|---|---|
| `FAILED` source (HTTP error, HTML shell, 0 ids, `(canary)`) | Repair the definition (recipe below) | — |
| `FAILED (config)` | Map the model in `providers/<p>.mjs` from the adapter's model-resolution code | — |
| `changed` snapshot, or a schema showing our typings are wrong | Fix it (typings recipe) | — |
| `new` id | Verify it in the vendor docs: endpoint, price, and whether our adapter serves it unchanged | Add or skip |
| `missing` | Confirm it in the vendor changelog or deprecations page | Remove or replace |
| Our `pricing` differs from the vendor's published price | Note both prices, with the pricing URL | Update or keep |

A wrong typing on a model we carry is a bug. Fix it; don't just report it.

Acknowledge an id only in two cases. The first is structural noise, such as a
dated alias or a shutdown the vendor documents. The second is after the user
says to skip it. Either way, record the reason.

## Repairing a failed source

1. Reproduce: `curl -sIL -m 30 <url>`.
2. Find where it moved. Look in the vendor's `llms.txt` first, then the adapter's
   `@see` URLs, then the docs sitemap or changelog, then a web search.
3. Prefer a machine-readable source, such as OpenAPI JSON or a `.md` twin of the
   page. Keep extraction scoped to the endpoints the adapter calls.
4. Verify:
   - Run `check.mjs --provider <p> --snapshots "$(mktemp -d)"` twice. The first
     run must show no `FAILED`, and the second must show no `changed`.
   - Run `node --test 'scripts/model-drift/test/*.test.mjs'`.

If several providers failed, dispatch one subagent per provider, in parallel.

## Typings recipe

1. Run `git status --porcelain scripts/model-drift/snapshots/`. An untracked
   (`??`) snapshot has never been reviewed, so read the whole file. A modified
   one needs only its `git diff`. For each model we carry, compare the vendor
   schema against `models/<p>.yaml`: enums, min and max bounds, capabilities,
   and prompt limits.
2. For each mismatch, first update the matching fact in
   `litegen-core/tests/catalog_conformance.rs` and watch it fail. Then fix the
   yaml.
3. Run both suites:
   - `cargo test --manifest-path litegen-core/Cargo.toml --test catalog_conformance`
   - `cargo test --manifest-path litegen-core/Cargo.toml --lib capabilities::registry`

   If an adapter hard-codes the old value, fix the adapter with a unit test too.

## Finish

1. Commit only your own paths. Other sessions share this working tree:
   `git add <paths> && git commit -m "chore(model-drift): weekly run YYYY-MM-DD" -- <paths>`.
   Then push. The snapshots are next week's baseline, so they must be
   committed.
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
