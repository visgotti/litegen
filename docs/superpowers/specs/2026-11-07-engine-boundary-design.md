# The engine boundary in litegen-core

Status: decided by litegen-d0 on 2026-11-07, at the user's request ("Fix the
seams and add an isolation test", "make sure we do this using best practices
that will guarantee better stability and proof that our code is not
breaking"). Not committed.

## 1. Why

`litegen-core` holds two kinds of code in one crate.

- **The engine** turns a request into a result. It holds the provider
  adapters, the catalog's capabilities and pricing, the shared types, the 3D
  library, input materializing and 3D delivery.
- **The server** wraps the engine for many users. It holds the HTTP API,
  auth, access, the database, management (keys, budgets, orgs), hooks, MCP,
  config, and the background job loop.

Today only habit keeps the engine off the server. A measurement on
2026-11-07 found 45 references from engine code into server code, across
about 20 files. Most are small helpers living in the wrong module, and each
new provider adds more. This change makes the line explicit and has a test
enforce it. A later extraction of an engine crate or library then becomes a
mechanical move, but that is not part of this change.

## 2. Goals

- G1. A test, run by `cargo test`, that fails when an engine file references
  a server module, naming the file, line and path.
- G2. Every one of today's 45 references is resolved, so the test passes.
- G3. No behaviour change:
  - moved items keep their bodies byte for byte;
  - the API contract (`/openapi.json`) is byte-identical before and after;
  - the full gate's counts match the baseline, plus the new tests.
- G4. Future sessions are told the rule: `.claude/rules/engine-boundary.md`,
  beside `mesh-standalone.md`.

## 3. Non-goals

- No new crate, no file reorganisation beyond what a seam needs, and no
  renames of public paths. Server call sites keep compiling through
  re-exports.
- Splitting `proxy/router.rs`'s routing and fallback into the engine. The
  engine borrows only two pure helpers from it; the split waits until a
  library needs routing.
- CI. The test runs in every `cargo test`; wiring CI is its own project.

## 4. The boundary

- **Engine, whole folders:** `providers/`, `capabilities/`, `types/`,
  `util/`, `mesh/` (which keeps its own stricter rules), and the new
  `store/`.
- **Engine, files in `proxy/`:**
  - `materializer.rs` (+ tests), `preserve.rs`, `image_edit.rs`;
  - `model3d_convert.rs`, `model3d_deliver.rs` (minus the lease, see D3),
    `model3d_source.rs`;
  - `circuit_breaker.rs` (+ tests), `ref_publish.rs`, `segmentation.rs`,
    `video_probe.rs`.
- **Server:** everything else. That covers:
  - the folders `api/`, `auth/`, `access/`, `config/`, `db/`, `hooks/`,
    `management/`, `mcp/`;
  - `observability.rs`, `main.rs`, and `lib.rs`'s server modules;
  - in `proxy/`: `router.rs` (+ tests), `poller.rs`, `storage.rs`,
    `webhook.rs`, `retention.rs`, `cache.rs`, `aws_sts.rs`, `registry.rs`,
    `model3d_stages.rs` and the new `delivery_lease.rs`.
- **The rule:** an engine file names only the engine, through `crate::…`
  and through `super::…`, wherever the `super::` climbs from. The server may
  use the engine freely.
- **Every file is classified:** every `.rs` file and folder in `proxy/` must
  be on one of the two lists, and every file in an engine folder is scanned,
  so a new file can't slip past.

## 5. Decisions

- D1. **Move, re-export, don't rewrite.** A seam moves the item with its body
  unchanged, and the old module re-exports it (`pub use`). Engine code
  imports the new path; server code keeps working through the old one.
- D2. **Storage.** The engine uses storage through `ImageStorage` and
  `ImageStore`.
  - These move to a new engine module, `store/`, with their error types, the
    key layout (prefixes, `model3d_asset_key`, `sanitize_model3d_format`),
    the content-type helpers (`image_format`, `model3d_content_type`) and
    the local, config-free implementations.
  - `proxy/storage.rs` keeps S3, bucket checks and building a store from
    config, and re-exports the rest.
  - One signature change follows. `ImageStorage::signed_url` takes the
    server's `access::signing::LinkSigner`, so its parameter becomes
    `&dyn store::LinkSigning`. That is a one-method engine trait
    (`local_query`, the only method the stores call), and `LinkSigner`
    implements it by calling its own method. Callers still pass a
    `LinkSigner`, which Rust coerces.
