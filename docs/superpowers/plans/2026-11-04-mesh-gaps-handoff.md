# Mesh gaps: handoff to the parallel sessions

Written 2026-11-04 (about 17:00) by litegen-fc, for the sessions that finish the
mesh-gap items. Each session gets one item, named in its kickoff line. Read this page in
full before you touch anything.

To start a session, paste:

> You are the session for mesh-gap item N. Read
> `docs/superpowers/plans/2026-11-04-mesh-gaps-handoff.md` in full and follow it. Your
> row in its Assignments table is item N.

If your kickoff has no item number, or still says "N", claim the first unclaimed item in
this order: 10, 3, 6, 7, 8 (item 10 first: item 5 is done, and it's the last scene-model item). An item is unclaimed while its Status cell in the
overview's table doesn't name a session.

1. Claim it with one Edit. Several rows share the same Status text, so the old text runs
   from that row's `[plan](…)` link to the end of the row, which matches one row only.
   The new text is the same, with the Status cell replaced by
   `in progress (<your session name>)`. If the Edit fails because the row changed,
   another session got there first: take the next item.
2. Message every litegen peer and litegen-fc with the item you took.

Item 4 went to litegen-b2 this way at 17:1x, and it closed on 2026-11-05.

## Where things stand

- The [overview](../specs/2026-11-04-mesh-gaps-overview-design.md) binds you. It holds
  the index, the shared rules, the bar for "done" and decisions D1–D6. On *who* and
  *when*, this page is newer than the overview. On *what* and *what counts as done*, the
  overview and your spec win.
- Items 1, 4, 9, 11 and 12 are done but not committed. Don't reopen them.
- **Item 11 closed at 18:27.** litegen-b2 finished it: the final review, one fix wave and
  a clean re-review. On tree ba778e7 the full gate passed, the harness checked 798 of 798
  cases, and MultiUVTest passed 228 of 228. Its files are released. The harness is free
  (since 18:26).
- **Item 4 is done (litegen-b2, 2026-11-05).** On tree 6879167 its full gate passed:
  lib 2366/0, the three load-sensitive tests alone 3/3, `--tests --no-fail-fast` with
  Postgres 3218/0 over 56 suites, clippy clean, isolation 3/3. The harness passed 912/912
  on every fixture, and the 16 Khronos imports 1,824/1,824. Its plan's status block lists
  the files to commit and rulings A1–A49. It holds no file now, and the harness is free.
  - What it leaves for you is under "Interactions" (items 3, 5, 6 and 10) and "Roles that
    moved" (OpenAPI texts and matrix cells).
- **So every remaining item can start every task.** No item session holds a file. Item 10
  alone waits, for item 5. Shared files still go one session at a time, in the
  "Shared files" order below.
- **Item 5 is done (litegen-fc, 2026-11-07), not committed.** On tree f84419f its gate
  passed for item 5 (lib 2,507, the three load-sensitive alone 3/3, 55 integration suites
  with Postgres 859, clippy clean; two `providers::config_conformance` failures were item
  2's `mock_data/` folder, which item 2 is fixing), and the release harness passed every
  fixture (1,026/1,026) and the `--import` set (798/798). Its plan's status block lists the
  files to commit and the rulings R1–R34. It holds no file now, and the harness is free.
  - For item 10: `Scene.lights/cameras`, `Capabilities.lights/cameras` and
    `LossKind::Lights/Cameras` are the pattern for points and lines; the harness has
    `camera` and `lit_lights` modes, native camera/lit passes and a numeric oracle
    (`scripts/mesh-verify/camlight-oracle.mjs`).
- **litegen-d0's engine-boundary refactor landed (2026-11-07, uncommitted; no behaviour
  change).** It moved code, and the old paths still compile through `pub use` re-exports:
  - the content types, `model3d_content_type` included, went to `store/mod.rs` (item 6's
    `usdc` media type goes there now);
  - `providers/config_conformance.rs` went to `proxy/config_conformance.rs`;
  - the delivery lease went to `proxy/delivery_lease.rs` (`model3d_deliver.rs` keeps the
    delivery itself).
  A new `src/engine_boundary_tests.rs` runs on every `cargo test`, and
  `.claude/rules/engine-boundary.md` binds it: a new file in `proxy/` must go on
  `ENGINE_PROXY_FILES` or `SERVER_PROXY_FILES`. Read that rule before you add a file
  outside `mesh/`.
- **Item 2 is done (litegen-b2, 2026-11-08, tree 6fbad32), not committed.** One
  Draco-limits follow-up is open: B41, in its plan's status block.
- **Item 10 is done (litegen-fc, 2026-11-08, tree fe679b6), not committed.** Its plan's
  status block gives the proof (gate and release harness), the files to commit, and rulings
  I10-R1–R25. It holds no file, and the harness is free.
- **Item 3 is done (litegen-fc, 2026-11-09, tree e8270b9), not committed.** Its plan's
  status block gives the proof (gate and release harness), the files to commit, and rulings
  K1–K21a. Miri and a `basisu` build wait on the user. It holds no file, and the harness is
  free.
- **Item 8 is done (litegen-fc, 2026-11-09, tree 7e02650), not committed.** The opt-in
  `dae_morphs=keep` writes Collada morph controllers. Its plan's status block gives the
  proof (a SceneKit spike, the gate, and the release harness with `--dae-keep`), the files
  to commit and rulings DM1–DM23. The harness gained `--dae-keep`, xmllint against COLLADA
  1.4.1 (CI installs it), and a SceneKit renderer that plays morpher animation. It holds no
  file. Items 6 and 7 still have no session.
- **The light-units option is done (litegen-fc, 2026-11-09, the same gate), not committed.**
  It isn't one of the twelve items: the opt-in `fbx_light_units` / `usd_light_units` =
  `blender` writes and reads Blender's light units, confirmed in Blender 5.2.0. Its plan,
  `2026-11-08-light-units-option.md`, has the status block, the files and rulings L1–L11.
  One follow-up is the user's call (L11): an operation's `source_model` is still read with
  the default units and colour space.
- **Two other sessions build from this tree (2026-11-05).** Include both in your "green"
  rounds:
  - **litegen-bd** patches seven security findings in `api/handlers/{mod,orgs,users,oauth}.rs`,
    `api/middleware/mod.rs`, `util/ssrf.rs`, the db owner-transfer methods,
    `proxy/storage.rs`'s S3 `from_config` bodies, and the dashboard Login and Signup pages.
    Its regions in `api/handlers/mod.rs` avoid `generate_3d`, the 3D request validation
    and `option_needs_keeping`. Items 3, 6, 7 and 8: message it before you edit
    `api/handlers/mod.rs` or `proxy/storage.rs`. Ledger:
    `.superpowers/sdd/2026-11-05-security-fixes/`.
  - **litegen-d0** builds `litegen-core/src/mesh/` as its own crate, in `packages/3d-converter/`
    with its own target folder. It edits no shared file, but its build reads your
    in-flight `mesh/` edits, so keep `mesh/` compiling between edits.
- **Disk:** about 25 GiB free after item 4 closed (48 GiB at 06:25). `target/` is 19 GB
  and litegen-d0's `packages/3d-converter/target` 3.3 GB. The harness's output matrices
  are the big item: item 4's control and import matrices came to over 20 GB before
  litegen-b2 deleted them, leaving its ledger at 1.8 GB. Once a run's report and log
  carry the proof, delete its control and import matrices. Keep the reports, your closing
  matrix and the small `mesh_matrix` binaries behind your logged results. `target/` was 46 GB until
  litegen-fc deleted, with the user's word, litegen's own build outputs from before
  Nov 4 and the unused `incremental/` folder. Before a release build or a
  harness run, check `df -h`. Stop and tell the user below 8 GiB. More sessions mean more
  builds, so run `cargo clean` only with the user's word.
- Item 11's follow-ups, outside its done bar, sit in its ledger as Ruling F2. Two belong
  to items here:
  - item 6 adds `usdc` to `USD_FORMATS` and `SCENEKIT_FORMATS` in `verify.mjs`;
  - items 7 and 8 update the FNV pins their writer changes move.
- The sessions that held today's shared roles have ended: litegen-4e (it kept the
  overview), litegen-d8 (frontend and SDKs) and litegen-a3 (the matrix page). Their
  roles are reassigned below.

