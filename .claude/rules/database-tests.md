---
paths:
  - "litegen-core/src/db/**"
  - "litegen-core/src/management/**"
  - "litegen-core/src/mcp/oauth/**"
  - "litegen-core/migrations/**"
---

# Postgres tests and migrations

## Run the Postgres tests for real

Unless `LITEGEN_PG_TEST_URL` is set, the Postgres tests skip themselves, so a
plain `cargo test` never runs any Postgres SQL. Three modules are gated this
way: `db::postgres_tests`, `management::store_tests` and
`mcp::oauth::store_tests`.

Before you call a Postgres SQL change done, run these tests against a real
database. A throwaway container is enough. Other containers may be running, so
pick a free port:

```sh
docker run -d --rm --name litegen-pg-test -e POSTGRES_PASSWORD=pw \
  -e POSTGRES_DB=litegen_test -p 127.0.0.1:55439:5432 postgres:16-alpine
LITEGEN_PG_TEST_URL=postgres://postgres:pw@127.0.0.1:55439/litegen_test \
  CARGO_INCREMENTAL=0 cargo test --lib   # all three gated modules are lib tests
docker stop litegen-pg-test
```

## Migrations

- sqlx applies every migration the database hasn't seen yet, whatever its
  number. It refuses only when the database has applied a migration that the
  source no longer contains. So a migration that arrives with a lower number
  than ones already applied still runs: 018 (`mcp_oauth`) arrived after 019
  to 022.
- Editing a migration that a database has already applied changes its
  checksum, and litegen then won't start against that database. Migration 018
  (`mcp_oauth`) was edited in place during development. A database that
  applied an older 018 has to be recreated.
