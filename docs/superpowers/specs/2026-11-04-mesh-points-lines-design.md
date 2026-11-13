# Points and lines — design

Part of the mesh-gaps set ([overview](2026-11-04-mesh-gaps-overview-design.md)). Size: M.
Changes the scene model, after item 4 (material layers) and item 5 (lights and
cameras): all three add to `scene.rs`, `Capabilities` and `LossKind`.

## Why

Every reader drops point clouds and wireframes with a warning:

| Reader | Today |
|---|---|
| glTF | modes 0–3: "has mode {n} (points or lines); it was skipped" (`glb.rs`) |
| OBJ | `l` and `p` are counted and warned (`obj.rs`) |
| PLY | `edge` elements are counted and skipped. A file with no faces is refused as `Empty` |
| Collada | `<lines>` and `<linestrips>` are skipped (`dae.rs`) |
| USD | `Points`, `BasisCurves` and curves are warned (`usd_read.rs`) |
| FBX | line geometry is warned (`fbx_read.rs` module doc) |

The scene holds triangles only (`Primitive.indices`: "Triangle list"). So a scan's point
cloud can't be converted at all (`MeshError::Empty`), and a model's wireframe overlay,
measurement lines or hair guides vanish.

## Goals

1. The scene holds point and line primitives beside triangles, with positions,
   normals, vertex colours, UVs, materials, skins and morph targets, the same
   attributes a triangle primitive has. Point and line primitives are glTF's modes 0
   and 1.
2. Every container that can hold them reads and writes them. Every container that
   can't reports `points` or `lines` as a loss, by name.
3. A file of points alone is a valid input. It converts to every container that holds
   points, and to the others it gives a clear refusal.

## Non-goals

- Curves beyond polylines: cubic `BasisCurves`, NURBS, OBJ free-form. The warning
  stays, naming them.
- Point size and line width as data. glTF has neither, and every rasteriser draws a
  pixel. USD's `widths` gets a fixed authored value (below).
- Gaussian-splat PLY attributes (`f_dc_*`, `opacity`, `scale_*`, `rot_*`). A splat PLY
  reads as points with positions, plus colours where it has `red`/`green`/`blue`. The
  splat-specific properties are named in one warning.
- 3MF's beam lattice extension.

## Decisions (mine)

### The scene

- `Primitive` gains `topology: Topology`, with `enum Topology { #[default] Triangles,
  Lines, Points }`. This is glTF's own shape (`primitive.mode`), and it keeps one
  attribute and material model for all three.
- `indices` keeps its meaning per topology: a triangle list, a line list (pairs), or a
  point list. Strips, loops and fans are expanded on read, as for triangles. Points
  without indices get `0..n`.
- `validate`:
  - index count is a multiple of 3, 2 or 1 by topology;
  - every index is in range;
  - the scene caps count every topology's indices.
- `Scene::triangle_count` counts triangles only. The new `Scene::drawable_count` counts
  triangles + lines + points. `read_scene`'s `Empty` check becomes "nothing drawable".
- **The posing path keeps every topology** (revised after the builder check).
  `pose::world_primitives`, `bake` and `degrade` are how OBJ and PLY receive world-space
  geometry (`pose.rs`, `degrade` → `bake` → `world_primitives`), and `merge_all` is how
  PLY merges. So they pose and carry lines and points exactly as they do triangles.
  `merge_all` merges primitives of one topology only, giving at most one merged
  primitive per topology.
- **Triangle-only, each saying so in its doc** with a test that a line or point
  primitive beside triangles leaves its result unchanged:
  - `pose::flatten` and the flat `ir::Mesh` (the STL path and the flat API);
  - the area, volume and centroid metrics;
  - `inspect`'s triangle counts.
- **Bounds count every topology.** A new `pose::world_bounds(scene, pose)` takes every
  primitive's posed positions. `normalize::place` (ground, centre, `height_m`) uses it,
  so a point cloud is placed and scaled like a mesh instead of silently skipped, and
  USD's `widths` rule uses it. `inspect`'s manifest bounds use it too, so a point
  cloud's delivered asset has bounds.