## Assignments

Every item uses the same slug in three places: the spec
`docs/superpowers/specs/2026-11-04-<slug>-design.md`, the plan
`docs/superpowers/plans/2026-11-04-<slug>.md`, and the ledger
`.superpowers/sdd/2026-11-04-<slug>/`.

| Item | Slug | Start now | Then | Also yours |
|---|---|---|---|---|
| 2: Draco and meshopt input (litegen-b2) | `mesh-compressed-gltf-input` | **Done 2026-11-08**, not committed | B41 (Draco limits), open to any session | — |
| 3: KTX2 textures (litegen-fc) | `mesh-ktx2-textures` | **Done 2026-11-09**, not committed | Miri and `basisu`, on the user's word | Ruling A6 settled (K1) |
| 4: Material layers (litegen-b2) | `mesh-material-layers` | **Done 2026-11-05**, not committed | — | — |
| 5: Lights and cameras (litegen-fc) | `mesh-lights-cameras` | **Done 2026-11-07**, not committed | — | — |
| 6: USDC, clips, unlit (litegen-fc) | `mesh-usd-usdc-clips-unlit` | **Done 2026-11-10**, not committed | Nothing: the user skipped the AR Quick Look device check on 2026-11-10, so USDZ's `usd_clips` default stays `first` | Item 11's `usdc` gaps in `verify.mjs` closed |
| 7: FBX layers, ASCII | `mesh-fbx-layers-ascii` | Tasks 1–3 | Task 4 (the API, then a harness turn) and Task 5 | — |
| 8: Collada morphs (litegen-fc) | `mesh-dae-morph-output` | **Done 2026-11-09**, not committed | DM4 (a SceneKit clip-name opt-in), open to any session | Item 5's clip-name note settled as DM4 |
| 10: Points and lines (litegen-fc) | `mesh-points-lines` | **Done 2026-11-08**, not committed | — | The steward role (litegen-fc keeps it) |

