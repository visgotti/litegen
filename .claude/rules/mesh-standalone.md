---
paths:
  - "litegen-core/src/mesh/**"
  - "litegen-core/src/proxy/model3d_*.rs"
  - "litegen-core/src/api/handlers/convert.rs"
  - "litegen-core/src/providers/model3d/**"
  - "litegen-core/src/types/mod.rs"
  - "litegen-core/src/capabilities/loader.rs"
---

# `mesh` stays a standalone library

`litegen-core/src/mesh/` is the 3D format-conversion library: bytes in, bytes
out. It lives inside the `litegen` crate for now, but it must stay liftable into
its own crate by moving the folder and writing a `Cargo.toml`, with no code
changes. Every edit keeps that true.

`mesh/isolation_tests.rs` enforces part of this in CI. Follow every rule below
whether or not the test would catch the break.

## Inside `src/mesh/`

- **No path into the rest of litegen.** No `crate::` at all, not even
  `crate::mesh::`. Use `super::`. Don't let `super::` climb out of the module
  either: from `mod.rs`, `super::` is the crate root, and from a codec file,
  `super::super::` is. The test catches `crate::` but not a `super::` that
  escapes.
- **Only these dependencies:** std, `serde_json` (glb.rs, matrix_tests.rs),
  `miniz_oxide` (zip.rs, fbx.rs), `quick_xml` (xml.rs), `image`
  (normalize.rs), `draco_core` (draco.rs only; the user's decision D2 in
  mesh-gaps item 2, pinned `=2.3.3` with `["decoder",
  "edgebreaker_valence_decode"]`) and `ruzstd` (ktx2.rs only; the user's
  decision D3 in mesh-gaps item 3, pinned `=0.9.1`, `default-features =
  false`, `["std"]`). Nothing else: no serde derives, `tracing`,
  `thiserror`, `anyhow`, `tokio`, `bytes`, `async-trait`, `utoipa` or
  `reqwest`. The test only checks *where* the listed crates appear. It does not
  reject other crates, so a `tracing::warn!` or a `#[derive(Serialize)]` passes
  CI. Write errors as plain enums with a hand-written `Display`. Adding a crate
  to the list is the user's call, never a drive-by.
- **No I/O, no async** outside `#[cfg(test)]`: no `std::fs`, `std::net`,
  `std::env` and no `async fn`. Sidecar files arrive through the `Sidecars` trait
  in `package.rs` (`MemorySidecars`, `NoSidecars`), never as paths.
- **No litegen concepts:** no provider names, model IDs, catalog/YAML fields, HTTP
  statuses, job IDs or storage keys. If a conversion depends on a choice litegen
  makes, take it as a parameter, the way `WriteOptions` and `normalize::Axis` do.
- **Fixtures stay in the folder:** keep them in `src/mesh/testdata/` and load them
  with `include_bytes!("testdata/…")`. Never reach outside with `../`, a
  `CARGO_MANIFEST_DIR` path or files from `litegen-core/tests/`.
- **A new `.rs` file** goes into both `mod.rs` and `MODULE_FILES` in
  `isolation_tests.rs`.
- **Anything litegen calls is `pub`, not `pub(crate)`.** After extraction,
  `pub(crate)` items are invisible to litegen.

## Outside `src/mesh/` (the callers)

- Litegen depends on mesh, never the reverse. Translate between litegen types and
  mesh types at the boundary (proxy, handlers, providers, the catalog loader).
- No inherent `impl` on a mesh type outside `src/mesh/`. That means no
  `impl MeshFormat { … }` in litegen code.
- Don't implement a foreign trait for a mesh type unless a litegen type appears
  in the trait's parameters. For example, `impl utoipa::ToSchema for MeshFormat`,
  `impl serde::Deserialize for Axis` and `impl Display for MeshFormat` all stop
  compiling once mesh is its own crate (orphan rule). `impl From<MeshFormat> for
  types::Foo` is fine. For OpenAPI or YAML, use a litegen-side type or newtype and
  convert it.

## When a change seems to need coupling

Stop and ask the user. Don't work around this rule, and don't edit
`isolation_tests.rs` to get it passing. Removing a check or widening `ALLOWED` is
a design decision, not a fix.
