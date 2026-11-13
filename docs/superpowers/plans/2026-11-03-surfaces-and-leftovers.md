# Surfaces and leftovers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring every surface (price API, providers list, landing site,
README, clippy) to what ships, and close the deferred minors.

**Architecture:** Each surface reads the catalog (`models/*.yaml`) or the
gateway instead of a hand-kept list, so it can't drift again; the minors are
fixed in place or closed with a ruling.

**Tech Stack:** Rust (litegen-core), TypeScript (price API on NestJS,
dashboard on React/vitest, landing on Next.js with node:test scripts).

**Spec:** docs/superpowers/specs/2026-11-03-surfaces-and-leftovers-design.md

## Global Constraints

- No commits without the user's go; nothing staged.
- One cargo at a time, `CARGO_INCREMENTAL=0`; no cargo and no SDK regen until
  litegen-30 pings that the merged tree builds.
- An OpenAPI change regenerates `docs/mcp/tools.md`
  (`cargo run --example mcp_tools_doc`) and the SDKs once, after the ping.
- `litegen-core/src/mesh/` is not touched; `scripts/model-drift/snapshots/`
  is never committed.

## Review Focus

- A catalog row added or dropped later: the price API's coverage check and
  the landing's snapshot test must fail until regenerated, not pass silently.
- A provider with runtime models but no catalog rows: the Providers page note
  must not claim it serves nothing when an operator added a model.
- The landing build with no gateway reachable: it must still produce the
  provider list from the shipped catalog.
- Clippy `-D warnings` on all targets with `--all-features`, not only the
  default build.
- A deferred minor closed by ruling must name why it stands.

---

### Task 1: The price API mirrors the catalog

**Files:** `apps/price-api/` (enums, scripts/check-coverage.ts,
scripts/lib/{coverage,seed-render,litegen-models}.ts, src/providers/**,
seed, models, pricing, providers and scrape services, model DTO, e2e spec,
eslint.config.mjs, README).

- [ ] `coverage:fix` regenerates every seed from the catalog (3D as
  `model3d`, priced per request) and scaffolds missing providers; the check
  fails on a seeded model the catalog dropped.
- [ ] A real scraper lists the ids it reports; the registry pins the rest
  manual. OpenAI becomes manual (GPT Image is billed per token).
- [ ] Seeding retires models the registry no longer declares; lists and the
  pricing table leave them out.

Run: `cd apps/price-api && npm run coverage:check && npx jest && npm run lint`
plus the e2e suite against a throwaway Postgres.

### Task 2: Providers that serve nothing say so (after the ping)

- [ ] Test (`tests/models_list_api.rs`): with an org pool of one model,
  `GET /v1/models` lists it alone, `?all=true` lists every served model, and
  a generation with a model outside the pool is still 403
  `model_not_allowed`.
- [ ] SDKs: `models.list({ all: true })` / `models.list(all=True)`.
- [ ] Dashboard Providers page lists `all` models and shows "No models run on
  <provider> yet" for a provider with none; god-test covers both (a reload
  keeps a model outside the pool listed).
- [ ] Regenerate `docs/mcp/tools.md`, the SDKs and the landing OpenAPI copy.

### Task 3: The landing site reads its providers from the catalog

- [ ] `sync-capabilities` without a gateway derives the packets from
  `models/*.yaml` (mock skipped); the snapshot is regenerated.
- [ ] `site.ts` `PROVIDERS`, the InfraFlow nodes and
  `derive-capabilities.test.mjs` read the snapshot; node:test and the
  landing build pass.

### Task 4: The README says what ships

- [ ] Intro, features, provider table (every catalogued provider and its
  families), roadmap and project structure checked against the tree.

### Task 5: Clippy passes (after the ping)

- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean;
  mesh findings allowed at `pub mod mesh;` with a pointer to the mesh rule.

### Task 6: Deferred minors

- [ ] Every `minor (deferred)` line in `.superpowers/sdd/*/progress.md` is
  fixed (with its test) or closed by a ruling in this plan's ledger.

### Task 7: Final gate

- [ ] cargo (with Postgres), clippy, dashboard vitest/eslint/tsc, landing
  tests and build, both SDK suites, price API (unit, e2e, lint, coverage),
  the browser suites.