Items 4 and 11 are closed, so no task waits for either now. Several items edit the same
files, so take them one at a time in the "Shared files" order, agreeing regions by
message.

## Skills you must use

Invoke each one with the Skill tool at the point named. None of them is optional.

1. **`superpowers:subagent-driven-development`** runs your plan. Each task gets an
   implementer, then a spec review, then a quality review, with fix rounds until both
   reviews are clean. A final review covers the whole item. If you implement inline,
   use `superpowers:executing-plans` instead. Skip two of its steps:
   - its worktree step, because `.claude/rules/working-in-this-repo.md` forbids
     worktrees;
   - its commit step, because nothing is committed on this laptop. Reviews diff tree
     snapshots instead (see "How each task runs").
2. **`superpowers:test-driven-development`** governs every behaviour change, in your
   session and in every implementer's. Subagents don't load skills on their own, so
   every implementer brief must say: "Invoke superpowers:test-driven-development before
   you write code, and superpowers:verification-before-completion before you report."
3. **`superpowers:verification-before-completion`** comes before every claim that
   something passes, is fixed or is done. That covers the ledger, messages to peers,
   the overview and your report. The evidence is a command and its output from the
   current tree, not a memory of an earlier run.
4. **`superpowers:systematic-debugging`** comes first when a test fails for a reason you
   didn't predict. Run it before you change any code.
5. **`model3d-provider`** (it loads `provider-standards`) applies when you touch any of
   these, which items 2, 3, 6, 7 and 8 do:
   - `providers/model3d/`;
   - `models/mock.yaml`;
   - delivery;
   - the 3D request API.

Subagents run on the session model. Don't pass `model: sonnet` before 2026-11-07
08:00 ET: the account is over its weekly Sonnet limit and gets a 429 until then.

## First 15 minutes

1. Start every Bash call with `export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH;`.
2. Read these in full:
   - the overview;
   - your spec and your plan;
   - `.claude/rules/mesh-standalone.md`;
   - the research notes your spec cites (`docs/superpowers/research/`).

   Items 3, 6, 7 and 8 also read the code of item 1's opt-in plumbing. The overview
   names the functions under "Opt-in options reuse item 1's plumbing".
