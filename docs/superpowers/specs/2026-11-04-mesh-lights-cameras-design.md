# Lights and cameras — design

Part of the mesh-gaps set ([overview](2026-11-04-mesh-gaps-overview-design.md)). Size: M.
Changes the scene model, after item 4 (material layers) and before item 10 (points and
lines).

## Why

The scene has neither lights nor cameras, so every reader drops them:

| Reader | Today |
|---|---|
| glTF | `KHR_lights_punctual` warns "its lights … is dropped". Core `cameras` are dropped **with no warning**: `glb.rs` has no camera handling, which breaks the module's "never silent" rule. |
| FBX | "{n} camera(s) and {m} light(s) were read as empty nodes; the scene has neither" (`fbx_read.rs`) |
| Collada | "{n} camera(s) and {m} light(s) are not carried" (`dae.rs`) |
| USD | "Camera / DistantLight / SphereLight / … is not carried" (`usd_read.rs`) |

glTF, FBX, Collada and USD all hold both. A product shot's camera, or a scene's key
light, is part of what the author made, and an engine import (Unity, Unreal, Blender,
Godot) uses them.

## Goals

1. The scene holds glTF's punctual lights (directional, point, spot) and cameras
   (perspective, orthographic), attached to nodes, so transforms and animation of the
   nodes apply.
2. glTF, FBX, Collada and USD read and write them. OBJ, PLY, STL and 3MF report
   `lights` and `cameras` as losses.
3. Every unit and axis conversion is pinned by a test against an independent reader:
   three.js's loader for the format, ufbx's or Blender's documented conventions for FBX,
   and Pixar's `UsdGeomCamera` and UsdLux docs for USD.

## Non-goals

- Area, dome, environment, IES and other non-punctual lights. Each is warned by name,
  as now (USD `RectLight`, `DomeLight`; FBX area and volume light types).
- Animated light or camera *properties* (`KHR_animation_pointer`, FBX curves on
  `Intensity` or `FieldOfView`). Node transforms animate as today, and property curves
  stay warned.
- Ambient light: Collada `<ambient>`, FBX `AmbientColor`. Warned.
- Shadows, light linking, depth of field, physical camera exposure.

## Decisions (mine)

### The scene

```rust
pub struct Light { pub name: Option<String>, pub kind: LightKind, pub color: [f32; 3] /* linear */, pub intensity: f32, pub range: Option<f32> }
pub enum LightKind { Directional, Point, Spot { inner_cone: f32, outer_cone: f32 } } // half-angles, radians
pub struct Camera { pub name: Option<String>, pub projection: Projection }
pub enum Projection {
    Perspective { yfov: f32 /* radians */, aspect: Option<f32>, znear: f32, zfar: Option<f32> },
    Orthographic { xmag: f32, ymag: f32, znear: f32, zfar: f32 },
}
// Scene { lights: Vec<Light>, cameras: Vec<Camera>, .. }   Node { light: Option<usize>, camera: Option<usize>, .. }
```

- The values are glTF's, units included:
  - intensity is in candela (point, spot) or lux (directional);
  - lights shine along the node's local −Z, and cameras look along −Z with +Y up.
- Each codec converts at its own boundary, as for everything else.
- `validate` (the writer-side invariant):
  - indices in range, and finite values;
  - `0 ≤ inner_cone < outer_cone ≤ π/2`;
  - `yfov` in (0, π);
  - perspective `znear > 0`, orthographic `znear ≥ 0` (glTF allows 0 there), and
    `zfar > znear` when present;
  - `xmag` and `ymag` nonzero.
- **Readers never refuse a file over a light or camera** (revised after the builder
  check). A reader drops a light or camera that would fail `validate`, with a warning
  naming it and the bound it breaks, as `glb.rs` drops a bad material. The node stays.
- `Capabilities` gains `lights: bool` and `cameras: bool`, and `LossKind` gains `Lights`
  ("lights") and `Cameras` ("cameras"). These are new wire strings, additive.
- `pose::degrade` and the static containers (OBJ, PLY, STL, 3MF) drop them and report
  them. A node that held only a light or camera is pruned as an empty node would be.

### Per container