- `Capabilities` gains `points: bool` and `lines: bool`. `losses` reports
  `LossKind::Points` ("points", "{n} point primitive(s) …") and `LossKind::Lines`
  ("lines", …). These are new wire strings, additive.
- `MeshFormat::prepare` removes the topologies a target can't hold before its writer
  sees the scene. A debug assertion in each such writer pins that it never receives
  one.
- Writing a scene that has nothing left for the target (points to STL) is
  `MeshError::Unsupported("stl holds triangles only; this scene has 9 points and no
  triangles")`. That's 422 `mesh_unsupported` at the converter: named, not a bare
  `mesh_empty`.

### Per container

| Container | Read | Write |
|---|---|---|
| glTF | modes 0 POINTS, 1 LINES, 2 LINE_LOOP and 3 LINE_STRIP (loop and strip expanded to lines) | modes 0 and 1 |
| OBJ | `p v…` → points, `l v…` → a polyline's segments, colours from `v x y z r g b` | world space as for faces. Lines are written as **one `l a b` per segment, in their own `o` object with their own `usemtl`**, because three.js's `OBJLoader` needs exactly that (research Q6). Points are `p` in their own object the same way |
| PLY | a file with no `face` element → one points primitive of every vertex. `edge` (`vertex1`, `vertex2`, optional `red`/`green`/`blue`) → lines | `Capabilities.points` and `lines` are true, and the content rule lives where 3MF's does: `ply::prepare` keeps points only when the scene has no triangles or lines. PLY has one vertex pool, and points beside faces would read back as nothing. `ply::losses(scene)` reports the stripped points, the way `threemf::losses` reports 3MF's content rules. Lines are written as an `edge` element |
| USD | `Points` (`points`, `normals`, `displayColor`) → points. `BasisCurves` with `type = "linear"` (`curveVertexCounts`, `wrap` periodic or nonperiodic) → lines. Cubic curves stay warned | `Points` and `BasisCurves type = "linear"`, both with `widths` authored as one constant: 0.25 % of the scene's rest-bounds diagonal (a choice that keeps Storm from drawing 1 m points) |
| Collada | `<lines>` and `<linestrips>` → lines | `<lines>`. Collada 1.4.1 has no point primitive, so `points` is reported |
| FBX | line geometry (`Geometry` class `Line`, `Points` + `PointsIndex`, where a negative index ends a strip) → lines | not written: `lines` and `points` are reported (no portable FBX point type, and line geometry is rarely imported) |
| 3MF, STL | — | `points` and `lines` reported |

### Hostile input

Point and line indices are charged to the same index budget as triangles before they're
allocated (`glb.rs` `Budget.indices`, `obj.rs`/`ply.rs` index caps). A 4M-point PLY is
within the scene caps. A loop or strip expansion is charged at its expanded size.

## What proves it is done

### Unit tests, RED first

- **scene.rs:**
  - `a_lines_primitive_needs_pairs` and `points_take_any_count`;
  - `a_scene_of_points_is_not_empty`;
  - `metrics_ignore_points_and_lines`: area, volume and centroid of `wire()` equal
    those of its triangles alone.
- **pose.rs and normalize.rs:**
  - `world_primitives_pose_lines_and_points`: an animated parent moves the line loop's
    and the points' world positions at a sample time exactly as it moves a triangle's;
  - `merge_all_keeps_topologies_apart`: one merged primitive per topology;
  - `world_bounds_count_points_and_lines`: points outside the cube widen the bounds;
  - `a_point_cloud_is_placed`: `place(ground, center, height_m: 2.0)` on a points-only
    scene moves and scales it, and gives no "no height" note.
- **Per reader:**
  - `glb_reads_points_lines_loops_and_strips`: a hand-built glTF with modes 0–3. The
    loop with n vertices gives n segments, the strip n − 1.
  - `obj_reads_p_and_l`: a polyline of 4 vertices gives 3 segments, with colours.
  - `ply_without_faces_is_a_point_cloud` and `ply_edges_are_lines`.
  - `usda_points_and_linear_curves_read`: a periodic 4-vertex curve gives 4 segments,
    and a cubic curve warns.
  - `dae_lines_and_linestrips_read`.
  - `fbx_line_geometry_reads`: a negative `PointsIndex` ends a strip.