3. Run `ListAgents`, then message every litegen peer with:
   - your item;
   - the files and regions your next tasks edit;
   - what you're waiting for.

   Read the peers' messages. For every file under "Shared files", agree the region
   before you edit it.
4. Make your ledger at `.superpowers/sdd/2026-11-04-<slug>/progress.md`. Give it a Setup
   section like item 9's (`.superpowers/sdd/2026-11-04-mesh-ply-big-endian/progress.md`).
   Copy the `snap` and `pkg` scripts from item 11's ledger
   (`.superpowers/sdd/2026-11-04-mesh-uv-sets-usd-dae/`) and point their `ws` at your
   own folder.
5. In the overview's table, set your row's Status to `in progress (<your session name>)`.
   Edit only your own row.
6. Mutation testing is banned (2026-11-09), so no session should leave a `NEGCTL` line. If
   `grep -rn NEGCTL litegen-core/src scripts/mesh-verify` still finds one, ask its session
   before you build.

## How each task runs

- **Snapshot first.** Run `./snap` before the task and record the BASE tree in your
  ledger. Run it again after the task for HEAD. `./pkg BASE HEAD <out> <paths…>` writes
  the review package.
  - `snap` writes through a private index (`GIT_INDEX_FILE=<ledger>/snap.index`), so the
    shared index stays empty.
  - Never run `git add` without that variable set.
- **RED, with the tree still compiling.** Write the tests your plan names, against a
  stub if needed, and run the plan's exact command.
  - RED must be an assertion failure, never a compile error, because other sessions
    build this tree.
  - Record the command, the failing test names and the failure line.
  - A named test that passes before the change isn't RED. Find out why before you go
    on.
- **GREEN.** Implement the least code that passes. Rerun the same command and record
  its `test result:` line.