| Container | Lights | Cameras |
|---|---|---|
| glTF | `KHR_lights_punctual`, read and written. It goes in `extensionsUsed` only, never `extensionsRequired` | core `cameras` and `node.camera` |
| FBX | `NodeAttribute` class `Light`: `LightType` 0 point, 1 directional, 2 spot; `Color`; `Intensity` = 100 × the scene's (three.js `FBXLoader` divides by 100); `InnerAngle` and `OuterAngle` in degrees, full cones. FBX lights point along −Y, so a fixed post-rotation is applied at the boundary (Blender's exporter: `MAT_CONVERT_LIGHT`, a rotation of π/2 about X) | `NodeAttribute` class `Camera`: `CameraProjectionType`, `FieldOfView` + `ApertureMode`, `FilmWidth`/`FilmHeight` (inches) and `AspectWidth`/`AspectHeight`, `NearPlane`/`FarPlane`, `OrthoZoom`. FBX cameras look along +X, so the boundary applies Blender's `MAT_CONVERT_CAMERA`, a rotation of π/2 about Y |
| Collada | `<light>` `<directional>`, `<point>` and `<spot>`. `falloff_angle` is in degrees, with full cone or half-angle as the COLLADA 1.4.1 spec's `<spot>` entry defines it: the builder quotes the page in `dae.rs`. three.js's `ColladaLoader` takes it as the half-angle (`ColladaComposer.js`, `light.angle = degToRad(falloffAngle)`); if the spec says otherwise, that's a cited deviation. It maps to `outer_cone`, and `falloff_exponent` → `inner_cone` by the builder's documented rule. Collada has no intensity, so `<color>` = colour × intensity, and `<extra><technique profile="litegen">` records the exact colour and intensity for an exact round trip | `<camera>` `<perspective>` (`yfov` or `xfov` + `aspect_ratio`, degrees) and `<orthographic>` (`xmag`, `ymag`) |
| USD | `DistantLight`, and `SphereLight` with `treatAsPoint = true`, plus `ShapingAPI` for a spot (`shaping:cone:angle` in degrees, the half-angle; `shaping:cone:softness` from the inner cone). `inputs:intensity`, with the mapping from photometric units documented and cited. The exact glTF values go in `customData.litegen` for an exact round trip | `UsdGeomCamera`: `projection`, `focalLength`, `horizontalAperture`, `verticalAperture`, `clippingRange`. `yfov = 2·atan(verticalAperture / (2·focalLength))`; orthographic apertures per UsdGeomCamera's units |
| OBJ, PLY, STL, 3MF | reported | reported |

**Who decides each mapping** (revised after the builder check). three.js can't be the
authority here. In r183, `FBXLoader` ignores FBX's +X camera axis (so a correct file
renders 90° off), builds no orthographic camera, divides near and far by 1000, uses
decay 1, and reads `InnerAngle` as the half-angle. `ColladaLoader` reads
`falloff_angle` as the half-angle. Neither aims a light along its node's axis: Collada
sets no target, and FBX sets one only from `LookAtProperty`, so their lights point at
the origin. So:

- **FBX:** the authority is the Autodesk SDK's conventions as ufbx and Blender's
  importer and exporter implement them. That's the axes above, `InnerAngle`/`OuterAngle`
  as full cones in degrees (Blender's exporter writes `spot_size`, a full angle), and
  `Intensity` in percent. The oracles are ufbx's documented reading and assimp's
  conversion of our FBX to glTF (`assimp export x.fbx x.gltf`), compared numerically.
- **Collada:** the authority is the COLLADA 1.4.1 spec text. The oracles are SceneKit
  (renders through the file's camera and lights) and assimp's glTF conversion.
- **USD:** the authority is the `UsdGeomCamera` and UsdLux schema docs. The oracles
  are usdrecord (`--camera`, and a lit render) and SceneKit. RealityKit and three.js's
  `USDLoader` ignore USD cameras and lights (research Q8). The research couldn't settle
  two things the builder pins from the schemas and records: the SphereLight intensity
  conversion, and whether the FBX and COLLADA spot angles are half or full cones.
- **three.js** `FBXLoader` and `ColladaLoader`: their deviations are listed in each
  codec's module doc with source lines, and the harness records their camera and light
  comparisons as cited skips. `GLTFLoader` stays the authority for glTF.

### The glTF camera warning ships first

Until the rest lands, `glb.rs` warns "the file has {n} camera(s), which litegen does not
carry". That restores the never-silent rule on its own. It's Task 1 of the plan.

## What proves it is done

### Unit tests, RED first

- `glb_names_cameras_it_drops` (Task 1, before the scene types exist).
- **scene.rs:** `light_and_camera_values_are_validated` (each bound above);
  `losses_name_lights_and_cameras`.
- **Round trips** of `fixtures::staged()` through glTF, FBX, Collada and USD. The
  fixture holds a directional light, a point light with range, a spot (inner 0.2,
  outer 0.6 rad), a perspective camera (yfov 0.8, aspect 1.5) and an orthographic one,
  on nodes under an animated parent. After the round trip, at rest and at every
  sample time:
  - every light's world position and direction (−Z of its node), colour, intensity
    and cones equal the fixture's within 1e-5;
  - every camera's world eye, forward and up and its projection values equal within
    1e-5.

  The exact-round-trip annotations (Collada extra, USD customData) are asserted
  present. A second variant strips them, then asserts the documented approximate
  mapping, with intensity within 1e-3 relative.
- **Axis tests:**
  - `fbx_cameras_look_along_plus_x_on_the_wire`: the raw FBX camera node's rotation,
    before the boundary correction, maps the scene's −Z forward to FBX's +X;
  - `fbx_lights_point_along_minus_y_on_the_wire`;
  - the same for USD and Collada, which share glTF's −Z, so their corrections are the
    identity.
- **Third-party readers in unit tests:**
  - `testdata/` gains an FBX with a camera and lights, written by assimp or ufbx-write
    from our staged GLB (our content, committable). A test reads it and checks the
    same values;
  - a Collada file written by SceneKit or assimp, the same way.

### Render harness

- **New `camera` mode:** render the geom mode through the file's own camera, compared
  with the expected picture from GLTFLoader's camera, at rest and at sample times.
  This catches a wrong axis, FOV, aspect or near plane as a 0.00 %-versus-large
  difference. Each format's judge:
  - glTF: three.js `GLTFLoader`;
  - Collada and USD: SceneKit's render through the file's camera, and usdrecord
    `--camera <path>` for USD;
  - FBX: assimp's glTF conversion of the file, rendered by `GLTFLoader` through its
    camera.

  three.js `FBXLoader` and `ColladaLoader` cases are cited skips.
- **`lit` mode** (from item 4), with only the file's own lights and no environment:
  - glTF: three.js;
  - Collada and USD: SceneKit, and usdrecord for USD;
  - FBX: assimp's glTF conversion.

  Light direction, cones and colour are compared there and numerically (next bullet).
- **Numeric oracle:** a script reads assimp's glTF conversion of every FBX and Collada
  output. It compares each camera's eye, forward, up, `yfov`, aspect, near and far, and
  each light's kind, colour, intensity, direction and cones, with the scene's, within
  1e-4. Where assimp's convention is documented to differ, the citation is recorded and
  the comparison adjusted.
- **Negative controls:**
  - drop the FBX camera post-rotation: the numeric oracle's forward check fails by 90°,
    and `fbx_cameras_look_along_plus_x_on_the_wire` fails;
  - invert the Collada `falloff_angle` rule (full ↔ half): the spec-pinned unit test
    and the numeric oracle's cone check fail.

  Restore each by rewriting.

### Third-party files

`--import` (local only):

- Khronos glTF-Sample-Assets `Cameras` and `LightsPunctualLamp`, plus another
  lights sample if the research note names one;
- Blender-exported FBX and USD of a lit, camera'd scene, if available.

The native check runs in `camera` and `lit` mode.

### Docs

- `3d-format-conversion.md`: §1's scene list, and each codec's section;
- `mesh/mod.rs`'s table, with lights and cameras columns;
- the converter's losses header text, and the SDK doc comments (via the frontend/SDK
  session);
- the matrix page's "Lights and cameras" row (via litegen-a3).

### Gate

The overview's full gate, the harness with `staged` in all modes including `camera` and
`lit`, and the `--import` set.

## Left to the builder

- Collada `falloff_exponent` ↔ inner cone: one documented rule, tested both ways.
- The USD intensity mapping. Pick it from the research note's evidence, cite it in
  `usd_write.rs`, and test it.
