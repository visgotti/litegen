# LiteGen SDKs

First-party SDKs for the LiteGen HTTP API — image, video and 3D generation.
Types and low-level clients are generated from the OpenAPI spec served by
`litegen-core` at `/openapi.json`; the ergonomic `LiteGenClient` facade is
hand-written on top of that.

## Packages

- [`typescript/`](./typescript) — `@litegen/sdk` (npm). Universal (Node 18+,
  browsers, Deno, Bun). Native `fetch`. ESM + CJS build via `tsup`. Request and
  response types come straight from the generated `schema.d.ts`.
- [`python/`](./python) — `litegen` (PyPI). Python 3.10+. Sync and async
  clients on top of `httpx`, returning plain dicts typed by the TypedDicts in
  `litegen.types` (checked against the spec by `tests/test_types_contract.py`).
  `litegen._generated` holds the attrs models `openapi-python-client` emits.

Both cover the async families the same way: `videos.generate` and
`models3d.generate` return a job that can be awaited for the final state or
iterated for progress, and report params lax mode (`strict: false`) dropped.

## Regenerating from `litegen-core`

```bash
cargo build --manifest-path litegen-core/Cargo.toml   # the script boots this debug binary
./sdks/scripts/regen-all.sh
```

The script:
1. Boots `litegen-core` (or uses `LITEGEN_BASE_URL` if already running).
2. Curls `/openapi.json` into `sdks/openapi.json`.
3. Runs `openapi-typescript` for the TS SDK.
4. Runs `openapi-python-client` for the Python SDK — pinned to the release that
   produced the committed output (see the comment in `regen-all.sh`), so a
   regeneration diff is a schema change and nothing else.

CI fails if `git diff sdks/` is non-empty after running this script — the
canonical source of truth for types is `litegen-core`, and the SDKs must
keep up. The landing site's copy (`apps/landing/public/openapi.json`) is
refreshed from `sdks/openapi.json` by its `sync-openapi` build step.

## Contract tests

`typescript/test/contract.test.ts` and `python/tests/test_contract.py` fail
when a spec operation has no SDK method, or the SDK calls a path the spec does
not declare. Known gaps are ratcheted in [`contract-allowlist.json`](./contract-allowlist.json):
shrink it as methods land, never grow it to silence new drift.

## Development

Each package has its own README and test/build scripts. Both ship example
scripts under `examples/` (`generate-3d.ts` / `generate_3d.py` for 3D).