- **No negative controls (the user's rule, 2026-11-09, global `CLAUDE.md`, "No mutation
  testing").** Plant no deliberate bug to show a test can fail, whether by hand, with a
  tool such as `cargo mutants`, or through a subagent, whatever a spec, plan or peer asks.
  Every "Negative control" step in items 6 and 7 is void. The RED above is the proof. For a
  test of behaviour that already exists, write in the ledger which regression it would
  catch.
- **Hostile input.** Every reader path you add gets two sweeps:
  - a truncation sweep over every prefix;
  - a bit-flip sweep: every bit of the first 4 KiB, plus at least 10,000 seeded flips
    elsewhere. Record the seed.

  Neither may panic, and each case finishes in under a second in release. Allocations
  are charged against a named budget before they happen.
- **Clippy after every slice of work:**
  `cargo clippy --all-targets --all-features --keep-going -- -D warnings`. With
  `--keep-going` you can see whether a break is yours or a peer's.
- **Review.** A spec review, then a quality review, over the package. Fix rounds
  continue until neither review has a Critical or Important finding. Record each minor
  you defer, with the reason.
- **Ledger.** Write one line per event:
  - the BASE and HEAD trees;
  - dispatches;
  - RED, GREEN and controls;
  - review verdicts;
  - rulings, in the form "Ruling: … — cost if wrong: …";
  - messages that changed the plan.

## Shared resources

### Cargo

- The repo rule says to ask before running cargo. When the user hands you this page,
  that's their go-ahead for the cargo runs your plan names, under the protocol below.
- To wait for a peer's build, run
  `until ! { pgrep -qx cargo || pgrep -qx rustc || pgrep -qx clippy-driver; }; do sleep 20; done`.
  Never use `pgrep -f`: it also matches other sessions' waiting shells, and two waiters
  then deadlock.
- Always set `CARGO_INCREMENTAL=0`. Focused runs (`cargo test --lib -- mesh::ktx2`) can
  interleave with peers'.
- Put several test filters after `--`: `cargo test --lib -- mesh::usd mesh::dae`.
- **Before a full gate or a release build:**
  1. Message every session that edits `litegen-core/src/`.
  2. Wait for each one's "green": it compiles, and no test is red by design.
  3. Send "starting", run, then send "done".

  When two items are ready at the same time, run one gate for both. Share the log paths
  and the src sha256s, and each session records the counts in its own ledger.
- After the release build, copy `target/release/examples/mesh_matrix` into your ledger
  folder and send "release build done". Peers can then edit `src/` again while you run
  the node harness from your copy.

### The harness

Every item's last task edits `scripts/mesh-verify/**`. Two harness runs at once make
usdrecord draw blank pictures. So one session holds the harness at a time:

- Before you edit or run it, ask "harness?" and wait for the holder's "harness
  released".
- Announce "harness: item N" when you take it. Announce "harness released" once your
  run's `summary` is in your ledger.
- Turns go first come, first served. Hold it for one edit-and-run cycle. If you need
  another round, queue again.

### Shared files

| File | Who edits it, in order |
|---|---|
| `mesh/glb.rs` | 2 (Tasks 2 and 4), 10 (Task 2) |
| `Cargo.toml`; `mesh/isolation_tests.rs` (`MODULE_FILES`, `ALLOWED`); the module list in `mesh/mod.rs` | 2 and 3 |
| `providers/model3d/mock.rs`, `models/mock.yaml`, `proxy/model3d_deliver.rs`, `tests/model3d_api.rs` | 2 and 3, then 6, 7 and 8 |
| `api/handlers/convert.rs`, `api/handlers/mod.rs`, `types/mod.rs`, `WriteOptions`, `DeliveryPlan` | 3, 6, 7 and 8. Each adds its own field, and adds only. Message litegen-bd before `api/handlers/mod.rs` |
| `mesh/fbx_read.rs` | 7 (Tasks 1–2), then 10 (Task 5) |
| `mesh/normalize.rs` (`transcode_textures`; `place`) | 3 (Task 4, on top of item 4's change: see "Interactions"), then 10 (Task 1: `place` uses `world_bounds`) |
| `scene.rs`, `fixtures.rs`, `pose.rs`, the capability rows in `mod.rs`, `LossKind` | 10 (and item 2's B28 in pose.rs's texture-transform pass), by region |
| `usd_read.rs`, `usd_write.rs`, `usd.rs`, `usd_stage.rs`, `usda.rs`, `usdc.rs` | 6 (Tasks 0–4), then 10 (Task 4) |
| `dae.rs` | 8 (Tasks 1–2), then 10 (Task 4) |
| `fbx.rs`, `fbx_write.rs`, `fbx_dom.rs` | 7 (Task 3), then 10 (Task 5) |
| `docs/superpowers/3d-format-conversion.md`, the lossiness table in `mesh/mod.rs` | Every item's docs task: only your codec's section and your §9 bullet |

To edit a shared file:

1. Agree the region by message.
2. Re-read the file just before you edit it.
3. Make small replacements with the Edit tool.

Never write a shared file whole, and never reformat it. Don't run `cargo fmt` over the
crate: run `rustfmt` only on files no one else edits. Never run `git checkout`,
`git restore` or `git stash`: each one wipes peers' work.

### Changes the API shows

These items change the OpenAPI:

- item 3: `ktx2_textures`;
- item 6: `usdc`, `usdz_root` and `usd_clips`;
- item 7: `fbx_encoding`;
- item 8: `dae_morphs`;
- items 5 and 10: their loss kinds;
- item 4 (landed, awaiting regeneration): two description-only changes. The
  `SceneMaterial.maps` doc in `types/mod.rs` names the layer maps (`clearcoat`,
  `sheen_color`, …), which are new string values, not a new field (Ruling A10). The
  warnings-header example at `api/handlers/convert.rs:164` now says "(lights, material
  variants, …)" (Ruling A18). The old and new texts are in
  `.superpowers/sdd/2026-11-04-mesh-material-layers/task-6-report.md` §3.

Before you land such a change, send the frontend/SDK owner (see "Roles that moved") the
exact shapes: the field name, type, enum values, default, the 400 codes, and the OpenAPI
description text.

Once the tree builds, the owner regenerates once for every change ready by then. It runs
`sdks/scripts/regen-all.sh`, then `node apps/landing/scripts/sync-openapi.mjs`, then the
SDK suites and the dashboard tests.

After your own change, regenerate `docs/mcp/tools.md` as your plan says (from
`litegen-core/`: `cargo run --example mcp_tools_doc > ../docs/mcp/tools.md`). Items that
touch the API run `cargo test --tests` with `LITEGEN_PG_TEST_URL` set, against a
throwaway Postgres container (`.claude/rules/database-tests.md`).

## Roles that moved

### Closing item 11: the item 4 session (done, 18:27)

litegen-b2 closed item 11 by the steps below. They stay here as the record.

Item 4 can't start until item 11 releases its files, so closing item 11 costs it no
time.

1. Ask the peers whether any session has resumed litegen-37's thread. If one has, that
   session finishes item 11, and you wait for its "item 11 closed".
2. Read item 11's spec, plan and ledger. In the ledger
   (`.superpowers/sdd/2026-11-04-mesh-uv-sets-usd-dae/progress.md`), the part from
   "Task 4" on matters most.
3. Run the final whole-item review over `review-final.diff` (trees `cd256b1..4483d84`),
   with the deferred minors the ledger lists. Fix:
   - every Critical and Important finding;
   - the minor marked "likely must-fix": the docs say SceneKit judges every uv set past
     the first two, but it judges only sets 2 and 3.

   Code fixes go through TDD.
4. Compare item 11's paths in the live tree with tree `4483d84`.
   - If src under them changed, rerun `cargo test --lib mesh::` and the full gate. Batch
     the gate with a peer's.
   - If `scripts/mesh-verify` changed, rerun the harness too.
5. At the top of item 11's plan, add a status block in the form items 1, 9 and 12 use:
   "**Status: done on <date> by session <name>, finishing litegen-37's work, not
   committed.**" Follow it with what differs from the plan and the files to commit.
6. Set item 11's overview row to **done**, and post "item 11 closed, files released" to
   every peer.

### Frontend/SDK owner

**litegen-d0 holds the role (2026-11-08).** A regeneration builds from the whole tree.
d0's run at 17:16 on 2026-11-08 therefore took every text landed by then: items 4, 5 and
10, and litegen-bd's (the list below). Send it the exact shapes before you land an OpenAPI-visible change,
then send "settled" once your texts are final. It regenerates after that and lands any
hand-written SDK option itself. The rest of this section is the earlier rule, kept as the
record.

If a frontend/SDK session is running, it keeps the role; ask in your first round of
messages. If none is, the item 10 session takes the role for this batch. It then holds:

- `dashboard/**`, `sdks/**` and `apps/landing/**`;
- the regenerated OpenAPI and SDK files.

Until one of them exists (none did on 2026-11-05), send your shapes to the user and
record them in your ledger. Waiting for the first regeneration (all in d0's 2026-11-08
run):

- item 4's two description texts (Rulings A10 and A18), above under "Changes the API
  shows";
- litegen-bd's security fixes: error codes and descriptions only, listed in
  `.superpowers/sdd/2026-11-05-security-fixes/`;
- item 5's texts: the new loss kinds `lights` and `cameras`, the converter's losses
  header and `fbx_color_space` descriptions, and the SDK doc comments' "known kinds"
  lists, old and new, in `.superpowers/sdd/2026-11-04-mesh-lights-cameras/openapi-texts.md`.

### Steward: the item 10 session

Items 10 and 3 are done (2026-11-08 and 2026-11-09). Their session, litegen-fc, keeps the
role. The steward:

- keeps the overview's "What can start now" list current from the peers' messages. Each
  session still edits only its own Status cell;
- owns the matrix page data (`.superpowers/sdd/2026-11-04-3d-matrix-page/build.mjs`).
  Sessions send it their changed cells. The 2026-11-08 refresh applied items 11, 4, 5, 10
  and 2 and the light-units cells, and item 3's cells follow its gate; the page's ledger
  records each. The page was last published 2026-11-04, so the refresh waits for a
  publish;
- tracks the harness queue and the gate batches, and nudges a session that holds the
  harness or a freeze too long.

If a session you depend on ends or goes quiet for an hour, read its ledger and tell the
user. Don't take over its files without the user's word. Item 11 is the one exception,
and this page settles it.

## Interactions to settle before you code

- **Items 2 and 3** both edit `mock.rs`, `models/mock.yaml`, `model3d_deliver.rs`,
  `Cargo.toml` and `isolation_tests.rs`. Agree regions first, as items 1 and 12 did.
  Each item adds only the crate its decision allows (D2: `draco-core =2.3.3`; D3:
  `ruzstd =0.9.1`), under its own `ALLOWED` entry. Any other crate is the user's
  decision.
- **Items 3 and 4: Ruling A6 is still provisional.** Item 4 is done, and its test
  `a_ktx2_layer_texture_is_dropped_and_its_layer_kept` (normalize.rs:1091) is in the
  tree. It assumes a KTX2 layer texture is dropped, while item 3 makes the production
  paths transcode KTX2 to PNG. litegen-b2 proposed this settlement, recorded in item 4's
  ledger as Ruling A6:
  - The test uses a KTX2 whose header names zlib supercompression (scheme 3). Item 3's
    spec refuses that scheme by name (`zlib_supercompression_is_refused_by_name`,
    mesh-ktx2-textures-design.md:183). It also says a KTX2 that fails to decode "is
    still dropped and reported as today" (:71-72).
  - So the test expects the same outcome before and after item 3: the texture dropped,
    its layer kept untextured, and a loss reported. It asserts only the note's stable
    part, not the decoder's reason, which item 3 adds.
  - If item 3 wants a case with a decodable KTX2 in a layer, it adds one to its own
    tests.

  **Item 3's session:** agree to A6, or change that one assertion yourself and record
  why as a Ruling. Tell litegen-b2 if it's still running; either way, record it in your
  ledger.
- **Item 3 and `normalize::transcode_textures`.** Item 4's change there is in: the
  dead-texture drop walks `Material::texture_slots_mut()`, not a five-slot list. Item 3's
  Task 4 rewrites the KTX2 path on top of it. Keep the slot walk.
- **Item 6 and item 4's USD reads (Ruling A17, agreed by litegen-fc).** Item 4's Task 3
  went ahead on today's `Value` types, in these places:
  - `usd_read.rs`: the clearcoat read (about :1450), the ior read, and their tests;
  - `usd.rs`: capabilities, `prepare` and `losses`;
  - `usd_write.rs`: `write_material`.

  **Item 6's session:** in Task 0, change by hand every one of those `Value::Str`
  matches that should also accept `Value::Token`, and record each change in your ledger.
- **Items 5 and 10 build on item 4's scene types.** From litegen-b2's closing notes:
  - `Material` carries `clearcoat`, `transmission`, `volume`, `specular`, `sheen`,
    `iridescence`, `anisotropy`, `ior` and `dispersion`. `Capabilities.layers` is a
    `LayerSupport` of nine bools. Keep `LayerSupport` additive. When you add `lights`,
    `cameras`, `points` or `lines` to `Capabilities`, follow the pattern of `layers`.
  - A container below Phong keeps `layers == LayerSupport::none()`, because `degrade`'s
    ColorOnly arm drops every layer.
  - A material texture pass uses `Material::texture_slots()` and `texture_slots_mut()`,
    which cover every slot, layers included. Never walk the named fields.
  - `scene_matrix_tests` compares layer values f32-exact. A new fixture with an off-value
    layer needs its codec's `prepare` to drop it (item 4's ledger, Ruling A21b).
  - Since item 4's final fix wave, a `verify.mjs` USD case with no geometry judge fails.
  - The harness's `lit` mode exists: `--modes lit`, `lit_coat`, a tolerance of 1/255.
