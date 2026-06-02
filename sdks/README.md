# LiteGen SDKs

First-party SDKs for the LiteGen HTTP API. Types and low-level clients are
generated from the OpenAPI spec served by `litegen-core` at `/openapi.json`;
the ergonomic `LiteGenClient` facade is hand-written on top of that.

## Packages

- [`typescript/`](./typescript) — `@litegen/sdk` (npm). Universal (Node 18+,
  browsers, Deno, Bun). Native `fetch`. ESM + CJS build via `tsup`.
- [`python/`](./python) — `litegen` (PyPI). Python 3.10+. Sync and async
  clients on top of `httpx`. Pydantic v2 models from `openapi-python-client`.

## Regenerating from `litegen-core`

```bash
./scripts/regen-all.sh
```

The script:
1. Boots `litegen-core` (or uses `LITEGEN_BASE_URL` if already running).
2. Curls `/openapi.json` into `sdks/openapi.json`.
3. Runs `openapi-typescript` for the TS SDK.
4. Runs `openapi-python-client` for the Python SDK.

CI fails if `git diff sdks/` is non-empty after running this script — the
canonical source of truth for types is `litegen-core`, and the SDKs must
keep up.

## Development

Each package has its own README and test/build scripts. Both ship example
scripts under `examples/`.