- D3. **3D delivery's lease.** `DeliveryClaim`, `claim_delivery` and their
  helpers (a database lease, so two observers don't deliver the same job)
  move to `proxy/delivery_lease.rs`, a server file, with their tests.
  `model3d_deliver.rs` keeps the delivery itself.
- D4. **Row readers.** `row_requested_formats` and `generation_assets` read a
  generation row and touch nothing else. They move from `api/handlers` to
  `types/generation.rs`, re-exported from the handlers. `generation_assets`
  brings the cluster it calls, which are pure "generation row → 3D response"
  rules:
  - `model3d_response_from_row`, and `usage_from_row_cost` with its
    `USD_PER_TOKEN`, from the handlers;
  - `enforce_requested_formats` from `router.rs`, re-exported there.
  `router.rs` keeps its own private `USD_PER_TOKEN`, a duplicate left for a
  later cleanup.
- D5. **The duration rule.** `resolve_duration_seconds` is catalog logic that
  both pricing and the validator use. It moves from
  `api/middleware/validator.rs` to `capabilities/`, re-exported from the
  validator.
- D6. **Two pure router helpers.** `image_content_type` and `sha256_hex`
  move to `util/`, re-exported from `router.rs`.
- D7. **The webhook client.** `util/ssrf.rs`'s only use of the server's
  `config::Mode` is `webhook_client`, and every caller of it is server code:
  the poller and four handler lines. It moves to `proxy/webhook.rs` with its
  test, unchanged, and those five lines change only their path. The engine
  keeps the clients it builds on.
- D8. **Server code living in engine folders moves to the server.**
  - `util/slug.rs` uses the database, so it moves to `management/slug.rs`,
    and its two callers in `api/handlers` change their path.
  - `providers/config_conformance.rs` checks the catalog against the
    registry and the validator, so it moves to `proxy/`.
  - `proxy/registry.rs` and `proxy/model3d_stages.rs` stay where they are,
    classified as server.
- D9. **The test.** `src/engine_boundary_tests.rs` is a `cfg(test)` module
  in `lib.rs`. Like `mesh/isolation_tests.rs`, it reads files at test time
  and reports every violation at once. It reads code the way the compiler
  does:
  - comments and the contents of string and char literals aren't code;
  - a `use` group split over several lines is one group;
  - a `super::` path is resolved from the module it's written in, including
    inline modules and files included with `#[path]`.

  Engine code may name only `ENGINE_DIRS` and the engine entries in
  `proxy/`. That is an allow-list, so a new server module needs no entry.
  The first version stripped `//` comments line by line and checked
  `super::` paths against a list of server modules. The final review on
  2026-11-07 showed it missed four kinds of path:
  - groups split over several lines;
  - paths after a URL on the same line;
  - `super::` climbs out of inline modules;
  - globs.

  Each of the four now has a test.

## 6. Proof that nothing breaks

1. **Baseline**, before any edit, with the cargo protocol and the peers'
   "green":
   - the full gate (below), with every test binary's counts recorded;
   - clippy;
   - `/openapi.json` from the current binary, hashed.
2. **RED first.** The new test fails on the untouched tree, and its list
   must equal the 45 references measured on 2026-11-07.
3. **One seam at a time.** After each: `cargo check --all-targets`, that
   module's tests, and the boundary test's list shrinking by exactly that
   seam.
4. **GREEN.** The boundary test passes. The full gate's counts equal the
   baseline plus the new tests, and clippy is clean.
5. **The contract.** `/openapi.json` from the rebuilt binary is
   byte-identical to the baseline.
6. **Moves are moves.** A script compares every moved item's text, before
   and after, whitespace-normalised. Any difference beyond paths and
   visibility fails it.
7. **Downstream.**
   - `packages/3d-converter`'s tests: its handler tripwire watches
     `model3d_content_type`, so its source location changes and it is
     re-pinned only after step 6 shows the body is the same.
   - The dashboard's 3D e2e against the rebuilt binary.
8. **A fresh review.** A reviewer reads the diff for any behaviour change.

The full gate, from `litegen-core`, as the 2026-11-04 overview defines it:

- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --lib -- --skip <the three load-sensitive mesh tests>`
- the three alone
- `cargo test --tests --no-fail-fast -- --skip <the three>`, with
  `LITEGEN_PG_TEST_URL` set to a throwaway `postgres:16-alpine`