- **Items 2 and 5 share `glb.rs`.** Item 5's Task 1 (naming glTF cameras in the warnings)
  touches only the reader. Item 2's Task 2 edits `view_bytes` in the same file. Agree
  regions first.
- **Item 6 and the harness.** Item 11's ledger notes that `verify.mjs` leaves `usdc` out
  of both `USD_FORMATS` and `SCENEKIT_FORMATS`. Item 6's Task 2 adds it to both.
- **Items 3, 6, 7 and 8** each add a field to:
  - the converter's `PARAMS`;
  - the 3D request type;
  - `DeliveryPlan`;
  - `WriteOptions` (items 6, 7 and 8 only).

  Put yours next to item 1's `packages`, and only add. Reuse `option_needs_keeping` and
  `write_keyed` rather than copying them.
- **Item 8 and SceneKit's clip names** (found on 2026-11-08 by item 5's harness follow-ups;
  see `.superpowers/sdd/2026-11-04-mesh-lights-cameras/followups-2026-11-08-report.md`, D).
  - litegen's Collada writer names a clip after its animation. SceneKit's Collada importer
    knows a clip by its name and registers none whose name is the id of a node, a geometry
    or an animation.
  - So an app that plays clips through SceneKit gets nothing for such a clip. Khronos's
    CesiumMilkTruck, whose animation and a node are both `Wheels`, is the case.
  - The harness now judges that clip with three.js alone.
  - A writer fix would give the clip a name no id holds. That changes a clip name other
    readers show, so under the rule below it's an opt-in. Item 8 owns this writer and
    starts with a SceneKit spike, so it decides.
  - **Settled (item 8, 2026-11-09, Ruling DM4):** not built in item 8. Its spec doesn't
    include it, and today's names stay the default. The opt-in is an open follow-up in item
    8's plan status block. Under `dae_morphs=keep` the harness fails a `<morph>` file whose
    clip SceneKit shadows this way (DM18), since nothing else would judge its animation.
