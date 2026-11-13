# Surfaces and leftovers — design

Round 5 of the known-gaps work (the user, 2026-11-03: "continue until no gaps
or issues remaining"). The design is delegated; every decision here is mine.

## Why

The code now does more than its surfaces say, and earlier rounds left minors
behind:

- providers that serve no catalog model (Bedrock image and video, OpenAI and
  Google video) still offer dashboard credential forms with no hint that
  nothing runs on them;
- the landing site's provider data hard-codes a provider list that has
  drifted (Bedrock in, OpenAI and Google listed as making video);
- the README's intro, provider table, roadmap and structure predate most
  vendors;
- `apps/price-api` seeds 7 of the 20 catalogued providers;
- `cargo clippy --all-targets -- -D warnings` fails (166 findings on
  2026-11-02);
- 96 minors are deferred across the ledgers in `.superpowers/sdd/`.

## Decisions

1. **Providers that serve nothing say so.** The dashboard's Providers page
   marks a provider with no served model (catalog or runtime): "No models
   run on bedrock yet. A gateway admin can add one under Gateway → Models",
   since a runtime model can run on it (Veo and Sora adapters work;
   Bedrock's speaks the retired Nova API). No adapter is deleted. The page
   already lists each provider's models, so it needs no new field on
   `GET /v1/providers`; it does need every served model, not only the org
   pool's (it edits that pool, and a model the pool left out vanished from it
   on reload), so `GET /v1/models?all=true` lists every served model.
2. **The landing site derives its providers from the catalog**, offline:
   `sync-capabilities` reads the shipped `models/*.yaml` when no gateway is
   given, so the committed snapshot never lists a mock or a provider with no
   rows. `site.ts`, the InfraFlow nodes and the test read that snapshot
   instead of hard-coding 18 providers.
3. **The README** is brought to what ships: every catalogued provider in the
   table with its families, the roadmap without vendors that are integrated,
   and the project structure as it is.
4. **The price API seeds every catalogued provider** from `models/*.yaml`.
5. **Clippy passes.** Findings are fixed in place; `src/mesh/` is not
   touched, so its findings are allowed at the module's declaration in
   `lib.rs` with a comment pointing at the mesh rule. Test-only findings are
   fixed like any other.
6. **Deferred minors**: each is fixed, or closed with a ruling when it is
   fixed already, by design, outside this tree's reach (a live vendor run, a
   vendor's unpublished price) or inside `src/mesh/`.

## Testing

Each change carries its test (Rust, vitest, node:test for the landing
scripts). The final gate: cargo with Postgres, clippy with `-D warnings`,
dashboard vitest/eslint/tsc, the landing tests, both SDK suites, the browser
suites.
