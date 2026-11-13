# Continuous integration

`.github/workflows/ci.yml` runs on every push, to any branch, and on every
pull request. The repository is public, so GitHub's standard runners cost
nothing, and every job runs every time. Require the **`ci-ok`** check in branch
protection: it fails unless every other job passed.

## The jobs

| Job | What it proves |
|---|---|
| `actionlint` | The workflow itself is valid, and its shell steps pass shellcheck. |
| `core` | litegen-core: clippy with `-D warnings`; the lib, all 55 integration suites and the examples, against real Postgres, a real Redis and a real S3 (Zenko CloudServer); the timing tests, run alone; nothing skipped; the OpenAPI document and both SDKs regenerate unchanged. |
| `release-binary` | `cargo build --release`. The binary goes to the jobs below. |
| `docker` | The image builds, and `.github/scripts/docker-smoke.sh` runs it: with environment variables only and no database setting (the database and kept files in the `/app/data` volume, through a restart and a new container; `docker stop` ends it cleanly), with a mounted config file (as `/app/litegen.yaml`, by `--config`, `LITEGEN_CONFIG_FILE_PATH` or `LITEGEN_CONFIG_FILE`; variables beat it, and nothing the image sets does), as uid 1000, refusing to start with no master key, and on Postgres from the `LITEGEN_DATABASE_*` parts. A failed check prints its container's log. |
| `dashboard` | Lint, unit tests, build, Storybook build and the stories suite. |
| `dashboard-e2e` | The four Playwright suites against the release binary: `main`, `access`, `hooks`, `multitenant`. |
| `landing` | The scripts' tests; the catalog snapshots and the OpenAPI copy match `models/` and `sdks/openapi.json`; `next build`; the built reference page shows every model's exact catalog price, its unit and what it does (`npm run test:built`). |
| `price-api` | Lint, unit tests, the e2e suite on Postgres, build, and every litegen model priced. |
| `sdk-typescript` | Typecheck, tests, build. |
| `sdk-python` | The tests, integration suite included, on Python 3.10 and 3.14 against the release binary. |
| `viewer` | packages/3d-viewer: typecheck and tests. |
| `converter` | packages/3d-converter: clippy (native and wasm32), the core tests, the npm package built and tested, the PyPI wheel built, its `.pyi` unchanged, the Python tests on 3.10 and 3.14. |
| `converter-app` | apps/3d-converter against the converter just built: tests, build, e2e and stories. |
| `mcp` | The MCP SDK against the release binary: `/mcp` and `/admin/mcp` smoke tests, a generation, and the OAuth sign-in end to end. |
| `scripts` | model-drift's and mesh-verify's unit tests, with Khronos's schema files fetched and checked, and xmllint installed (`libxml2-utils`) for mesh-verify's Collada schema test; `sdks/scripts/fetch-openapi.sh`'s tests (copies of the script against stub gateways and a stub server binary: a failed or bad fetch never replaces `sdks/openapi.json`); `deploy.js`'s landing-export tests (`scripts/deploy/`, with npm, wrangler and SSH stubbed out). |

## What a red run usually means

- **"generated files are out of date"**: a source moved ahead of its
  generated file. Regenerate and commit what changed:
  - an OpenAPI-visible change (a route, a field, a header, a doc comment the
    OpenAPI shows): `sdks/scripts/regen-all.sh`, then
    `node apps/landing/scripts/sync-openapi.mjs`;
  - a `models/*.yaml` change: `node apps/landing/scripts/sync-capabilities.mjs`
    and `node apps/landing/scripts/sync-models.mjs`;
  - the converter's stub: `packages/3d-converter/python/scripts/build.sh`;
  - `docs/mcp/tools.md` (a Rust test checks it): from `litegen-core/`,
    `cargo run --example mcp_tools_doc > ../docs/mcp/tools.md`.
- **"a test skipped itself"**: a suite that needs a service or a runtime
  (Postgres, Redis, S3, python3, node or bun for the hooks, scripts/mesh-verify's
  packages and xmllint, the litegen binary) passed without running. CI provides all of
  them, so a skip means a job lost one. `.github/scripts/no-skips.sh` lists
  the lines.
- **Clippy**: CI uses the toolchain `rust-toolchain.toml` pins (1.99.1).
  Locally, put rustup's proxies first on `PATH` (see
  `.claude/rules/working-in-this-repo.md`).

## Running a job's steps yourself

Each step is plain shell, run from the job's `working-directory`. The
services the `core` job uses can be started with Docker:

```sh
docker run -d --rm --name pg -p 127.0.0.1:5432:5432 -e POSTGRES_PASSWORD=pw -e POSTGRES_DB=litegen_test postgres:16-alpine
docker run -d --rm --name litegen-redis-test -p 127.0.0.1:56379:6379 redis:7-alpine
docker run -d --rm --name s3 -p 127.0.0.1:9000:8000 -e REMOTE_MANAGEMENT_DISABLE=1 zenko/cloudserver
export LITEGEN_PG_TEST_URL=postgres://postgres:pw@127.0.0.1:5432/litegen_test
export LITEGEN_REDIS_TEST_URL=redis://127.0.0.1:56379
export LITEGEN_S3_TEST_ENDPOINT=http://127.0.0.1:9000 LITEGEN_S3_TEST_KEY=accessKey1 LITEGEN_S3_TEST_SECRET=verySecretKey1
```

The Redis tests (shared limits, slots, cache and events, and the two-replica
suite `tests/scaling_replicas.rs`, which needs Postgres as well) skip
themselves without `LITEGEN_REDIS_TEST_URL`. Use a throwaway Redis, not one
that serves anything: the tests write keys under a `litegen-test:<id>:` prefix
of their own, which they don't all remove, and some run `SCRIPT FLUSH`. CI runs
Redis 7 as a service, on port 6379.

The Playwright suites take `LITEGEN_E2E_BINARY` (any litegen build), and the
Python SDK's integration tests boot `litegen-core/target/debug/litegen`.

## What CI leaves out, and why

- **Live vendor tests** (the `#[ignore]`d `live_*` suites, `dashboard/e2e-live`):
  they need paid keys, and a pull request from a fork gets no secrets.
- **The weekly model-drift run** (`scripts/model-drift/check.mjs`): it fetches
  vendor pages, so it checks the vendors, not this code. Its unit tests run.
- **mesh-verify's render matrix**: it needs Blender and assimp. Its unit tests
  run.
- **rustfmt**: the code doesn't follow rustfmt's layout.
- **Lint for the landing page**: it has no ESLint config; `next build`
  type-checks it.

Actions are pinned to commit SHAs. `.github/dependabot.yml` proposes updates
each week, and that pull request's CI run checks them.