- **New decisions.** D1–D6 are settled. For anything else:
  1. Take the choice that keeps today's output as the default and gives users an
     opt-in.
  2. Record it as a Ruling, with its cost if wrong.
  3. Name it in your report.

  Stop and ask the user only about a new crate, a licence question, or a change to
  settled behaviour.

## Done, and the proof

An item is done only when every line of the overview's "What 'done' means" (1–9) holds,
and the ledger shows the evidence for each line:

1. For each test the spec names: its RED command and failure line, and its GREEN
   `test result:` line.
2. ~~For each negative control…~~ Void since 2026-11-09 (no mutation testing). Instead,
   for each test of behaviour that already existed, record which regression it would catch.
3. `cargo test --lib mesh::` passing, and the matrix count showing your new content in
   every pair and both write modes.
4. The full gate's four commands with their `test result:` lines and clippy's exit
   status, plus the src tree hash (from `./snap`) they ran on.
5. `mesh::isolation_tests` passing, and your read of the diff for `crate::`, an escaping
   `super::` and outside crates.
6. The harness report's `summary` block, pasted in. It shows 0 failures, and every new
   skip cites a reader's source line. The ledger also records your item's third-party
   set run with `--import`.
7. For the sweeps: the counts, the seed and the slowest case.
8. Each doc the bar lists, updated. For a doc that needs no change, record the grep that
   shows it.