- **Per writer:** `glb`, `obj`, `ply`, `usd` and `dae` each write `wire()` and read it
  back. Same topology counts, positions bit-exact (text writers use shortest-repr),
  same colours.
- `writers_never_see_a_topology_they_cannot_hold`: `prepare` for stl, 3mf and fbx on
  `wire()` gives no non-triangle primitives, and the debug assertion is tested with
  `#[should_panic]` in a debug build.
- `points_to_stl_is_named`: a points-only scene to STL gives `Unsupported`, with the
  message above.
- Hostile input: the truncation and bit-flip sweeps on the new glTF, OBJ, PLY, USD and
  Collada paths. Also `a_line_strip_of_a_billion_vertices_is_charged_before_expanding`:
  a glTF LINE_STRIP accessor whose count passes the index budget is refused before
  allocation.

### Cross-format matrix

`fixtures::wire()`:

- a unit cube (triangles);
- a square line loop with vertex colours, under an animated node;
- a 3 × 3 grid of coloured points.

`scene_matrix_tests` compares, for every pair, at rest and at every sample time:

- the point and line counts where both containers hold them;
- their world-space bounds and centroids;
- that the triangles are unchanged;
- the loss list (exactly `points` and/or `lines` where a capability says so).

### Render harness

- Points draw as `THREE.Points` (4 px, `sizeAttenuation: false`) and lines as
  `THREE.LineSegments` (1 px), in the geom and albedo modes. The expected picture is
  GLTFLoader's.
- Each format's loader is used where it builds point or line objects. Where a loader
  can't, that comparison is skipped with the source line cited, and another oracle
  judges. From the research (Q6):
  - glTF points and lines survive in every reader that takes glTF;
  - three.js and RealityKit drop USD `Points` and `BasisCurves`, and SceneKit drops
    `Points`. Storm renders both, contradicting Apple's feature table, so **usdrecord
    is USD's judge**;
  - Collada `<lines>` (what litegen writes) is drawn by three.js and SceneKit.
    `<linestrips>`, read but never written, keeps one strip in three.js and draws
    nothing in SceneKit;
  - three.js's `FBXLoader` reads only NurbsCurve and assimp only Line geometry, which
    is why FBX writes neither.
- usdrecord can't be compared pixel for pixel with screen-space points, because Storm
  draws world-space `widths`. It's compared on the coverage's bounding box and
  centroid, within 1 % of the frame.
- Negative control: write OBJ `l` indices off by one. The OBJ albedo cases and
  `obj_reads_p_and_l`'s round trip fail.

### Third-party files

`--import` runs:

- a glTF-Sample-Assets model with points or lines, if one exists (the research note
  names candidates);
- a public point-cloud PLY (local only): Stanford's `drill_1.6mm_0_cyb.ply`, from
  `http://graphics.stanford.edu/pub/3Dscanrep/drill.tar.gz` (sha256 prefix
  `c4277dc7dddc63b8`). It's big-endian, with 4,223 vertices, a `range_grid` list element
  and no faces. Today it reads to `MeshError::Empty` (item 9). It must read as 4,223
  points;
- a USD file with `Points` written by `usdcat` from our USDA (committed: our content).

The native check compares our reading with the original's own loader.

### Docs

- `3d-format-conversion.md` §1's table and every codec section;
- `mesh/mod.rs`'s lossiness table (new columns of what each keeps);
- the converter's OpenAPI text for `X-Litegen-Conversion-Losses` (the two new kinds);
- the SDK doc comments that list the loss kinds (TS `client.ts`, Python `types.py`),
  through the frontend/SDK session;
- the matrix page (via litegen-a3).

### Gate

The overview's full gate, the harness fixture set with `wire`, and the `--import` set
above.

## Left to the builder

- Whether line strips are merged into one lines primitive per material or kept apart.
  Either is valid. Keep the count stable for the matrix.
- The exact USD `widths` rule, within the stated bound.
