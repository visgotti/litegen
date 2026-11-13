# Working in this checkout

## One checkout, no worktrees

Do all litegen work in this checkout. Don't create a git worktree, call
`EnterWorktree`, or work in a sibling or scratch clone, even for a branch of
your own. If a skill or plan says to make one (for example
`superpowers:using-git-worktrees`), skip that step and say why.

Why: a Claude session that enters a worktree drops out of this folder's
thread history. Its work also sits apart from everyone else's until someone
merges it back by hand. The MCP server work was stranded that way in November
2026.

## Several Claude sessions share this tree

The user often runs several sessions at once in this one checkout, split by
area:

- backend: `litegen-core/**`, `models/*.yaml`;
- frontend and SDKs: `dashboard/**`, `sdks/**`, `apps/landing/**` and the
  regenerated OpenAPI and SDK files;
- model drift: `scripts/model-drift/**`;
- skills: `.claude/skills/**`.

All of them share one git index and one `target/`.

- At the start, list the other sessions (`ListAgents`). Agree on who owns
  which files before you edit anything.
- Never leave anything staged.
- Commit only when the user asks, and name the paths:
  `git commit -m … -- <paths>`. A plain `git add -A` or `git commit -a` sweeps
  up another session's unfinished work.
- Ask before running cargo, and never run it while another session does. A
  build compiles everyone's edits, and two at once have filled the disk. Use
  `CARGO_INCREMENTAL=0` when disk space is short.
- Build with the pinned Rust. `rust-toolchain.toml` pins 1.99.1, and
  packages/3d-converter pins the same version plus the wasm target. rustup
  reads the pin, but Homebrew's cargo (1.98.1) ignores it. So put
  `/opt/homebrew/opt/rustup/bin` first in `PATH`: cargo finds clippy and rustc
  through `PATH`, so calling the proxy's cargo by its full path still runs
  Homebrew's clippy. `cargo clippy --version` should print 0.1.99. Two
  toolchains in one `target/` keep two sets of build outputs.
- Before an OpenAPI-visible change (a route, a field, a header), send the
  exact shapes to the frontend/SDK session. Once the tree builds, that session
  regenerates once: `sdks/scripts/regen-all.sh`, then
  `node apps/landing/scripts/sync-openapi.mjs`.
- `scripts/model-drift/check.mjs --snapshots <dir>` rewrites every snapshot it
  fetches. Except during the weekly drift run, pass it a temporary copy of the
  snapshots. Compare `git status --short scripts/model-drift/snapshots` before
  and after the run. Once a snapshot is overwritten, another session's
  uncommitted edits to it can't be recovered.

## Where notes go

- Work ledgers go in `.superpowers/sdd/`. Git ignores that folder through its
  own `.gitignore`, but it travels with a copy of the folder.
- Specs and plans go in `docs/superpowers/{specs,plans}/`.
- Never keep a note only in the session scratchpad under `/private/tmp`.
  macOS empties it on every restart.

## Gotchas

- **Restore a mutated file by rewriting it, never with `mv file.orig file`.**
  `mv` brings back the file's old mtime, so cargo keeps the mutant build. This
  happened to a file embedded with `include_str!`. If a restored test still
  fails, `touch` the file and rerun before you start debugging.
- **Never name a file `claude.md`.** The macOS filesystem ignores case, so
  Claude Code loads that file as `CLAUDE.md` instructions. For example, the
  MCP page for the Claude apps is `docs/mcp/clients/claude-apps.md`.

## SDKs and clients stay thin

The SDKs and the dashboard only do these things:

- wrap requests and responses;
- poll;
- give typed shapes from the OpenAPI codegen.

They do no 3D parsing, apply no format rules and hold no other domain logic.
When a client needs something only the server knows, expose it in the API
instead of copying the rule into the client.
`ParamSpecStringArray.derivable_values` came about this way.