9. For the API items: the openapi tests, the SDK suites after regeneration, the MCP tool
   and doc tests, and dashboard vitest and Playwright where the UI shows the change.

A gate run on an older tree doesn't count if your paths changed since then: rerun it. A
skipped or ignored test doesn't count unless the spec allows it and the ledger cites
why.

Then:

1. At the top of your plan, add the status block: "**Status: done on <date> by session
   <name>, not committed.**" Follow it with what differs from the plan and why, and the
   full list of files to commit.
2. Set your overview row to `**done** (<name>), not committed`, and tell the steward
   what that unblocks.
3. Post "item N done, released <files>" to every peer.
4. Report to the user:
   - the evidence above, one line per point, with the ledger path;
   - the minors you deferred;
   - your Rulings;
   - the files to commit.

   Never commit or stage anything.

## Never

- Never use a worktree, `EnterWorktree` or a second checkout.
- Never commit, never stage anything in the shared index, never stash, and never run
  `git checkout` or `git restore` on a file.
- Never plant a mutant (the user banned mutation testing on 2026-11-09). Never `mv` a file
  back over an edit either, since the old mtime keeps the old build.
- Never run two harnesses at once, and never run cargo during a peer's announced freeze.
- Never edit a file another session holds without its agreement.
- Never keep a note only in `/private/tmp`, and never name a file `claude.md`.
- Never claim a pass without the output that shows it.
