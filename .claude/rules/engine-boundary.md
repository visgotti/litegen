# The engine names no server module

`litegen-core` holds two kinds of code in one crate:

- **The engine** turns a request into a result. It holds the provider
  adapters, the catalog's capabilities and pricing, the shared types, the 3D
  library, input materializing and 3D delivery.
- **The server** wraps the engine for many users. It holds the HTTP API, auth,
  access, the database, management (keys, budgets, orgs), hooks, MCP, config
  and the background job loop.

The server may use the engine. The engine never names the server. That keeps
the engine testable without a database, and lets it be lifted into its own
crate or library as a move, not a rewrite. Design:
`docs/superpowers/specs/2026-11-07-engine-boundary-design.md`.

## What counts as which

`src/engine_boundary_tests.rs` holds the lists, and it is the source of truth:

- **Engine folders:** `ENGINE_DIRS`, which lists `providers/`,
  `capabilities/`, `types/`, `util/`, `mesh/` (which keeps its own stricter
  rules: `mesh-standalone.md`) and `store/`.
- **Engine entries in `proxy/`:** `ENGINE_PROXY_FILES` (materializing, 3D
  conversion and delivery, reference publishing, and so on). A folder there
  is listed as `name/`.
- **Server:** everything else. That covers `api/`, `auth/`, `access/`,
  `config/`, `db/`, `hooks/`, `management/`, `mcp/`, `observability.rs`,
  `lib.rs`'s own items, the `proxy` root's own items and re-exports, and the
  entries on `SERVER_PROXY_FILES` (router, poller, storage, webhook,
  retention, cache, registry, the delivery lease, and so on). Any new
  top-level module is server code until it goes on `ENGINE_DIRS`.

## The rule

- **Engine code names only the engine.** That holds for every path in it:
  - `crate::…` paths, including groups split over several lines and globs
    (`crate::*`);
  - `super::…` paths, resolved from the module they're written in. That
    includes inline modules such as `mod tests` and files included with
    `#[path]`.

  `cargo test` and `cargo test --lib` check it; `cargo test --test <name>`
  alone doesn't. A failure names the file and line.
- **A new file or folder in `proxy/`** goes on `ENGINE_PROXY_FILES` or
  `SERVER_PROXY_FILES`. The test fails until it's on one, and that's the
  moment to decide which side it's on.
- **A new engine folder** goes on `ENGINE_DIRS`.
- **An engine file may `#[path]`-include only engine files.**

## When engine code needs something the server has

- **A pure helper that lives in a server file:** move it into the engine
  verbatim, and re-export it from its old path (`pub use`), so server call
  sites don't change. `types/generation.rs`, `util/content.rs` and
  `capabilities/duration.rs` came about this way.
- **A value from config, the database or the request:** take it as a
  parameter. Or define a small engine trait that the server implements:
  `store::LinkSigning` lets a store sign its own links without the engine
  holding the secret.
- **Engine code that needs the database or config:** it is server code, so
  move it to the server side. `management/slug.rs` (from `util/`) and
  `proxy/delivery_lease.rs` (from `proxy/model3d_deliver.rs`) moved this way.
- **Storage:** the engine imports from `crate::store` (traits, local stores,
  key layout, content types). `proxy/storage.rs` keeps S3, bucket checks and
  the wiring from config, and re-exports `store` so the old paths still work.

## Don't

- Don't add a server folder to `ENGINE_DIRS`, move a server entry onto the
  engine list, or edit the scanner to make a failure go away. Where the line
  runs is a design decision, so ask the user.
- Don't route around the test. It catches the paths people write by
  accident. It doesn't chase these, and each of them still crosses:
  - an alias of the crate (`use crate as root;`);
  - a path that a macro builds;
  - `include!`.

  A re-export of a server item through an engine module is caught at the
  re-export, so it can't hide a crossing either.
