# Engine boundary: implementation plan

Spec: `docs/superpowers/specs/2026-11-07-engine-boundary-design.md`.
Executed inline by litegen-d0 (superpowers:executing-plans), with test-driven
development. Ledger: `.superpowers/sdd/2026-11-07-engine-boundary/progress.md`.

## Global constraints

- **No behaviour change.** Bodies move verbatim (spec D1). The one signature
  change is `ImageStorage::signed_url`'s signer becoming `&dyn
  store::LinkSigning` (spec D2).
- **Git:** no commits, no staging, no stash, no worktrees. Restore a file by
  rewriting it, never with `mv`.
- **The cargo protocol, every run:**
  - wait until `pgrep -x cargo`, `rustc` and `clippy-driver` have been idle
    for 10 s;
  - `grep -rn NEGCTL litegen-core/src scripts/mesh-verify` comes back empty;
  - at least 8 GiB of disk is free;
  - `CARGO_INCREMENTAL=0` and `-j 4`.
- **Claimed files:** told to litegen-b2, litegen-fc and litegen-bd before the
  first edit. A full gate waits for each to answer "green".
- **Out of scope:** `mesh/`, which litegen-b2's item 2 is editing.

## Task 0: baseline (no edits)

- Snapshot every file this plan touches into the ledger's `before/`, the
  input to the moves-are-moves check.
- Run the full gate on the untouched tree, with the commands from spec §6,
  and record each test binary's passed, failed and ignored counts in
  `baseline.txt`.
- Hash `/openapi.json` from the current debug binary, run with a scratch
  config on a free port, into `openapi.before.sha256`. Never write
  `sdks/openapi.json`.
- Expected: every count recorded. Any failure that already exists is named
  in the ledger, so it isn't mistaken for this work's.

## Task 1: the boundary test, RED

- Create `litegen-core/src/engine_boundary_tests.rs` and add
  `#[cfg(test)] mod engine_boundary_tests;` to `lib.rs`. It holds three
  tests:
  - `engine_code_names_no_server_module`: scans the engine folders
    recursively, plus the engine list in `proxy/`, skipping itself. A
    `crate::` path into a server module is a violation. Inside `proxy/`,
    `super::` into a server file is one too. It fails listing
    `file:line: path`.
  - `every_proxy_file_is_classified`: the `.rs` files in `proxy/` equal the
    union of the engine and server lists.
  - `the_lists_name_real_files`: every listed file exists.
- Run `cargo test --lib engine_boundary`.
- Expected: the first test FAILS, and its list equals the 45 references in
  the spec. The other two pass, apart from files that don't exist yet
  (`delivery_lease.rs`, `store/`), which are added to the lists only when
  their task creates them.

## Task 2: pure helpers (D4, D5, D6)

- Move each item verbatim and re-export it from the old path:
  - `resolve_duration_seconds` → `capabilities/duration.rs`;
  - `row_requested_formats` and `generation_assets` → `types/generation.rs`;
  - `image_content_type` and `sha256_hex` → `util/content.rs`.
- Engine call sites import the new paths: `capabilities/pricing.rs`,
  `providers/image/{bedrock,bytedance,tokenhub}.rs`, `proxy/model3d_source.rs`
  and `proxy/model3d_deliver.rs`.
- Expected: `cargo check --all-targets` is clean, and the RED list loses
  exactly these references.

## Task 3: the storage interface (D2)

- Create `store/mod.rs`. The traits, their errors, the key layout, the
  content-type helpers and the local implementations move there verbatim.
  `proxy/storage.rs` keeps S3, bucket checks and building from config, and
  says `pub use crate::store::*;`.
- Engine call sites import from `crate::store`: `materializer.rs` (+ tests),
  `ref_publish.rs`, `model3d_source.rs`, `model3d_deliver.rs` and the four 3D
  providers.
- Expected: `cargo check --all-targets` is clean, and the storage references
  leave the RED list.

## Task 4: the delivery lease (D3)

- Move `DeliveryClaim`, `claim_delivery`, `claim_from`, their slot and lease
  helpers, and their tests into `proxy/delivery_lease.rs`, verbatim. Server
  call sites import the new path.
- Run `cargo test --lib -- proxy::delivery_lease proxy::model3d_deliver`,
  plus the Postgres lease tests against the throwaway database.
- Expected: the same counts as the baseline for these tests; the database
  references leave the list.

## Task 5: config and server code in engine folders (D7, D8)

- Move `webhook_client` and its test from `util/ssrf.rs` to
  `proxy/webhook.rs`, and update the path in its five server callers:
  `poller.rs:141` and `api/handlers/mod.rs` lines 1010, 2474, 3805, 4091.
- Move `util/slug.rs` to `management/slug.rs`, and update its two callers in
  `api/handlers`.
- Move `providers/config_conformance.rs` to `proxy/config_conformance.rs`,
  first confirming with whoever edited it at 00:26 on 2026-11-07.
- Expected: the RED list is empty and the boundary test passes (GREEN).

## Task 6: proof (spec §6, steps 4 to 8)

- **Full gate:** the counts equal `baseline.txt` plus the new tests, and
  clippy is clean.
- **The contract:** rebuild the debug binary; its `/openapi.json` hash equals
  `openapi.before.sha256`.
- **Moves are moves:** run the moved-items check against `before/`.
- **Downstream:** the converter's `cargo test -p litegen-3d-converter`, with
  the tripwire re-pinned after the moves check, and the dashboard's 3D e2e
  against the rebuilt binary.
- **Rules:** write `.claude/rules/engine-boundary.md`.
- **Review:** a fresh reviewer reads the diff for behaviour changes.
- Expected: every check passes. Anything that differs is either fixed or
  ruled in the ledger.

## Review focus

1. A `super::` path inside `proxy/` that reaches a server file: the scanner
   must catch it.
2. A re-export cycle (`store` ↔ `proxy::storage`) or a duplicate-definition
   error under `--all-features`.
3. A test module moved without its `#[cfg(test)]`, or a `pub(crate)` item
   that becomes unreachable from its new path.
4. The tripwire re-pin: it must follow a verified identical body, never a
   changed one.
5. A peer editing a claimed file mid-plan: re-snapshot `before/` for that
   file, and re-run its seam.
