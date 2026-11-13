//! COLLADA (`.dae`) 1.4.1 — read and write.
//!
//! # Who reads what this writes
//!
//! Collada is the interchange format of the pre-glTF tool chain: 3ds Max,
//! Maya, SketchUp, Unity, Godot, SceneKit, assimp and three.js read it (Blender
//! did until 5.0 removed it). The schema can carry nearly the whole scene —
//! node hierarchy, Phong materials with textures, skins, morph targets and
//! keyframed transforms — but every reader honours a different subset, so the
//! writer is shaped by what the readers people actually use do with each
//! construct. The oracles are three.js r183's `ColladaLoader` (the most widely
//! used open reader, and the engine of the dashboard's viewer), Apple's
//! SceneKit importer (the stack behind Quick Look) and assimp 6.0; each
//! decision below names the evidence, and every document this writer
//! produces validates against the 1.4.1 XSD.
//!
//! # Decisions, and why
//!
//! - **Morph targets are baked unless asked for.** three.js fails the WHOLE load
//!   on any `<morph>` controller: `ColladaParser.parseController` stores a
//!   controller with no `skin` for it ("Morph target animation not supported
//!   yet."), and `ColladaComposer.buildNode` then reads `controller.skin.joints`
//!   ("Cannot read properties of undefined (reading 'joints')"); a skin over a
//!   morph fails in `buildController` reading `sources`. A file with a weight
//!   clip fails earlier than either, in `ColladaParser.parseAnimationChannel`
//!   (`ColladaParser.js:347-354`, r183): the channel's target `…-weights(i)` has
//!   no `/`, so `sid` is undefined and `sid.indexOf` throws, and the `joints`
//!   and `sources` failures are those of a static morph and of a static skin
//!   over one. assimp 6.0 cannot import one either: the `<targets>` loop in
//!   `ColladaParser::ReadController` advances the wrong node, the target id
//!   stays empty, and the import fails with "Unable to resolve library
//!   reference ''". A file with morphs would
//!   open in neither, so by default ([`DaeMorphs::Bake`]) [`prepare`] applies
//!   every target at its node's default weights, drops the weight animation,
//!   and the loss is reported. Morph controllers ARE read (NORMALIZED and
//!   RELATIVE): DCC files carry them.
//!
//!   [`DaeMorphs::Keep`] writes them, for a caller who knows its reader plays
//!   them: SceneKit and Xcode's scene editor, and Godot (from its source).
//!   The form is the one SceneKit was measured playing (macOS 26.6.2,
//!   2026-11-09: at rest and at six times of a weight clip, each picture 0.00 %
//!   from a control with the morph applied by hand; the tables and conclusions
//!   are in `docs/superpowers/research/research-scenekit-collada-morphs.md`,
//!   and what each reader does with the file in
//!   `docs/superpowers/3d-format-conversion.md` §7):
//!   - a `<controller>` per morphed primitive and set of node weights, over
//!     that primitive's `<geometry>`: `<morph method="NORMALIZED">`, an
//!     `IDREF_array` of target geometries, and a `float_array` of the default
//!     weights, which SceneKit honours at rest. NORMALIZED with absolute
//!     targets is the reading every implementation agrees on, `base·(1−Σw) +
//!     Σ wᵢ·targetᵢ`, and SceneKit does not clamp it (sums to 1.48 drew
//!     exactly), so the scene's additive offsets are written as base + offset;
//!   - each target a full `<geometry>`, the target's absolute positions and
//!     normals (base normal + normal offset, the base's own for a target with
//!     none), on the base's indices and material;
//!   - `POSITION` and `NORMAL` both inside `<vertices>`, in a morphed base and
//!     in its targets. The reader takes normal offsets only from there, and
//!     SceneKit morphs the normals (lit pictures differ from a base-normals
//!     control by up to 21,442 pixels). Unmorphed geometries keep `NORMAL` on
//!     the primitive element, so `Bake`'s bytes are unchanged;
//!   - a skin over a morph is `<skin source="#…-morph">`, and SceneKit
//!     deforms it as skin(morph(base)) at every sample time;
//!   - the weight animation is one `<animation>` of a sampler and a channel per
//!     controller and weight, keyed like a matrix channel (every source key,
//!     30 fps, LINEAR, a key just before each STEP key) and at 0, which
//!     overrides the default weights from the start as in glTF. The channel's
//!     target is `<controller>-weights(i)`, the MORPH_WEIGHT `<source>`'s id:
//!     SceneKit plays nothing for one that names the `<float_array>` inside it,
//!     though the reader takes both (Ruling DM9). Its three arrays of the time
//!     grid each are why `write_scene`'s resample budget charges
//!     `primitives × targets × 3` values per time for a weight channel under
//!     `Keep` (Ruling DM13);
//!   - a node draws its first controller itself and the rest from identity
//!     child nodes, as skinned primitives are (SceneKit keeps one
//!     `<instance_controller>` per node), and nodes whose weights differ get
//!     controllers of their own, since `<instance_controller>` has no weight
//!     override. Every controller shares the base and target geometries, but
//!     lists every target of its mesh: so each (set, primitive, target) is
//!     charged as a vertex, with the meshes' own, against the scene's vertex
//!     ceiling, as is each vertex of every skin controller over a morph (one
//!     for each set and skinned primitive) and 17 for each of its joints (one
//!     name, a 4x4 bind pose), and a scene past it is refused before anything
//!     is written (Rulings DM17, DM22, DM23).
//!
//!   Not established in SceneKit: STEP weight
//!   keys, a morph under a transformed node, a skin with non-identity
//!   bind-shape or inverse-bind matrices over a morph, several nodes drawing
//!   one controller (the spike measured one node a controller; the writer
//!   shares a controller between nodes at equal weights that no clip
//!   animates), all of which the round trip through this reader covers, and
//!   the weights outside 0 to 1 singly. A mesh of several primitives is
//!   established: the harness judged Khronos's MorphPrimitivesTest, written
//!   with `Keep`, in SceneKit at rest (item 8's gate, 2026-11-09). Read
//!   back, a morphed mesh of several primitives is a mesh and a node for each
//!   primitive, with the same target names and default weights, and nodes that
//!   differed only in weights read as meshes of their own.
//!
//! - **One `<geometry>` per primitive.** three.js merges every primitive of a
//!   `<mesh>` into one `BufferGeometry` and concatenates their attributes
//!   (`buildGeometryType`); a primitive without colours or normals beside one
//!   with them shifts every later attribute out of line with its position. A
//!   geometry per primitive, instanced together on the node, cannot misalign.
//!   SceneKit, though, keeps only one `<instance_controller>` per node (a
//!   two-controller node loads with one skinned geometry), so a skinned
//!   mesh's later primitives are drawn from identity child nodes — harmless,
//!   since skinning ignores the holding node's transform. The reader merges a
//!   node's instances back into one mesh, and controllers that bind the same
//!   joints the same way into one skin.
//!
//! - **Lines are written as `<lines>`, never `<linestrips>`, and points are
//!   not written.** A lines primitive is a `<lines>` element with a pair of
//!   corners a segment (COLLADA 1.4.1 spec, pp. 5-72, 5-74, 5-80), over the
//!   same vertices, so a loop's shared vertices stay shared. three.js's
//!   `parseGeometryPrimitive` overwrites `primitive.p` at every `<p>`, so only
//!   the last strip of a `<linestrips>` survives, and SceneKit builds a
//!   geometry with no elements from one (`docs/superpowers/research/
//!   research-consumers.md`, Q6); assimp reads both. The spec has no point
//!   primitive (a `<mesh>` holds `<lines>`, `<linestrips>`, `<polygons>`,
//!   `<polylist>`, `<triangles>`, `<trifans>` and `<tristrips>`), so
//!   [`prepare`] drops a points primitive and `scene::losses` names it; a
//!   writer given one anyway was not prepared, and a debug build panics
//!   (`pose::debug_assert_holds`).
//!
//! - **Vertex colours are linear, and written inside `<vertices>`.** The spec
//!   gives colour no space; the readers decide. SceneKit takes a `COLOR` as
//!   linear wherever it sits (measured: 0.5 renders as sRGB 188), as does
//!   assimp (which also writes glTF's linear colours as-is), and so does
//!   three.js for a `COLOR` inside `<vertices>`. Only
//!   three.js's primitive-level path decodes sRGB, through an off-by-one
//!   (`buildGeometryData`: `startIndex = array.length - sourceStride - 1`
//!   converts the previous vertex's alpha, then R and G, and never B). Written
//!   in `<vertices>`, the scene's linear colours reach all three exactly; read
//!   anywhere, a `COLOR` is linear.
//!
//! - **Material colours are sRGB-encoded.** three.js converts `<diffuse>`,
//!   `<emission>` and `<specular>` colours from sRGB to linear
//!   (`ColladaComposer.buildMaterial`: `ColorManagement.colorSpaceToWorking(
//!   material.color, SRGBColorSpace)`, and `ColorManagement.enabled` is true by
//!   default; measured: 0.5 loads as 0.214), as does SceneKit (0.5 renders as
//!   sRGB 127); assimp passes them through unconverted. Writing the linear
//!   value would show every flat-coloured part washed out in both, so colours
//!   are encoded on write and decoded on read, unclamped, with three.js's own
//!   curve.
//!
//! - **Matrices.** Collada matrices are column-vector math written ROW-major
//!   ("written in row-major order to aid the human reader", spec 1.4.1 §5),
//!   the transpose of the scene's column-major array. Every node is one
//!   `<matrix sid="transform">`; the reader also composes `translate`,
//!   `rotate`, `scale` and `lookat` stacks in document order.
//!
//! - **Animation is one baked matrix channel per animated node,** sampled with
//!   [`pose::sample_pose`] at every source key plus 30 fps, LINEAR. Collada has
//!   no quaternion element, and three.js decomposes matrix keys to TRS and
//!   interpolates those, turning STEP into linear on matrix channels
//!   (`buildMatrixTracks` never applies the sampler's interpolation) — so a
//!   STEP channel is emulated with an extra key just before each step. For
//!   the same decomposition, a scale that is exactly zero, in a key or on a
//!   node, is written as a millionth (`math::nonsingular_scale`): three.js
//!   turns a singular matrix into unit scale, so a hidden part would show
//!   full size.
//!
//! - **Skins** are written the way three.js binds them: skinned meshes hoisted
//!   to identity roots ([`glb::hoist_skinned_meshes`]), `bind_shape_matrix`
//!   identity, a joints `Name_array` of node `sid`s (three.js names a `JOINT`
//!   bone by its sid and matches by name), every joint `type="JOINT"`, and
//!   `<skeleton>` naming each root joint. On read the bind-shape matrix is
//!   folded into the inverse bind matrices, which is the scene's convention.
//!
//! - **Textures are sidecar files** (`textures/<name>.png`, relative
//!   `<init_from>`), PNG and JPEG only. A self-contained document carries no
//!   images: `data:` URIs in `<init_from>` are not something Collada readers
//!   resolve reliably, and a broken reference is worse than a reported loss.
//!
//! - **Up to eight uv sets,** `TEXCOORD` inputs `set="0"` … `set="7"`, each
//!   its own `<source>` in f64 and on the `<triangles>`, not in
//!   `<vertices>`, where three.js gives set 1 an item size of 0. assimp keeps
//!   eight, and SceneKit read ten
//!   (`docs/superpowers/research/research-consumers.md`, Q7). three.js
//!   r183's `ColladaLoader` reads two ([`UV_SETS`] cites the lines): a reader
//!   limit, not the file's. It also samples every texture from set 0, so
//!   [`prepare`] puts the sets a package's textures sample first.
//!
//! - **`<index_of_refraction>` is glTF's ior, unless it is an exporter's
//!   default.** Some exporters write it on every material:
//!   - SceneKit writes 1 (every effect of `testdata/dae/scenekit/`).
//!   - Blender 2.8-4.5 writes its Principled BSDF's IOR wherever a material
//!     has one (`source/blender/io/collada/EffectExporter.cpp:133-137`,
//!     called for every effect at :204, and `collada_utils.cpp:1236-1244`,
//!     v4.5.15). OpenCOLLADA then writes any value of 0 or more
//!     (`COLLADAStreamWriter/src/COLLADASWEffectProfile.cpp:157` and
//!     :375-380, v1.6.68). Blender's default is 1.45 in 2.8-3.6
//!     (`node_shader_bsdf_principled.cc:83`, v3.6.23) and 1.5 in 4.0-4.5
//!     (:52, v4.5.15). Blender 5 has no Collada.
//!   - assimp 6.0.5 writes one only when its source has one
//!     (`code/AssetLib/Collada/ColladaExporter.cpp:774` and :827), but its
//!     OBJ importer supplies 1 to any material whose MTL has no `Ni`
//!     (`code/AssetLib/Obj/ObjFileData.h:180`, `ObjFileImporter.cpp:641`).
//!
//!   OpenCOLLADA's Maya exporter writes it only when a shader's refractions
//!   are on (`COLLADAMaya/src/COLLADAMayaEffectExporter.cpp:539-547`), and
//!   its 3ds Max exporter only inside an `<extra>`
//!   (`COLLADAMax/src/COLLADAMaxEffectExporter.cpp:99`). Read as an ior,
//!   SceneKit's 1 would take every dielectric's reflection away in glTF.
//!   So 1, 1.45 and 1.5 read as unset (`DEFAULT_IORS`), and every other
//!   value of 1 or above is the ior. One of 0 or below is no refractive
//!   index, which is what the element holds, and is left out with a warning,
//!   as OBJ's `Ni 0` is: read as KHR_materials_ior's 0 it would be a perfect
//!   mirror, where the `Ni 0` of an OBJ assimp converted means no ior
//!   (Ruling A42).
//!   So is one KHR_materials_ior can't hold (between 0 and 1), one that is
//!   no number, or a `<param>` that names no float. Written, an ior of 0, 1
//!   or 1.45 is left out and named, since it would read back unset; 1.5,
//!   glTF's own default, is left out unnamed, as it draws the same.
//!
//! - **Lights and cameras follow the 1.4.1 spec's text**, which is the
//!   authority for Collada here; SceneKit and assimp are the oracles, and
//!   three.js's departures are listed below. The scene's values are glTF's,
//!   and each crosses this boundary as follows.
//!   - *Axes:* nothing to correct. A light shines along its node's −Z ("The
//!     light's default direction vector in local coordinates is [0,0,-1]",
//!     `<directional>` p. 5-30, `<spot>` p. 5-119). A camera looks along −Z
//!     with +Y up ("the lens looks towards the local -Z axis, and the top of
//!     the camera is aligned with the local +Y axis", `<camera>` p. 5-20).
//!     glTF's do the same. `<unit>` and `<up_axis>` reach them through the
//!     root correction, as they reach meshes.
//!   - *Colour and intensity:* Collada has no intensity, so `<color>` is the
//!     colour times the intensity (candela, or lux for a directional light),
//!     linear (Ruling R17). Unlike a material's colour, it is a linear scale
//!     that runs far past 1, where the sRGB curve would explode. assimp
//!     writes and reads it linear (`glTF2Importer.cpp` L975-L978,
//!     `ColladaExporter.cpp` L513-L579, `ColladaLoader.cpp` L347-L355,
//!     v6.0.5), and Blender writes its power times its colour
//!     (`LightExporter.cpp` L52, v4.5.0). A point or spot light's
//!     attenuation is written as 0, 0, 1. Then the spec's `A =
//!     constant_attenuation + ( Dist * linear_attenuation ) + (( Dist^2 ) *
//!     quadratic_attenuation )` (p. 5-119) is glTF's inverse square, which is
//!     how assimp writes glTF's lights too (`glTF2Importer.cpp` L980-L996,
//!     v6.0.5). glTF's `range` has no Collada form; only the extra below
//!     keeps it.
//!   - *Spot cones:* `<falloff_angle>` is the full cone, in degrees: twice
//!     the outer cone's half-angle (Ruling R23, which revises R4). The
//!     `<spot>` entry (pp. 5-119 to 5-120) gives it a default of 180, and
//!     says only that it and `<falloff_exponent>` "are used to specify the
//!     amount of attenuation based on the direction of the light". SceneKit,
//!     the Collada oracle of litegen's spec, reads it as the full cone
//!     (measured by Task 6a's harness), and Blender's Collada IO writes and
//!     reads it so (`LightExporter.cpp` L70, `DocumentImporter.cpp` L1030,
//!     v4.5.0). The default 180 is then a hemisphere, and a wider cone, past
//!     glTF's bound on a spot's outer cone, lights nearly every way, so the
//!     reader takes it as a point light, warned (Ruling R32). Two readers
//!     depart from this:
//!     - three.js reads it as the half-angle (below).
//!     - assimp reads it as its inner cone (`ColladaLoader.cpp` L359), so
//!       its inner cone is our full cone, twice our outer half-angle, and it
//!       guesses its outer cone from `<falloff_exponent>`, wider still
//!       (L365-L372: "I don't know how to interpret it, so I need to
//!       guess"): it reads a spot of inner 0.2 and outer 0.6 rad written
//!       here as 1.2 and 1.845 (measured by the harness's numeric oracle).
//!       It writes its inner cone there too (`ColladaExporter.cpp`
//!       L564-L567, v6.0.5).
//!
//!     `<falloff_exponent>` carries the inner cone by one rule,
//!     `falloff_exponent` and its inverse `inner_cone`. Two beams fall to
//!     half their axis' intensity at the same angle: the spec's, cosᵉ of the
//!     angle from the axis up to the cone's edge (the exponent is OpenGL's
//!     `GL_SPOT_EXPONENT`, which the spec's GLSL and GLES render states carry
//!     as `light_spot_exponent`, pp. 8-65 to 8-66), and glTF's, which falls
//!     from the inner to the outer cone as `((cos θ − cos outer) / (cos inner
//!     − cos outer))²` (the KHR_lights_punctual README's falloff). That angle
//!     is the beam angle by which lighting practice sizes a spot, and the
//!     rule has a closed form both ways. An exponent too weak to halve the
//!     beam inside the cone, the default 0 among them, is a hard edge, which
//!     the reader softens.
//!   - *Cameras:* `<perspective>` takes `<yfov>` in degrees, with
//!     `<aspect_ratio>` when the camera has one. `<yfov>` alone is legal,
//!     and leaves the aspect to the viewport, as glTF's unset one does.
//!     `<orthographic>` takes `<xmag>` and `<ymag>` as half the view's width
//!     and height, glTF's own. The spec says both "your COLLADA orthographic
//!     viewport becomes [[-xmag,xmag],[-ymag,ymag]]" and "This gives an
//!     orthographic width of xmag/2" (p. 5-90), which disagree. The first is
//!     the derivation (the magnifications are "simple scale factors" of
//!     OpenGL's [-1, 1] viewport), and it is what Blender's Collada IO writes
//!     and reads (`CameraExporter.cpp` L69, `DocumentImporter.cpp` L855 and
//!     L882, v4.5.0). `<zfar>` is required, so an infinite far plane is
//!     written as 1000 m (`fbx::unset_far`, as FBX writes it) and reported
//!     ([`extra_losses`]).
//!   - *The litegen extra:* each `<light>`, and each camera's `<optics>`,
//!     carries `<extra><technique profile="litegen">` with the scene's own
//!     values, named after glTF's: `gltf_name`, the name whole, which the
//!     `name` attribute, an NCName, cannot hold ("Key Light" is `Key_Light`
//!     there); `gltf_color`, `gltf_intensity`, `gltf_range`,
//!     `gltf_inner_cone_angle` and `gltf_outer_cone_angle`; `gltf_yfov`,
//!     `gltf_aspect_ratio`, `gltf_znear`, `gltf_zfar`, `gltf_xmag` and
//!     `gltf_ymag`. Lengths are in the document's `<unit>`, and an unset
//!     value is left out, a name among them. So a litegen round trip is
//!     exact, and the reader prefers the extra. A camera with no name is
//!     written with its id as its `name`, since assimp 6.0.5 reads no optics
//!     for a `<camera>` without one (`ColladaParser.cpp` L1049-L1051); its
//!     extra, with no `gltf_name`, keeps it unnamed. No other reader looks
//!     for those names: assimp gathers `<color>`, `<intensity>`,
//!     `<outer_cone>`, `<znear>` and their kin from anywhere under a
//!     `<light>` or `<camera>` (`ColladaParser.cpp` L327-L410, v6.0.5), and
//!     three.js reads the common technique alone.
//!
//! # What is read
//!
//! `<triangles>`, `<polylist>`, `<polygons>`, `<trifans>` and `<tristrips>`
//! (n-gons fanned when convex, ear-clipped when not), and `<lines>` and
//! `<linestrips>` as line segments (a strip of `c` corners is `c - 1`; a strip
//! of fewer than 2 is skipped and said once for its element; vertices no
//! segment names are not in the scene, as a triangle's are not), with any
//! input layout:
//! shared offsets, `<vertices>` indirection, accessor strides and offsets, up
//! to eight texture coordinate sets. Node transforms in document order,
//! `instance_node` and `library_nodes`, skin and morph controllers, the
//! `profile_COMMON` shaders (diffuse colour or texture through the
//! surface/sampler chain, emission, transparency in all four opaque modes,
//! `index_of_refraction` as the ior unless it is an exporter's default),
//! images through the caller's sidecars, animations of whole matrices, single
//! elements, `translate`/`rotate`/`scale` and their components with LINEAR,
//! STEP, BEZIER and HERMITE keys (resampled to TRS), morph-weight animation,
//! and `animation_clip`s. `<unit>` and `<up_axis>` become one correction on the
//! root nodes, so the scene comes out in metres, Y up.
//!
//! Lights and cameras (`<instance_light>`, `<instance_camera>`) are read by
//! the rules above. Without a litegen extra, they are read by the documented
//! approximate mapping:
//! - `<color>`, linear (Ruling R17), splits into the colour and the intensity
//!   by moving its largest component into the intensity, so the colour ends
//!   in 0 to 1 (Ruling R8).
//! - A point or spot light's attenuation folds into the intensity as it
//!   stands at 1 m, in the document's units. The scene's lights all fall off
//!   by the inverse square, so any other falloff is warned.
//! - No light has a range: no rule in the spec makes one.
//! - A spot's `<falloff_angle>` is its full cone (Ruling R23), so the
//!   default 180 is a hemisphere. A wider cone is read as a point light,
//!   warned (Ruling R32).
//! - Every field of view the spec allows is read (pp. 5-89, 5-92 to 5-93).
//!   The aspect ratio is the view's width over its height: tan(xfov/2) /
//!   tan(yfov/2) for a perspective view, as glTF's is, and the relation by
//!   which assimp and Blender turn `<aspect_ratio>` and `<yfov>` into a
//!   horizontal angle (`ColladaLoader.cpp` L430-L432, v6.0.5;
//!   `DocumentImporter.cpp` L868, v4.5.0). The spec's own "aspect_ratio =
//!   xfov / yfov" (p. 5-93) is that ratio's small-angle form. Given
//!   `<xfov>` and `<yfov>`, assimp takes tan(xfov) / tan(yfov) of the full
//!   angles instead (L426-L427), which also agrees only at small angles. A
//!   lone `<xfov>`, `<xmag>` or `<ymag>` leaves the other to the viewport,
//!   taken at 16:9, with a warning.
//!
//! A reader never refuses a file over a light or camera. One that does not
//! read or breaks a bound is dropped with a warning naming it, and its node
//! stays; a hard-edged spot is softened instead (`Light::soften_hard_edge`).
//! An instance whose url names no light or camera is warned, and its node
//! kept. A light or camera that no node instances is dropped, and named
//! (Ruling R13). A node keeps its first light and its first camera. An
//! `<ambient>` light is dropped, named with its colour (Ruling R15).
//!
//! three.js r183's `ColladaLoader` is not an authority for lights and
//! cameras, and departs from the rules above in these ways:
//! - It ignores `<xfov>`: a camera with no `<yfov>` gets `PerspectiveCamera`'s
//!   default of 50°, and one with no `<aspect_ratio>` its default of 1
//!   (`collada/ColladaComposer.js` L1410-L1417; `PerspectiveCamera.js` L33).
//! - It halves `<xmag>` and `<ymag>` (L1427-L1428), so an orthographic view
//!   shows half the width and height written.
//! - It aims no light along its node's axis (Ruling R18). `buildLight` sets no
//!   target (L1466-L1494), and a directional or spot light "points from its
//!   position to the target's position", a new `Object3D` at the origin
//!   (`src/lights/DirectionalLight.js` L58-L70, `SpotLight.js` L58-L69). So
//!   each one shines at the origin.
//! - It reads `<falloff_angle>` as the half-angle (L1492): a `SpotLight`'s
//!   `angle` is the "Maximum angle of light dispersion from its direction"
//!   (`src/lights/SpotLight.js` L79-L85). So it draws our spots, whose
//!   `<falloff_angle>` is the full cone (Ruling R23), twice as wide. It
//!   ignores `<falloff_exponent>`, so every spot is hard-edged.
//! - It takes √(1 / `<quadratic_attenuation>`) as a light's cutoff distance,
//!   and the constant and linear terms not at all (`collada/ColladaParser.js`
//!   L1185-L1188, set as `light.distance` at `ColladaComposer.js` L1491).
//!   Its renderer fades a light to nothing at that distance
//!   (`lights_pars_begin.glsl.js` L56-L70). So the inverse square written
//!   here, quadratic 1, cuts every point and spot light off 1 m away (Ruling
//!   R18 keeps the spec's 0, 0, 1).
//! - It decodes a light's `<color>` from sRGB (L1175-L1178), as it decodes a
//!   material's. This writer's is linear (Ruling R17), so three.js draws a
//!   component past 1 brighter (3 as 12.8) and one below 1 darker.
//!
//! # Untrusted input
//!
//! Every count and index in the file is checked against the data actually
//! present before anything is sized from it, arrays are charged to a parse
//! budget, node instancing is bounded and walked with explicit stacks (an
//! `instance_node` cycle is an error, not a hang), and nothing indexes with a
//! value the file chose without proving it in range first.
//!
//! Ruling IDs ("Ruling A22a") are mesh-gaps item 4's rulings, listed with
//! their costs in the status block of
//! `docs/superpowers/plans/2026-11-04-mesh-material-layers.md`. "Ruling R23"
//! and its kin are item 5's (lights and cameras), listed with their costs in
//! the "Rulings" section of
//! `docs/superpowers/plans/2026-11-04-mesh-lights-cameras.md`.
//!
//! @see <https://www.khronos.org/files/collada_spec_1_4.pdf>

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::{self, Write as _};
use std::rc::Rc;

use super::glb;
use super::ir::Mesh;
use super::math::{self, Mat4};
use super::package::{self, NoSidecars, Package, PackageFile, Sidecars};
use super::polygon::{triangulate, Triangulation, MAX_EAR_CLIP};
use super::pose;
use super::scene::{
    AlphaMode, AnimPath, Animation, Camera, Capabilities, Channel, Filter, Image, ImageFormat, Interpolation,
    LayerSupport, Light, LightKind, Loss, LossKind, Material, MaterialSupport, MorphTarget, Node, Primitive, Projection,
    Sampler, Scene, SceneMesh, Skin,
    SkinWeights, Texture, TextureRef, TextureSlot, Topology, Transform, Wrap, MAX_ANIMATION_KEYS, MAX_JOINTS,
    MAX_MORPH_TARGETS, MAX_NODES, MAX_SCENE_INDICES, MAX_SCENE_VERTICES,
};
use super::xml::Element;
use super::{DaeMorphs, MeshError, MeshFormat, SceneRead, WriteOptions};

// ─── constants ──────────────────────────────────────────────────────────────

/// The 1.4.x namespace, which is what this writer declares.
const NS_COLLADA_141: &str = "http://www.collada.org/2005/11/COLLADASchema";

/// How far into the bytes the root element must start. A real document opens
/// with an XML declaration and perhaps a comment banner; anything whose first
/// tag is further away than this is not a Collada file a tool wrote.
const SNIFF_BYTES: usize = 64 * 1024;

/// Texture coordinate sets the writer carries: eight, as many as the reader
/// keeps. Two of them reach three.js r183's `ColladaLoader`, a reader limit:
/// it reads set 0 as `uv` and set 1 as `uv1` and nothing beyond
/// (`parseGeometryPrimitive` keys a set `n > 0` as `TEXCOORD{n}` and
/// `buildGeometryType` handles only `TEXCOORD1`).
pub const UV_SETS: usize = 8;

/// Texture coordinate sets the reader keeps; past this a primitive's extra
/// sets are reported and dropped.
const MAX_READ_UV_SETS: usize = 8;

/// Joint influences kept per vertex (the glTF reader's ceiling too); the
/// strongest survive and the rest are reported.
const MAX_INFLUENCES: usize = 32;

/// Ceiling on numbers parsed from the document's arrays, for the same reason
/// as glb's `MAX_DECODED_VALUES`: every array is bounded by the input, but the
/// budget is what keeps the parse proportionate (each array is parsed once and
/// cached, so references cannot multiply it). 48M is ~192 MB of `f32`.
const MAX_PARSED_VALUES: usize = 48_000_000;

/// Node nesting depth, counting `instance_node` expansion, beyond which the
/// file is refused. Real hierarchies are a few dozen deep; the scene's own
/// algorithms are iterative, so this bounds work, not the stack.
const MAX_NODE_DEPTH: usize = 512;
/// Transform elements on one `<node>`. Exporters write a handful (Maya's
/// pivot stacks run to about a dozen); each animated sample recomposes the
/// whole stack, so without a ceiling one node's stack multiplies every sample.
const MAX_NODE_OPS: usize = 128;
/// Cut a clip's stretch, `start` to `end` seconds (`None`: its channels'
/// end), out of its channels and shift it to start at 0. Each channel keeps
/// its keys strictly inside the stretch plus a key sampled at each edge, so
/// it plays exactly that stretch; the reader's channels are LINEAR or STEP,
/// which sampling an edge does not change.
fn clip_window(anim: &mut Animation, start: f32, end: Option<f32>) {
    if !(start > 0.0) && end.is_none() {
        return;
    }
    let end = end.unwrap_or(f32::INFINITY);
    let last = anim.channels.iter().filter_map(|c| c.times.last().copied()).fold(start, f32::max);
    let end = end.min(last);
    for ch in anim.channels.iter_mut() {
        let stride = ch.values.len() / ch.times.len().max(1);
        let mut times = vec![0.0];
        let mut values = pose::sample_channel(ch, start, stride);
        // Strictly increasing after the shift, which can round two close
        // keys together in f32.
        for (k, &t) in ch.times.iter().enumerate() {
            if t > start && t < end && t - start > times[times.len() - 1] {
                times.push(t - start);
                values.extend_from_slice(&ch.values[k * stride..(k + 1) * stride]);
            }
        }
        if end - start > times[times.len() - 1] {
            times.push(end - start);
            values.extend(pose::sample_channel(ch, end, stride));
        }
        (ch.times, ch.values) = (times, values);
    }
}

/// Channel references summed over every `<animation_clip>`: a clip resamples
/// each channel it names, so clips sharing one big animation multiply it.
const MAX_CLIP_CHANNELS: usize = 2_000_000;

/// Largest input offset accepted on a primitive `<input>`. Offsets select
/// columns of the interleaved `<p>` list; real files use a handful.
const MAX_INPUT_OFFSET: usize = 64;


/// Sampling rate for animation the writer bakes and the reader resamples.
const RESAMPLE_FPS: f32 = 30.0;

/// Samples per resampled curve, whatever its duration: a key at t = 1e9 must
/// not become thirty billion samples.
const MAX_RESAMPLES: usize = 100_000;

/// How long before a STEP key the emulated jump starts, in seconds: well under
/// a frame at any rate a viewer plays, and representable next to any time of
/// a clip shorter than a day.
const STEP_EPSILON: f32 = 1e-4;

/// Phong exponent ceiling; past this the highlight is a point and the
/// roughness it stands for is effectively zero.
const MAX_SHININESS: f32 = 10_000.0;

/// Characters of file content an error message may quote.
const MAX_DETAIL: usize = 300;

const PRIMARY_FILE: &str = "model.dae";
const TEXTURE_DIR: &str = "textures";
const AUTHORING_TOOL: &str = "litegen mesh converter";
/// `<created>` and `<modified>` are required by the schema; a constant keeps
/// the output byte-identical run to run.
const TIMESTAMP: &str = "2000-01-01T00:00:00Z";

// ─── errors and small helpers ───────────────────────────────────────────────

fn not_dae(detail: impl Into<String>) -> MeshError {
    MeshError::NotThisFormat { expected: MeshFormat::Dae, detail: detail.into() }
}

fn malformed(detail: impl fmt::Display) -> MeshError {
    MeshError::Malformed(format!("collada: {}", clip(&detail.to_string(), MAX_DETAIL)))
}

fn unsupported(detail: impl fmt::Display) -> MeshError {
    MeshError::Unsupported(format!("collada: {}", clip(&detail.to_string(), MAX_DETAIL)))
}

/// At most `max` characters of `s`. Ids and names come from the file, and an
/// error message must never quote an unbounded amount of it.
fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// A file-supplied identifier, quoted and bounded for a message.
fn q(s: &str) -> String {
    format!("{:?}", clip(s, 60))
}

/// Children of `e` with this local name, borrowing from the document rather
/// than from the name.
fn kids<'a, 'n>(e: &'a Element, name: &'n str) -> impl Iterator<Item = &'a Element> + 'n
where
    'a: 'n,
{
    e.children.iter().filter(move |c| c.name == name)
}

fn kid<'a>(e: &'a Element, name: &str) -> Option<&'a Element> {
    e.children.iter().find(|c| c.name == name)
}

fn parse_count(v: &str, what: &str) -> Result<usize, MeshError> {
    v.trim().parse::<usize>().map_err(|_| malformed(format!("{what} is {}, which is not a count", q(v))))
}

fn attr_count(e: &Element, name: &str, what: &str) -> Result<Option<usize>, MeshError> {
    e.attr(name).map(|v| parse_count(v, &format!("{what} {name}"))).transpose()
}

fn mat_ptr(e: &Element) -> usize {
    e as *const Element as usize
}

/// sRGB transfer, unclamped, exactly the curve three.js decodes Collada colours
/// with (`Color.setRGB(..., SRGBColorSpace)`): components above 1 (HDR
/// emission) continue the power curve instead of being cut off.
fn srgb_encode(c: f32) -> f32 {
    let c = c as f64;
    (if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }) as f32
}

fn srgb_decode(c: f32) -> f32 {
    let c = c as f64;
    (if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }) as f32
}

/// Phong exponent for a roughness, the usual Blinn-Phong/GGX correspondence
/// `s = 2/α² − 2` with `α = roughness²`.
fn shininess_from_roughness(r: f32) -> f32 {
    let r = r.clamp(0.0, 1.0) as f64;
    let a2 = r * r * r * r;
    if a2 <= 2.0 / (MAX_SHININESS as f64 + 2.0) {
        return MAX_SHININESS;
    }
    ((2.0 / a2 - 2.0) as f32).clamp(0.0, MAX_SHININESS)
}

/// The inverse of [`shininess_from_roughness`].
fn roughness_from_shininess(s: f32) -> f32 {
    let s = s.clamp(0.0, MAX_SHININESS) as f64;
    (2.0 / (s + 2.0)).powf(0.25) as f32
}

/// Collada's row-major text of a column-major matrix.
fn row_major(m: &Mat4) -> [f64; 16] {
    math::transpose(m)
}

// ─── capabilities, prepare, losses ──────────────────────────────────────────

/// One row of the lossiness table: what a document this writer produces
/// carries (see `MeshFormat::capabilities`).
///
/// - `texture_filters` is false because no reader honours them: three.js's
///   `parseEffectSampler` reads only `<source>`, and assimp's sampler model
///   has no filter fields.
/// - `texture_wrap` is true: the spec's `wrap_s`/`wrap_t` are written, and so
///   is the MAYA `<extra>` on the `<texture>` (`wrapU`/`wrapV`/`mirrorU`/
///   `mirrorV`), which is what both three.js (`getTexture`) and assimp
///   (`ReadSamplerProperties`) actually read. three.js maps `wrapU` to repeat
///   or clamp and has no mirror: a reader limitation, not the file's.
/// - `texture_transforms` is false: profile_COMMON has none, so
///   `pose::degrade` bakes them into the UVs, which renders identically.
/// - `morph_targets` follows [`WriteOptions::dae_morphs`]: false under `Bake`,
///   which applies the targets in [`prepare`], and true under `Keep`.
/// - `lights` and `cameras` are true: `<library_lights>` and
///   `<library_cameras>`, each with a litegen `<extra>` that makes a litegen
///   round trip exact (module docs). What other readers cannot see of them
///   is in [`extra_losses`].
pub fn capabilities(opts: WriteOptions) -> Capabilities {
    Capabilities {
        hierarchy: true,
        normals: true,
        uv_sets: UV_SETS,
        vertex_colors: true,
        materials: MaterialSupport::Phong,
        textures: !opts.self_contained,
        texture_filters: false,
        texture_wrap: true,
        texture_transforms: false,
        skinning: true,
        morph_targets: opts.dae_morphs == DaeMorphs::Keep,
        animation: true,
        multiple_meshes: true,
        // `<index_of_refraction>`, less exporters' defaults (module docs).
        layers: LayerSupport { ior: true, ..LayerSupport::none() },
        lights: true,
        cameras: true,
        // `<lines>` (COLLADA 1.4.1 spec, pp. 5-72, 5-74, 5-80), never
        // `<linestrips>`: three.js keeps only the last `<p>` of one, and
        // SceneKit builds a geometry with no elements from it. The spec has no
        // point primitive, so `prepare` drops points and `scene::losses`
        // names them.
        points: false,
        lines: true,
    }
}

/// The texture slots a Collada package keeps, base colour (`<diffuse>`) and
/// emission (`<emission>`): `pose::degrade`'s Phong reduction drops
/// metallic-roughness and occlusion, and [`reduce_material`] drops normal
/// maps. [`prepare`] orders uv sets by these alone, once the others are gone.
const TEXTURED_SLOTS: &[TextureSlot] = &[TextureSlot::BaseColor, TextureSlot::Emissive];

/// The scene reduced to exactly what [`write_scene`] encodes, which is also
/// what [`read_scene`] gives back for it.
///
/// The generic reduction (`pose::degrade`) would bake skins and hierarchy too
/// the moment `morph_targets` is false, which Collada does not need: under
/// [`DaeMorphs::Bake`] only the targets go, baked per node at that node's
/// effective weights (under `Keep` they stay). Then the
/// Phong-model specifics: a `<diffuse>` is a colour OR a texture, colours pass
/// through sRGB text, roughness through a Phong exponent, and so on —
/// [`reduce_material`] lists them.
///
/// Under `Keep`, what the reader gives back differs from this scene only in
/// how meshes and nodes divide the drawing, never in what it draws: a morphed
/// mesh of several primitives reads back as a mesh and a node for each
/// primitive (SceneKit keeps one controller per node), and nodes that differed
/// only in their weights read as meshes of their own. That split is not
/// mirrored here: the comparisons that hold the contract (the scene matrix,
/// the posed-geometry tests) judge the drawing, not the node and mesh counts.
/// Everything else is modelled: target normals (`kept_target_normals`) and
/// target names (`kept_target_names`), each as the writer writes it and the
/// reader reads it back.
///
/// Once every binding Collada drops is gone (the generic reduction's, those
/// whose image is not PNG or JPEG, and [`reduce_material`]'s), each
/// primitive's uv sets are put in the order its material's remaining
/// textures sample them: base colour's set first, then emission's, then the
/// rest by index, with every binding renumbered
/// ([`pose::order_sampled_sets_first`] over [`TEXTURED_SLOTS`]). three.js
/// r183's `ColladaLoader` samples every texture from set 0 whatever the file
/// binds: `ColladaComposer`'s `getTexture` never sets a texture's `channel`
/// (`docs/superpowers/research/research-consumers.md` Q7). Readers that
/// follow the bindings, such as SceneKit and assimp, draw the same either
/// way. A map or an image Collada drops never takes set 0, a scene textured
/// on set 0 writes unchanged, and a single file, which carries no textures,
/// keeps scene order.
///
/// It does not compact vertices. The reader numbers the vertices a primitive's
/// corners name and drops the rest, so a lines primitive with a vertex no
/// segment names reads back with one fewer, as a triangle primitive does
/// today; the segments are the same.
pub fn prepare(scene: &Scene, opts: WriteOptions) -> Result<Scene, MeshError> {
    let caps = capabilities(opts);
    // `Bake` applies the targets here, so none reach `degrade`; `Keep` hands
    // it the scene with them. Either way `degrade` must not bake skins and
    // hierarchy on their account.
    let baked = match opts.dae_morphs {
        DaeMorphs::Bake => bake_morph_targets(scene),
        DaeMorphs::Keep => scene.clone(),
    };
    let mut s = pose::degrade(&baked, &Capabilities { morph_targets: true, ..caps })?;
    {
        // Only PNG and JPEG travel as sidecars every Collada reader decodes.
        let Scene { materials, textures, images, .. } = &mut s;
        for m in materials.iter_mut() {
            for (_, slot) in m.texture_slots_mut() {
                let portable = slot
                    .as_ref()
                    .and_then(|r| textures.get(r.texture))
                    .and_then(|t| images.get(t.image))
                    .is_some_and(|im| im.format.is_universal());
                if !portable {
                    *slot = None;
                }
            }
        }
    }
    for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
        // Tangents only serve normal maps, which profile_COMMON cannot carry.
        p.tangents = None;
        if opts.dae_morphs == DaeMorphs::Keep {
            kept_target_normals(p);
        }
    }
    if opts.dae_morphs == DaeMorphs::Keep {
        s.meshes.iter_mut().for_each(kept_target_names);
    }
    for m in s.materials.iter_mut() {
        reduce_material(m);
    }
    // Now that only the bindings Collada writes are left.
    pose::order_sampled_sets_first(&mut s, TEXTURED_SLOTS);
    pose::prune_unused_textures(&mut s);
    pose::nonsingular_scales(&mut s, true);
    // A directional light keeps no range, as glTF's writer keeps none
    // (`glb::light_json`): neither format applies one to it.
    for l in s.lights.iter_mut().filter(|l| l.kind == LightKind::Directional) {
        l.range = None;
    }
    s.validate()?;
    Ok(s)
}

/// A kept primitive's target normals as the reader gives them back from what
/// the writer writes (`DaeMorphs::Keep`). Every target geometry carries
/// normals when its base does, a target with no normal offsets carrying the
/// base's own, so it reads back with offsets of zero; and no normal is
/// written for a base with none, so its targets' offsets are gone.
fn kept_target_normals(p: &mut Primitive) {
    match &p.normals {
        None => p.targets.iter_mut().for_each(|t| t.normals = None),
        Some(base) => {
            let zero = vec![[0.0; 3]; base.len()];
            for t in p.targets.iter_mut().filter(|t| t.normals.is_none()) {
                t.normals = Some(zero.clone());
            }
        }
    }
}

/// A kept mesh's target names as the reader gives them back from what the
/// writer writes (`DaeMorphs::Keep`): each target geometry's `name` attribute,
/// an NCName (`ncname`), and for a target with no name `target{i}`, which the
/// writer then writes. The reader would otherwise name it by its geometry's id.
fn kept_target_names(mesh: &mut SceneMesh) {
    if mesh.target_count() == 0 {
        return;
    }
    mesh.target_names = (0..mesh.target_count())
        .map(|i| ncname(mesh.target_names.get(i).map(String::as_str)).unwrap_or_else(|| format!("target{i}")))
        .collect();
}

/// Every mesh with morph targets replaced, per node, by its geometry at that
/// node's effective weights; weight channels removed. Instancing survives
/// wherever nodes share a mesh AND weights.
fn bake_morph_targets(scene: &Scene) -> Scene {
    let mut s = scene.clone();
    if !scene.meshes.iter().any(|m| m.target_count() > 0) {
        return s;
    }
    let weights = pose::default_pose(scene).weights;
    let mut out: Vec<SceneMesh> = Vec::new();
    let mut plain: Vec<Option<usize>> = vec![None; scene.meshes.len()];
    let mut baked: HashMap<(usize, Vec<u32>), usize> = HashMap::new();
    for ni in 0..s.nodes.len() {
        let Some(mi) = s.nodes[ni].mesh else { continue };
        let Some(mesh) = scene.meshes.get(mi) else { continue };
        let index = if mesh.target_count() == 0 {
            match plain[mi] {
                Some(i) => i,
                None => {
                    out.push(mesh.clone());
                    plain[mi] = Some(out.len() - 1);
                    out.len() - 1
                }
            }
        } else {
            let w = weights.get(ni).cloned().unwrap_or_default();
            let key = (mi, w.iter().map(|x| x.to_bits()).collect::<Vec<u32>>());
            match baked.get(&key) {
                Some(&i) => i,
                None => {
                    out.push(SceneMesh {
                        name: mesh.name.clone(),
                        primitives: mesh
                            .primitives
                            .iter()
                            .map(|p| {
                                let (positions, normals) = pose::morphed(p, &w);
                                Primitive { positions, normals, targets: Vec::new(), ..p.clone() }
                            })
                            .collect(),
                        weights: Vec::new(),
                        target_names: Vec::new(),
                    });
                    baked.insert(key, out.len() - 1);
                    out.len() - 1
                }
            }
        };
        s.nodes[ni].mesh = Some(index);
        s.nodes[ni].weights = None;
    }
    s.meshes = out;
    for a in s.animations.iter_mut() {
        a.channels.retain(|c| c.path != AnimPath::Weights);
    }
    s.animations.retain(|a| !a.channels.is_empty());
    s
}

/// The `<index_of_refraction>` values exporters write on every material by
/// default, which the reader takes as unset (module docs): SceneKit's 1, and
/// Blender's Principled BSDF default, 1.45 in 2.8-3.6 and 1.5 in 4.0-4.5.
const DEFAULT_IORS: [f32; 3] = [1.0, 1.45, 1.5];

/// A material as profile_COMMON carries it, which is exactly what the reader
/// rebuilds from what the writer emits for it.
fn reduce_material(m: &mut Material) {
    // <diffuse> holds a colour or a texture, never their product.
    if m.base_color_texture.is_some() {
        m.base_color[0] = 1.0;
        m.base_color[1] = 1.0;
        m.base_color[2] = 1.0;
    } else {
        for c in m.base_color.iter_mut().take(3) {
            *c = srgb_decode(srgb_encode(*c));
        }
    }
    // <emission> likewise; glTF multiplies its texture by the factor, so a
    // zero factor means the texture shows nothing at all.
    match (&m.emissive_texture, m.emissive) {
        (Some(_), [0.0, 0.0, 0.0]) => m.emissive_texture = None,
        (Some(_), _) => m.emissive = [1.0; 3],
        (None, _) => {
            for c in m.emissive.iter_mut() {
                *c = srgb_decode(srgb_encode(*c));
            }
        }
    }
    m.normal_texture = None;
    m.normal_scale = 1.0;
    m.occlusion_texture = None;
    m.occlusion_strength = 1.0;
    m.metallic_roughness_texture = None;
    m.metallic = 0.0;
    m.roughness = if m.roughness >= 1.0 {
        1.0
    } else {
        roughness_from_shininess(shininess_from_roughness(m.roughness))
    };
    m.unlit = false;
    // Collada has transparency but no alpha test.
    match m.alpha_mode {
        AlphaMode::Opaque => m.base_color[3] = 1.0,
        AlphaMode::Mask => {
            if m.base_color_texture.is_some() {
                m.alpha_mode = AlphaMode::Blend;
            } else if m.base_color[3] >= m.alpha_cutoff {
                m.alpha_mode = AlphaMode::Opaque;
                m.base_color[3] = 1.0;
            } else {
                m.alpha_mode = AlphaMode::Blend;
                m.base_color[3] = 0.0;
            }
        }
        AlphaMode::Blend => {
            if m.base_color_texture.is_none() && m.base_color[3] >= 1.0 {
                m.alpha_mode = AlphaMode::Opaque;
                m.base_color[3] = 1.0;
            }
        }
    }
    m.alpha_cutoff = 0.5;
    // An exporter's default reads back as unset, and so does KHR's 0, which
    // is no refractive index (Ruling A42).
    if m.ior.is_some_and(|v| v == 0.0 || DEFAULT_IORS.contains(&v)) {
        m.ior = None;
    }
}

/// What a Collada document loses beyond the generic capability table, named.
/// `scene` is the input, before [`prepare`].
pub fn extra_losses(scene: &Scene, opts: WriteOptions) -> Vec<Loss> {
    let mut out = Vec::new();
    let mut push = |kind: LossKind, what: String, why: &str| out.push(Loss::new(kind, what, format!("dae {why}")));
    if scene.meshes.iter().flat_map(|m| &m.primitives).any(|p| p.colors.as_ref().is_some_and(|c| c.iter().any(|c| c[3] < 1.0))) {
        push(LossKind::Alpha, "vertex colour alpha".into(), "readers draw vertex colour as RGB; Collada's transparency is a material's");
    }
    // Materials matching `f`, named (by scene index when unnamed).
    let pick = |f: &dyn Fn(&Material) -> bool| -> Vec<String> {
        scene
            .materials
            .iter()
            .enumerate()
            .filter(|(_, m)| f(m))
            .map(|(i, m)| m.name.clone().unwrap_or_else(|| format!("#{i}")))
            .collect()
    };
    let weight_channels =
        scene.animations.iter().flat_map(|a| &a.channels).filter(|c| c.path == AnimPath::Weights).count();
    if weight_channels > 0 && opts.dae_morphs == DaeMorphs::Bake {
        push(
            LossKind::MorphTargets,
            format!("{weight_channels} morph-weight animation channel(s)"),
            "is written without morph controllers (three.js and assimp fail to load them); the targets are \
             applied at their default weights and their animation cannot follow",
        );
    }
    if !opts.self_contained {
        let tinted = pick(&|m| m.base_color_texture.is_some() && m.base_color[..3] != [1.0; 3]);
        if !tinted.is_empty() {
            push(
                LossKind::Materials,
                format!("base colour factor of {} textured material(s) [{}]", tinted.len(), tinted.join(", ")),
                "has a diffuse colour OR texture, not their product; the texture is kept",
            );
        }
        let glow = pick(&|m| m.emissive_texture.is_some() && m.emissive != [0.0; 3] && m.emissive != [1.0; 3]);
        if !glow.is_empty() {
            push(
                LossKind::Materials,
                format!("emissive factor of {} material(s) [{}]", glow.len(), glow.join(", ")),
                "has an emission colour OR texture, not their product; the texture is kept",
            );
        }
        let normal_maps = scene.materials.iter().filter(|m| m.normal_texture.is_some()).count();
        if normal_maps > 0 {
            push(
                LossKind::Textures,
                format!("{normal_maps} normal map(s)"),
                "profile_COMMON has no normal-map input (`bump` means a height map to most readers)",
            );
        }
        // Texture-transform conflicts are named by `scene::losses` for every
        // container without texture transforms, Collada included.
    }
    let unlit = pick(&|m| m.unlit);
    if !unlit.is_empty() {
        push(
            LossKind::Materials,
            format!("unlit shading of {} material(s) [{}]", unlit.len(), unlit.join(", ")),
            "is written as lambert: a schema-valid <constant> has no diffuse, and three.js renders it white",
        );
    }
    let masked = pick(&|m| m.alpha_mode == AlphaMode::Mask);
    if !masked.is_empty() {
        push(
            LossKind::Alpha,
            format!("alpha cutoff of {} material(s) [{}]", masked.len(), masked.join(", ")),
            "has transparency but no alpha test; MASK is written as blending",
        );
    }
    // KHR's 0 reads back as unset: it is no refractive index (Ruling A42),
    // named as OBJ names its `Ni 0`.
    let n = scene.materials.iter().filter(|m| m.ior == Some(0.0)).count();
    if n > 0 {
        push(
            LossKind::Materials,
            format!("an ior of 0 on {n} material(s)"),
            "leaves an ior of 0 out, as its reader takes an <index_of_refraction> of 0 or below as no refractive index",
        );
    }
    // An exporter's default ior reads back as unset (`DEFAULT_IORS`). 1.5,
    // glTF's own default, draws the same unset, so it goes unnamed.
    for v in DEFAULT_IORS.into_iter().filter(|&v| v != super::scene::DEFAULT_IOR) {
        let n = scene.materials.iter().filter(|m| m.ior == Some(v)).count();
        if n > 0 {
            push(
                LossKind::Materials,
                format!("an ior of {v} on {n} material(s)"),
                &format!(
                    "reads an <index_of_refraction> of {v} as unset, since exporters write it on every material by default \
                     (SceneKit 1, Blender 2.8-3.6 1.45); the ior is dropped"
                ),
            );
        }
    }
    // glTF's `range` has no Collada form (Ruling R27): other readers light a
    // point or spot light past it, and only litegen's own <extra> keeps it.
    // A directional light's goes unnamed: `prepare` drops it, as glTF's
    // writer does, and no reader applies one.
    let ranged: Vec<String> = scene
        .lights
        .iter()
        .enumerate()
        .filter(|(_, l)| l.kind != LightKind::Directional && l.range.is_some())
        .map(|(i, l)| l.name.clone().unwrap_or_else(|| format!("#{i}")))
        .collect();
    if !ranged.is_empty() {
        push(
            LossKind::Lights,
            format!("the range of {} light(s) [{}]", ranged.len(), ranged.join(", ")),
            "has no light range, so other readers light past it; only litegen's own <extra> keeps it",
        );
    }
    // `<zfar>` is required: other readers see the far plane written for an
    // infinite one (litegen's own reads it back from the extra).
    let infinite: Vec<String> = scene
        .cameras
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(c.projection, Projection::Perspective { zfar: None, .. }))
        .map(|(i, c)| c.name.clone().unwrap_or_else(|| format!("#{i}")))
        .collect();
    if !infinite.is_empty() {
        push(
            LossKind::Cameras,
            format!("the infinite far plane of {} camera(s) [{}]", infinite.len(), infinite.join(", ")),
            "always stores a far plane (<zfar> is required); 1000 m (Blender's default clip end), or 10,000 times a near plane \
             at or past it, was written for other readers, and litegen's own <extra> keeps it infinite",
        );
    }
    out
}

// ─── lights and cameras ─────────────────────────────────────────────────────

/// The `<technique>` profile of the `<extra>` that holds a light's or a
/// camera's own values (module docs, "Lights and cameras").
const LITEGEN_PROFILE: &str = "litegen";

/// Degrees of `<falloff_angle>` per degree of a spot's outer cone, which is
/// a half-angle: 2, since `<falloff_angle>` is the full cone (Ruling R23,
/// which revises R4; module docs, and `collada_falloff_angle_follows_the_spec`
/// quotes the 1.4.1 `<spot>` entry).
const FALLOFF_ANGLE_PER_HALF_ANGLE: f64 = 2.0;

/// Numbers charged to the parse budget for each light or camera built: more
/// than a `<light>` or a `<camera>` and its litegen technique hold.
const LIGHT_OR_CAMERA_VALUES: usize = 16;

/// A spot's `<falloff_angle>`: the full cone, in degrees, around its outer
/// cone, a half-angle in radians (Ruling R23), at most 180. An f32 π/2 is a
/// hair past 90°, and 180 reads back as the same f32.
fn falloff_angle(outer_cone: f32) -> f64 {
    f64::from(outer_cone).min(std::f64::consts::FRAC_PI_2).to_degrees() * FALLOFF_ANGLE_PER_HALF_ANGLE
}

/// The outer cone, a half-angle in radians, of a `<falloff_angle>`, a full
/// cone in degrees: [`falloff_angle`] inverted.
fn outer_cone(falloff_angle: f64) -> f64 {
    (falloff_angle / FALLOFF_ANGLE_PER_HALF_ANGLE).to_radians()
}

/// A spot's `<falloff_exponent>`: the exponent e for which the spec's beam,
/// cosᵉ of the angle from the axis, falls to half its axis' intensity at the
/// angle where glTF's beam does, between `inner_cone` and `outer_cone`
/// (half-angles, radians). glTF's falloff is the KHR_lights_punctual
/// README's, `((cos θ − cos outer) / (cos inner − cos outer))²`. It is 1/2
/// where `cos θ = cos outer + (cos inner − cos outer) / √2`, so
/// `e = −ln 2 / ln cos θ`. [`inner_cone`] inverts it. A cone too narrow for
/// f64's cosine to tell from its axis (under about 1e-8 rad) has no finite
/// exponent, and gets the largest.
fn falloff_exponent(inner_cone: f32, outer_cone: f32) -> f64 {
    let co = f64::from(outer_cone).min(std::f64::consts::FRAC_PI_2).cos();
    let ci = f64::from(inner_cone).cos();
    let half = co + (ci - co) / std::f64::consts::SQRT_2;
    let e = -std::f64::consts::LN_2 / half.ln();
    if e.is_finite() && e >= 0.0 {
        e
    } else {
        f64::MAX
    }
}

/// The inner cone, in radians, of a spot whose outer cone is `outer_cone`
/// and whose `<falloff_exponent>` is `exponent` (0 or more):
/// [`falloff_exponent`] inverted. The beam is at half where
/// `cos θ = 2^(−1/e)`, and glTF's falloff is at half there when
/// `cos inner = cos outer + √2 (cos θ − cos outer)`. An exponent too weak to
/// halve the beam inside the outer cone, the spec's default 0 (a uniform cone)
/// among them, gives a hard edge: the outer cone itself, which the reader
/// softens (`Light::soften_hard_edge`). An exponent too strong for the cone
/// gives an inner cone of 0.
fn inner_cone(outer_cone: f64, exponent: f64) -> f64 {
    let co = outer_cone.min(std::f64::consts::FRAC_PI_2).cos();
    let half = if exponent > 0.0 { 0.5f64.powf(1.0 / exponent) } else { 0.0 };
    let ci = co + std::f64::consts::SQRT_2 * (half - co);
    if !(ci > co) {
        outer_cone
    } else if ci >= 1.0 {
        0.0
    } else {
        ci.acos()
    }
}

/// A light or camera as a warning names it: by its `name`, else its `id`,
/// quoted and cut short ([`q`]).
fn labelled(kind: &str, el: &Element) -> String {
    match el.attr("name").filter(|n| !n.is_empty()).or_else(|| el.attr("id").filter(|i| !i.is_empty())) {
        Some(n) => format!("{kind} {}", q(n)),
        None => format!("{kind} with no name or id"),
    }
}

/// The `n` numbers `e` holds, or what is wrong with them, worded to follow
/// `its <tag> `.
fn numbers(e: &Element, n: usize) -> Result<Vec<f64>, String> {
    let tokens: Vec<&str> = e.text.split_ascii_whitespace().take(n + 1).collect();
    if tokens.len() != n {
        let held = if tokens.len() > n { format!("more than {n} values") } else { format!("{} value(s)", tokens.len()) };
        return Err(format!("holds {held}, not {n}"));
    }
    tokens.iter().map(|t| t.parse::<f64>().map_err(|_| format!("holds {}, which is not a number", q(t)))).collect()
}

/// The number `e`'s child `<name>` holds; `None` when `e` has no such child.
fn optional_number(e: &Element, name: &str) -> Result<Option<f64>, String> {
    kid(e, name).map(|c| numbers(c, 1).map(|v| v[0]).map_err(|why| format!("its <{name}> {why}"))).transpose()
}

/// The litegen technique of the first of `extras` that has one.
fn litegen_technique<'e>(mut extras: impl Iterator<Item = &'e Element>) -> Option<&'e Element> {
    extras.find_map(|x| x.children.iter().find(|t| t.name == "technique" && t.attr("profile") == Some(LITEGEN_PROFILE)))
}

/// The scene's own name of a light or camera, as its litegen technique
/// holds it (`<gltf_name>`, which the writer leaves out for one with no
/// name): `None` without one, as every value the technique leaves out is
/// unset, whatever the `name` attribute, an NCName, says.
fn exact_name(t: &Element) -> Option<String> {
    kid(t, "gltf_name").map(|n| n.text.clone()).filter(|n| !n.is_empty())
}

/// A `<light>` as the scene's light, with what reading it should warn of
/// (each worded to follow `light "lamp" `), or why it does not read. Its
/// bounds are `Light::check`'s to judge. `meter` is the document's `<unit>`.
/// The litegen technique gives the light exactly when it reads
/// ([`exact_light`]); otherwise the common technique gives it by the
/// documented mapping (module docs, "What is read").
fn read_light(el: &Element, meter: f64) -> Result<(Light, Vec<String>), String> {
    let common = kid(el, "technique_common").ok_or("it has no <technique_common>")?;
    let shape = common
        .children
        .iter()
        .find(|c| matches!(c.name.as_str(), "ambient" | "directional" | "point" | "spot"))
        .ok_or("its <technique_common> holds no <directional>, <point>, <spot> or <ambient>")?;
    if shape.name == "ambient" {
        let colour = match kid(shape, "color").map(|c| (numbers(c, 3), c)) {
            Some((Ok(v), _)) => format!("({}, {}, {})", v[0], v[1], v[2]),
            Some((Err(_), c)) => q(&c.text),
            None => "unstated".into(),
        };
        return Err(format!(
            "it is an ambient light, colour {colour}, which the scene does not carry: it holds directional, point and spot lights only"
        ));
    }
    let name = el.attr("name").filter(|n| !n.is_empty()).map(str::to_string);
    let mut kind = match shape.name.as_str() {
        "directional" => LightKind::Directional,
        "point" => LightKind::Point,
        _ => LightKind::Spot { inner_cone: 0.0, outer_cone: 0.0 },
    };
    let mut notes = Vec::new();
    if let Some(t) = litegen_technique(kids(el, "extra")) {
        match exact_light(t, kind, meter) {
            Ok((light, more)) => return Ok((Light { name: exact_name(t), ..light }, more)),
            Err(why) => notes.push(format!("has a litegen <extra> that does not read ({why}), so its common technique was read")),
        }
    }
    // Linear colour times intensity (Ruling R17).
    let colour = numbers(kid(shape, "color").ok_or("it has no <color>")?, 3).map_err(|why| format!("its <color> {why}"))?;
    let linear = [colour[0], colour[1], colour[2]];
    // A point or spot light's brightness at 1 m: Dist is in the document's
    // unit, so 1 m is 1 / meter of them.
    let mut at_one_metre = 1.0;
    if kind != LightKind::Directional {
        let term = |n: &str, default: f64| optional_number(shape, n).map(|v| v.unwrap_or(default));
        let (k, l, q) = (term("constant_attenuation", 1.0)?, term("linear_attenuation", 0.0)?, term("quadratic_attenuation", 0.0)?);
        at_one_metre = k + l / meter + q / (meter * meter);
        if !(at_one_metre > 0.0 && at_one_metre.is_finite()) {
            return Err(format!("its attenuation (constant {k}, linear {l}, quadratic {q}) leaves it no light at 1 m"));
        }
        if !(k == 0.0 && l == 0.0) {
            notes.push(format!(
                "has the attenuation constant {k}, linear {l}, quadratic {q}, not the inverse-square falloff the scene's lights \
                 have; it was read with that falloff, as bright as it is at 1 m"
            ));
        }
    }
    if let LightKind::Spot { .. } = kind {
        // The full cone (Ruling R23): the default 180 is a hemisphere. A
        // wider one lights nearly every way, past glTF's bound on a spot's
        // outer cone (`Light::check`), so it is a point light (Ruling R32).
        let angle = optional_number(shape, "falloff_angle")?.unwrap_or(180.0);
        let exponent = optional_number(shape, "falloff_exponent")?.unwrap_or(0.0);
        if !(exponent >= 0.0) {
            return Err(format!("its falloff exponent {exponent} is not a number of 0 or more"));
        }
        let outer = outer_cone(angle);
        kind = if outer.is_finite() && (outer as f32) > std::f32::consts::FRAC_PI_2 {
            notes.push(format!("has a <falloff_angle> of {angle}°, a cone wider than a hemisphere (180°), so it was read as a point light"));
            LightKind::Point
        } else {
            LightKind::Spot { inner_cone: inner_cone(outer, exponent) as f32, outer_cone: outer as f32 }
        };
    }
    // Ruling R8: the largest component moves into the intensity, so the
    // colour ends in 0 to 1.
    let peak = linear.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let color = if peak > 0.0 { linear.map(|c| (c / peak) as f32) } else { linear.map(|c| c as f32) };
    Ok((Light { name, kind, color, intensity: (peak / at_one_metre) as f32, range: None }, notes))
}

/// The light a litegen technique holds: the scene's own colour, intensity,
/// range and cones, as the writer wrote them, with lengths in the
/// document's `<unit>`. `kind` comes from the common technique, which the
/// technique's cones must fit.
fn exact_light(t: &Element, kind: LightKind, meter: f64) -> Result<(Light, Vec<String>), String> {
    let need = |n: &str| optional_number(t, n)?.ok_or_else(|| format!("it has no <{n}>"));
    let c = numbers(kid(t, "gltf_color").ok_or("it has no <gltf_color>")?, 3).map_err(|why| format!("its <gltf_color> {why}"))?;
    let intensity = need("gltf_intensity")?;
    let range = optional_number(t, "gltf_range")?;
    let kind = match kind {
        LightKind::Spot { .. } => LightKind::Spot {
            inner_cone: need("gltf_inner_cone_angle")? as f32,
            outer_cone: need("gltf_outer_cone_angle")? as f32,
        },
        other => other,
    };
    let mut notes = Vec::new();
    let range = match (kind, range) {
        (LightKind::Directional, Some(r)) => {
            notes.push(format!("is directional, so the range {r} its litegen <extra> gives is ignored"));
            None
        }
        (_, r) => r.map(|r| (r * meter) as f32),
    };
    let light = Light { name: None, kind, color: [c[0] as f32, c[1] as f32, c[2] as f32], intensity: intensity as f32, range };
    Ok((light, notes))
}

/// A `<camera>` as the scene's camera, with what reading it should warn of
/// (each worded to follow `camera "shot" `), or why it does not read. Its
/// bounds are `Camera::check`'s to judge. The litegen technique gives the
/// projection exactly when it reads ([`exact_projection`]); otherwise the
/// common technique gives it, every combination the spec allows (module
/// docs, "What is read").
fn read_camera(el: &Element, meter: f64) -> Result<(Camera, Vec<String>), String> {
    let optics = kid(el, "optics").ok_or("it has no <optics>")?;
    let common = kid(optics, "technique_common").ok_or("its <optics> has no <technique_common>")?;
    let shape = common
        .children
        .iter()
        .find(|c| c.name == "perspective" || c.name == "orthographic")
        .ok_or("its <optics> holds no <perspective> or <orthographic>")?;
    let name = el.attr("name").filter(|n| !n.is_empty()).map(str::to_string);
    let perspective = shape.name == "perspective";
    let mut notes = Vec::new();
    if let Some(t) = litegen_technique(kids(optics, "extra")) {
        match exact_projection(t, perspective, meter) {
            Ok(projection) => return Ok((Camera { name: exact_name(t), projection }, notes)),
            Err(why) => notes.push(format!("has a litegen <extra> that does not read ({why}), so its common technique was read")),
        }
    }
    let value = |n: &str| optional_number(shape, n);
    let need = |n: &str| value(n)?.ok_or_else(|| format!("its <{}> has no <{n}>", shape.name));
    let (znear, zfar, aspect) = (need("znear")? * meter, need("zfar")? * meter, value("aspect_ratio")?);
    // The viewport an aspect ratio left to it is taken at: FBX's, for a
    // camera with none.
    let viewport = f64::from(super::fbx::UNSET_ASPECT);
    let projection = if perspective {
        // The aspect ratio is the view's width over its height (p. 5-93):
        // tan(xfov/2) / tan(yfov/2).
        let half = |degrees: f64| (degrees.to_radians() / 2.0).tan();
        let (yfov, aspect) = match (value("xfov")?, value("yfov")?) {
            (x, Some(y)) => (y.to_radians(), aspect.or_else(|| x.map(|x| half(x) / half(y)))),
            (Some(x), None) => match aspect {
                Some(a) => (2.0 * (half(x) / a).atan(), Some(a)),
                None => {
                    notes.push(format!("gives only a horizontal field of view, xfov {x}°, so its vertical one was taken at a 16:9 viewport"));
                    (2.0 * (half(x) / viewport).atan(), None)
                }
            },
            (None, None) => return Err("its <perspective> has neither an <xfov> nor a <yfov>".into()),
        };
        Projection::Perspective { yfov: yfov as f32, aspect: aspect.map(|a| a as f32), znear: znear as f32, zfar: Some(zfar as f32) }
    } else {
        let (xmag, ymag) = match (value("xmag")?, value("ymag")?, aspect) {
            (Some(x), Some(y), _) => (x, y),
            (Some(x), None, Some(a)) => (x, x / a),
            (None, Some(y), Some(a)) => (y * a, y),
            (Some(x), None, None) => {
                notes.push(format!("gives only a horizontal magnification, xmag {x}, so its vertical one was taken at a 16:9 viewport"));
                (x, x / viewport)
            }
            (None, Some(y), None) => {
                notes.push(format!("gives only a vertical magnification, ymag {y}, so its horizontal one was taken at a 16:9 viewport"));
                (y * viewport, y)
            }
            (None, None, _) => return Err("its <orthographic> has neither an <xmag> nor a <ymag>".into()),
        };
        Projection::Orthographic { xmag: (xmag * meter) as f32, ymag: (ymag * meter) as f32, znear: znear as f32, zfar: zfar as f32 }
    };
    Ok((Camera { name, projection }, notes))
}

/// The projection a litegen technique holds: the scene's own, as the writer
/// wrote it, an unset aspect ratio or far plane left out, with lengths in
/// the document's `<unit>`. `perspective` comes from the common technique.
fn exact_projection(t: &Element, perspective: bool, meter: f64) -> Result<Projection, String> {
    let need = |n: &str| optional_number(t, n)?.ok_or_else(|| format!("it has no <{n}>"));
    let length = |n: &str| need(n).map(|v| (v * meter) as f32);
    Ok(if perspective {
        Projection::Perspective {
            yfov: need("gltf_yfov")? as f32,
            aspect: optional_number(t, "gltf_aspect_ratio")?.map(|a| a as f32),
            znear: length("gltf_znear")?,
            zfar: optional_number(t, "gltf_zfar")?.map(|f| (f * meter) as f32),
        }
    } else {
        Projection::Orthographic { xmag: length("gltf_xmag")?, ymag: length("gltf_ymag")?, znear: length("gltf_znear")?, zfar: length("gltf_zfar")? }
    })
}

impl<'a> Reader<'a> {
    /// The lights and cameras the nodes instance ([`read_light`],
    /// [`read_camera`]), each `<light>` and `<camera>` read once however
    /// many nodes instance it, and charged to the parse budget before it is
    /// built; then a warning for each one that no node of the scene
    /// instances, which is dropped (Ruling R13). A reader never refuses a
    /// file over a light or camera: one that does not read or breaks a bound
    /// (`Light::check`, `Camera::check`) is dropped with a warning naming it
    /// and why, and its node stays; a hard-edged spot is softened instead
    /// (`Light::soften_hard_edge`).
    fn lights_and_cameras(&mut self) -> Result<(), MeshError> {
        let mut lights: HashMap<usize, Option<usize>> = HashMap::new();
        for (ir, inst) in std::mem::take(&mut self.instanced_lights) {
            let Some(el) = self.instanced(ir, inst, "light") else { continue };
            let kept = match lights.get(&mat_ptr(el)) {
                Some(&kept) => kept,
                None => {
                    self.charge(LIGHT_OR_CAMERA_VALUES, "a light")?;
                    let label = labelled("light", el);
                    let read = read_light(el, self.meter).and_then(|(mut l, mut notes)| {
                        notes.extend(l.soften_hard_edge());
                        l.check().map(|()| (l, notes))
                    });
                    let kept = match read {
                        Ok((l, notes)) => {
                            for n in notes {
                                self.warn(format!("{label} {n}"));
                            }
                            self.scene.lights.push(l);
                            Some(self.scene.lights.len() - 1)
                        }
                        Err(why) => {
                            self.warn(format!("{label} is dropped: {why}"));
                            None
                        }
                    };
                    lights.insert(mat_ptr(el), kept);
                    kept
                }
            };
            self.scene.nodes[ir].light = kept;
        }
        let mut cameras: HashMap<usize, Option<usize>> = HashMap::new();
        for (ir, inst) in std::mem::take(&mut self.instanced_cameras) {
            let Some(el) = self.instanced(ir, inst, "camera") else { continue };
            let kept = match cameras.get(&mat_ptr(el)) {
                Some(&kept) => kept,
                None => {
                    self.charge(LIGHT_OR_CAMERA_VALUES, "a camera")?;
                    let label = labelled("camera", el);
                    let kept = match read_camera(el, self.meter).and_then(|(c, notes)| c.check().map(|()| (c, notes))) {
                        Ok((c, notes)) => {
                            for n in notes {
                                self.warn(format!("{label} {n}"));
                            }
                            self.scene.cameras.push(c);
                            Some(self.scene.cameras.len() - 1)
                        }
                        Err(why) => {
                            self.warn(format!("{label} is dropped: {why}"));
                            None
                        }
                    };
                    cameras.insert(mat_ptr(el), kept);
                    kept
                }
            };
            self.scene.nodes[ir].camera = kept;
        }
        self.unplaced("light", "library_lights", &lights);
        self.unplaced("camera", "library_cameras", &cameras);
        Ok(())
    }

    /// The `<light>` or `<camera>` (`kind`) that node `ir`'s instance names,
    /// or `None`, with a warning, when its url names none in the document.
    /// The node is kept either way.
    fn instanced(&mut self, ir: usize, inst: &'a Element, kind: &str) -> Option<&'a Element> {
        let url = inst.attr("url").unwrap_or("");
        let found = self.doc.get(url);
        if let Some(el) = found.filter(|e| e.name == kind) {
            return Some(el);
        }
        let why = match found {
            Some(e) => format!("which is a <{}>, not a <{kind}>", clip(&e.name, 30)),
            None if !url.trim().starts_with('#') => "outside this document (only #id references within the file are followed)".into(),
            None => "which names nothing in this document".into(),
        };
        let node = self.info[ir].id.or(self.info[ir].name).unwrap_or("");
        self.warn(format!("node {} instances {kind} {}, {why}; the node is kept without it", q(node), q(url)));
        None
    }

    /// A warning for each `<light>` or `<camera>` (`kind`) of `library` that
    /// no node of the scene instanced (`read` holds those one did): each is
    /// dropped (Ruling R13, as glTF's reader drops one no node places). They
    /// are named one by one while the node budget lasts (`MAX_NODES`, as
    /// many as a scene can place), and counted past it.
    fn unplaced(&mut self, kind: &str, library: &str, read: &HashMap<usize, Option<usize>>) {
        let (mut named, mut more) = (0usize, 0usize);
        for el in self.doc.library(library, kind) {
            if read.contains_key(&mat_ptr(el)) {
                continue;
            }
            if named < MAX_NODES {
                self.warn(format!("{} is instanced by no node of the scene, so it is dropped", labelled(kind, el)));
                named += 1;
            } else {
                more += 1;
            }
        }
        if more > 0 {
            self.warn(format!("{more} more {kind}(s) are instanced by no node of the scene, so they are dropped"));
        }
    }
}

impl Writer<'_> {
    /// `<library_cameras>`: each camera's common-profile `<optics>` (module
    /// docs, "Lights and cameras"), then the litegen technique that holds
    /// its projection exactly.
    fn library_cameras(&mut self) {
        let s = self.s;
        self.out.open("library_cameras", &[]);
        for (i, c) in s.cameras.iter().enumerate() {
            let id = self.camera_ids[i].clone();
            // assimp 6.0.5 reads no optics for a <camera> with no `name`
            // (`ColladaParser.cpp` L1049-L1051), so an unnamed camera is
            // named by its id; its extra, with no <gltf_name>, keeps it
            // unnamed for litegen.
            let name = ncname(c.name.as_deref()).unwrap_or_else(|| id.clone());
            self.out.open("camera", &[("id", id.as_str()), ("name", name.as_str())]);
            self.out.open("optics", &[]);
            self.out.open("technique_common", &[]);
            let exact = match c.projection {
                Projection::Perspective { yfov, aspect, znear, zfar } => {
                    self.out.open("perspective", &[]);
                    self.out.numbers("yfov", &[], &join_f64([f64::from(yfov).to_degrees()]));
                    if let Some(a) = aspect {
                        self.out.numbers("aspect_ratio", &[], &join_f32([a]));
                    }
                    self.out.numbers("znear", &[], &join_f32([znear]));
                    self.out.numbers("zfar", &[], &join_f32([zfar.unwrap_or_else(|| super::fbx::unset_far(znear))]));
                    self.out.close("perspective");
                    let mut exact = vec![("gltf_yfov", yfov)];
                    exact.extend(aspect.map(|a| ("gltf_aspect_ratio", a)));
                    exact.push(("gltf_znear", znear));
                    exact.extend(zfar.map(|f| ("gltf_zfar", f)));
                    exact
                }
                Projection::Orthographic { xmag, ymag, znear, zfar } => {
                    self.out.open("orthographic", &[]);
                    for (tag, v) in [("xmag", xmag), ("ymag", ymag), ("znear", znear), ("zfar", zfar)] {
                        self.out.numbers(tag, &[], &join_f32([v]));
                    }
                    self.out.close("orthographic");
                    vec![("gltf_xmag", xmag), ("gltf_ymag", ymag), ("gltf_znear", znear), ("gltf_zfar", zfar)]
                }
            };
            self.out.close("technique_common");
            self.litegen_extra(c.name.as_deref(), &exact.into_iter().map(|(tag, v)| (tag, join_f32([v]))).collect::<Vec<_>>());
            self.out.close("optics");
            self.out.close("camera");
        }
        self.out.close("library_cameras");
    }

    /// `<library_lights>`: each light's common-profile `<directional>`,
    /// `<point>` or `<spot>` (module docs, "Lights and cameras"), then the
    /// litegen technique that holds its colour, intensity, range and cones
    /// exactly. A directional light has no range (`prepare`).
    fn library_lights(&mut self) {
        let s = self.s;
        self.out.open("library_lights", &[]);
        for (i, l) in s.lights.iter().enumerate() {
            let id = self.light_ids[i].clone();
            let name = ncname(l.name.as_deref());
            let mut attrs = vec![("id", id.as_str())];
            if let Some(n) = name.as_deref() {
                attrs.push(("name", n));
            }
            self.out.open("light", &attrs);
            self.out.open("technique_common", &[]);
            let shape = match l.kind {
                LightKind::Directional => "directional",
                LightKind::Point => "point",
                LightKind::Spot { .. } => "spot",
            };
            self.out.open(shape, &[]);
            // Linear colour times intensity (Ruling R17).
            let shown = l.color.map(|c| f64::from(c) * f64::from(l.intensity));
            self.out.numbers("color", &[], &join_f64(shown));
            if l.kind != LightKind::Directional {
                // The inverse square (module docs).
                self.out.numbers("constant_attenuation", &[], "0");
                self.out.numbers("linear_attenuation", &[], "0");
                self.out.numbers("quadratic_attenuation", &[], "1");
            }
            let mut exact = vec![("gltf_color", join_f32(l.color)), ("gltf_intensity", join_f32([l.intensity]))];
            if let Some(r) = l.range.filter(|_| l.kind != LightKind::Directional) {
                exact.push(("gltf_range", join_f32([r])));
            }
            if let LightKind::Spot { inner_cone, outer_cone } = l.kind {
                self.out.numbers("falloff_angle", &[], &join_f64([falloff_angle(outer_cone)]));
                self.out.numbers("falloff_exponent", &[], &join_f64([falloff_exponent(inner_cone, outer_cone)]));
                exact.push(("gltf_inner_cone_angle", join_f32([inner_cone])));
                exact.push(("gltf_outer_cone_angle", join_f32([outer_cone])));
            }
            self.out.close(shape);
            self.out.close("technique_common");
            self.litegen_extra(l.name.as_deref(), &exact);
            self.out.close("light");
        }
        self.out.close("library_lights");
    }

    /// `<extra><technique profile="litegen">` with the scene's own `name`,
    /// whole, as `<gltf_name>` when it has one (the `name` attribute holds
    /// only its NCName), then an element per value, each the scene's own
    /// number(s) as text.
    fn litegen_extra(&mut self, name: Option<&str>, values: &[(&str, String)]) {
        self.out.open("extra", &[]);
        self.out.open("technique", &[("profile", LITEGEN_PROFILE)]);
        if let Some(n) = name.filter(|n| !n.is_empty()) {
            self.out.leaf("gltf_name", &[], n);
        }
        for (tag, text) in values {
            self.out.numbers(tag, &[], text);
        }
        self.out.close("technique");
        self.out.close("extra");
    }

    /// Node `ni`'s `<instance_camera>`, when it places a camera.
    fn instance_camera(&mut self, ni: usize) {
        if let Some(c) = self.s.nodes[ni].camera {
            let url = format!("#{}", self.camera_ids[c]);
            self.out.empty("instance_camera", &[("url", &url)]);
        }
    }

    /// Node `ni`'s `<instance_light>`, when it places a light.
    fn instance_light(&mut self, ni: usize) {
        if let Some(l) = self.s.nodes[ni].light {
            let url = format!("#{}", self.light_ids[l]);
            self.out.empty("instance_light", &[("url", &url)]);
        }
    }
}

// ─── reader: sniffing ───────────────────────────────────────────────────────

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The local name of the document's root element, found by skipping the XML
/// prolog by hand: deciding "is this Collada at all" must not depend on the
/// whole file parsing, or a truncated Collada file would be misreported as
/// some other format.
fn root_name(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(SNIFF_BYTES)];
    let mut i = if head.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    loop {
        while head.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        let rest = head.get(i..)?;
        if rest.starts_with(b"<?") {
            i += find(rest, b"?>")? + 2;
        } else if rest.starts_with(b"<!--") {
            i += find(rest, b"-->")? + 3;
        } else if rest.starts_with(b"<!") {
            // DOCTYPE, whose internal subset may itself contain '>'.
            let mut depth = 0usize;
            let mut j = 2;
            loop {
                match *rest.get(j)? {
                    b'[' => depth += 1,
                    b']' => depth = depth.saturating_sub(1),
                    b'>' if depth == 0 => break,
                    _ => {}
                }
                j += 1;
            }
            i += j + 1;
        } else if rest.first() == Some(&b'<') {
            let name: Vec<u8> = rest[1..]
                .iter()
                .take_while(|b| !b.is_ascii_whitespace() && **b != b'/' && **b != b'>')
                .take(256)
                .copied()
                .collect();
            let name = String::from_utf8(name).ok()?;
            return Some(name.rsplit(':').next().unwrap_or("").to_string());
        } else {
            return None;
        }
    }
}

fn sniff(bytes: &[u8]) -> Result<(), MeshError> {
    if bytes.starts_with(&[0xFE, 0xFF]) || bytes.starts_with(&[0xFF, 0xFE]) {
        return Err(not_dae("the bytes are UTF-16 text; COLLADA documents are read as UTF-8"));
    }
    match root_name(bytes) {
        Some(n) if n == "COLLADA" => Ok(()),
        Some(n) => Err(not_dae(format!("the XML root element is <{}>, not <COLLADA>", clip(&n, 60)))),
        None => Err(not_dae("not an XML document whose root element is <COLLADA>")),
    }
}

// ─── reader: document index ─────────────────────────────────────────────────

struct Doc<'a> {
    root: &'a Element,
    ids: HashMap<&'a str, &'a Element>,
    duplicate_ids: usize,
}

impl<'a> Doc<'a> {
    fn new(root: &'a Element) -> Doc<'a> {
        let mut ids = HashMap::new();
        let mut duplicate_ids = 0;
        let mut stack = vec![root];
        while let Some(e) = stack.pop() {
            if let Some(id) = e.attr("id") {
                // XML ids are unique; when a file repeats one, the first in
                // document order is the one a reader resolving `#id` meets.
                if ids.contains_key(id) {
                    duplicate_ids += 1;
                } else {
                    ids.insert(id, e);
                }
            }
            stack.extend(e.children.iter().rev());
        }
        Doc { root, ids, duplicate_ids }
    }

    /// The element a `#id` URL names. References into other documents
    /// (`other.dae#id`) are not followed: conversion has no filesystem.
    fn get(&self, url: &str) -> Option<&'a Element> {
        let id = url.trim().strip_prefix('#')?;
        self.ids.get(id).copied()
    }

    fn by_id(&self, id: &str) -> Option<&'a Element> {
        self.ids.get(id).copied()
    }

    /// Every `item` of every `<library_...>` called `lib`.
    fn library(&self, lib: &str, item: &str) -> Vec<&'a Element> {
        kids(self.root, lib).flat_map(|l| kids(l, item).collect::<Vec<_>>()).collect()
    }

    fn resolve(&self, url: &str, expect: &str, at: &str) -> Result<&'a Element, MeshError> {
        let e = match self.get(url) {
            Some(e) => e,
            None if !url.trim().starts_with('#') => {
                return Err(unsupported(format!(
                    "{at} refers to {}, outside this document; only #id references within the file are followed",
                    q(url)
                )))
            }
            None => return Err(malformed(format!("{at} refers to {}, which is not in the document", q(url)))),
        };
        if e.name != expect {
            return Err(malformed(format!("{at} refers to {}, a <{}> where a <{expect}> belongs", q(url), clip(&e.name, 40))));
        }
        Ok(e)
    }
}

// ─── reader: sources and accessors ──────────────────────────────────────────

/// An accessor resolved and checked: element `i`'s component `c` is
/// `array[offset + i * stride + comps[c]]`, for every `i < count`, proven in
/// range by [`View::check`] before anything reads it.
struct View<'a> {
    source_id: &'a str,
    array: &'a Element,
    count: usize,
    offset: usize,
    stride: usize,
    /// Positions within an element of the NAMED params, expanded by width; an
    /// unnamed `<param>` is, per the spec, a value to skip.
    comps: Vec<usize>,
}

impl View<'_> {
    fn check(&self, len: usize) -> Result<(), MeshError> {
        if self.count == 0 {
            return Ok(());
        }
        let last = (self.count - 1)
            .checked_mul(self.stride)
            .and_then(|x| x.checked_add(self.offset))
            .and_then(|x| x.checked_add(self.comps.iter().copied().max().unwrap_or(0)));
        match last {
            Some(l) if l < len => Ok(()),
            _ => Err(malformed(format!(
                "source {} describes {} elements of stride {} from offset {}, past its {len}-value array",
                q(self.source_id),
                self.count,
                self.stride,
                self.offset
            ))),
        }
    }

    fn at(&self, i: usize, c: usize) -> usize {
        self.offset + i * self.stride + self.comps[c]
    }
}

fn param_width(ty: &str) -> usize {
    match ty {
        "float4x4" => 16,
        "float3x3" => 9,
        "float4" | "float2x2" => 4,
        "float3" => 3,
        "float2" => 2,
        _ => 1,
    }
}

fn view_of<'a>(doc: &Doc<'a>, source: &'a Element) -> Result<View<'a>, MeshError> {
    let sid = source.attr("id").unwrap_or("");
    if source.name != "source" {
        return Err(malformed(format!("{} is a <{}>, not a <source>", q(sid), clip(&source.name, 40))));
    }
    let acc = kid(source, "technique_common")
        .and_then(|t| kid(t, "accessor"))
        .ok_or_else(|| malformed(format!("source {} has no technique_common accessor", q(sid))))?;
    let at = format!("source {} accessor", q(sid));
    let count = attr_count(acc, "count", &at)?.ok_or_else(|| malformed(format!("{at} has no count")))?;
    let offset = attr_count(acc, "offset", &at)?.unwrap_or(0);
    let stride = attr_count(acc, "stride", &at)?.unwrap_or(1);
    if stride == 0 {
        return Err(malformed(format!("{at} has stride 0")));
    }
    let array = match acc.attr("source") {
        Some(url) => doc.get(url).ok_or_else(|| malformed(format!("{at} names {}, which is not in the document", q(url))))?,
        // The accessor's `source` is required; the array beside it is the
        // only thing it could have meant.
        None => source
            .children
            .iter()
            .find(|c| c.name.ends_with("_array"))
            .ok_or_else(|| malformed(format!("{at} names no array")))?,
    };
    if !matches!(array.name.as_str(), "float_array" | "int_array" | "Name_array" | "IDREF_array" | "bool_array") {
        return Err(malformed(format!("{at} names a <{}>, not an array", clip(&array.name, 40))));
    }
    let mut comps = Vec::new();
    let mut pos = 0usize;
    let params: Vec<&Element> = kids(acc, "param").collect();
    for p in &params {
        let width = param_width(p.attr("type").unwrap_or("float"));
        if p.attr("name").is_some_and(|n| !n.is_empty()) {
            comps.extend(pos..pos + width);
        }
        pos += width;
    }
    if params.is_empty() {
        // No params at all: the whole stride is the element.
        comps.extend(0..stride);
        pos = stride;
    }
    if pos > stride {
        return Err(malformed(format!("{at} params span {pos} values but the stride is {stride}")));
    }
    Ok(View { source_id: sid, array, count, offset, stride, comps })
}

// ─── reader: state ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct GeomPrim {
    symbol: Option<String>,
    positions: Vec<[f32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
    uvs: Vec<Vec<[f32; 2]>>,
    /// The Collada `set` of each entry of `uvs`.
    uv_sets: Vec<u32>,
    colors: Option<Vec<[f32; 4]>>,
    /// Each vertex's index into `<vertices>`, which is what skin weights and
    /// morph targets are indexed by.
    vertex: Vec<u32>,
    indices: Vec<u32>,
    /// What `indices` draws: triangles, or the segments of `<lines>` and
    /// `<linestrips>`.
    topology: Topology,
}

/// The elements of one primitive element, as numbers of its `<p>` corners:
/// triangles, or line segments.
enum Elements {
    Triangles(Vec<[usize; 3]>),
    Segments(Vec<[usize; 2]>),
}

impl Elements {
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn len(&self) -> usize {
        match self {
            Elements::Triangles(t) => t.len(),
            Elements::Segments(s) => s.len(),
        }
    }

    /// Corners to an element.
    fn corners_each(&self) -> usize {
        match self {
            Elements::Triangles(_) => 3,
            Elements::Segments(_) => 2,
        }
    }

    /// Every corner of every element, in order.
    fn corners(&self) -> Box<dyn Iterator<Item = usize> + '_> {
        match self {
            Elements::Triangles(t) => Box::new(t.iter().flatten().copied()),
            Elements::Segments(s) => Box::new(s.iter().flatten().copied()),
        }
    }
}

struct GeomData {
    name: Option<String>,
    prims: Vec<GeomPrim>,
    /// Elements of `<vertices>` (its POSITION source).
    vertex_count: usize,
    /// Maya's `<extra><technique profile="MAYA"><double_sided>` on the geometry.
    double_sided: bool,
}

struct SkinData<'a> {
    name: Option<String>,
    geometry: &'a Element,
    morph: Option<Rc<MorphData<'a>>>,
    joints: Vec<String>,
    /// The joint source is an `IDREF_array` (node ids) rather than sids.
    joints_are_ids: bool,
    /// INV_BIND_MATRIX × bind_shape_matrix, column-major: the scene's IBM.
    ibms: Vec<Mat4>,
    bind_shape: Mat4,
    /// Influences of `<vertices>` element `v`: `pairs[starts[v]..starts[v + 1]]`,
    /// joint `-1` meaning the bind shape itself.
    starts: Vec<usize>,
    pairs: Vec<(i64, f32)>,
}

struct MorphData<'a> {
    /// The `<controller>` holding the morph: what weight channels address.
    controller: usize,
    base: &'a Element,
    relative: bool,
    targets: Vec<&'a Element>,
    names: Vec<String>,
    weights: Vec<f32>,
}

enum Controller<'a> {
    Skin(Rc<SkinData<'a>>),
    Morph(Rc<MorphData<'a>>),
}

#[derive(Clone)]
enum TexSource {
    Color([f32; 4]),
    Texture { texture: usize, image: usize, texcoord: String },
}

#[derive(Clone, Copy, PartialEq)]
enum Shading {
    Constant,
    Lambert,
    Phong,
}

#[derive(Clone)]
struct EffectData {
    shading: Shading,
    emission: Option<TexSource>,
    diffuse: Option<TexSource>,
    transparent: Option<(String, TexSource)>,
    transparency: Option<f32>,
    shininess: Option<f32>,
    /// `<index_of_refraction>` as glTF's ior ([`Reader::index_of_refraction`]).
    ior: Option<f32>,
    double_sided: bool,
}

/// One `<instance_material>`: symbol, material, and the texcoord semantics it
/// binds to `TEXCOORD` sets.
type MaterialBinding<'a> = (&'a str, &'a Element, Vec<(String, u32)>);

/// A `<bind_material>`: material symbol → material, with the texcoord
/// bindings of each.
struct Binding<'a> {
    entries: Vec<MaterialBinding<'a>>,
    /// A canonical spelling, so two instances that bind identically share
    /// one mesh.
    signature: String,
}

enum PartSource<'a> {
    Geometry(&'a Element),
    Controller(&'a Element),
}

struct Part<'a> {
    source: PartSource<'a>,
    binding: Rc<Binding<'a>>,
    skeletons: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum OpKind {
    Matrix,
    Translate,
    Rotate,
    Scale,
    LookAt,
}

#[derive(Clone)]
struct Op {
    sid: Option<String>,
    kind: OpKind,
    /// As written: a matrix row-major, a rotate as axis then degrees.
    values: Vec<f64>,
}

struct NodeInfo<'a> {
    /// Shared by every `instance_node` copy of the same element: parsed once.
    ops: Rc<Vec<Op>>,
    parts: Vec<Part<'a>>,
    sid: Option<&'a str>,
    id: Option<&'a str>,
    name: Option<&'a str>,
}

struct Reader<'a> {
    doc: Doc<'a>,
    sidecars: &'a dyn Sidecars,
    scene: Scene,
    warnings: Vec<String>,
    warned: HashSet<String>,
    parsed: usize,
    total_vertices: usize,
    total_indices: usize,
    total_keys: usize,
    correction: Option<Mat4>,
    f32_cache: HashMap<usize, Rc<Vec<f32>>>,
    f64_cache: HashMap<usize, Rc<Vec<f64>>>,
    name_cache: HashMap<usize, Rc<Vec<String>>>,
    geom_cache: HashMap<usize, Rc<GeomData>>,
    controller_cache: HashMap<usize, Rc<Controller<'a>>>,
    effect_cache: HashMap<usize, Rc<EffectData>>,
    image_cache: HashMap<usize, Option<usize>>,
    texture_cache: HashMap<(usize, String), usize>,
    material_cache: HashMap<String, Option<usize>>,
    mesh_cache: HashMap<String, usize>,
    skin_cache: HashMap<String, usize>,
    /// `skin_for` by controller, skeleton roots and (when the bind shape is
    /// a joint) instancing node: the content key costs the whole skeleton,
    /// and a controller instanced many times was keyed afresh each time.
    instance_skins: HashMap<(usize, Vec<String>, Option<usize>), (usize, Option<usize>)>,
    /// Morph-target floats built so far (`scene::MAX_MORPH_VALUES`).
    morph_values: usize,
    /// Curves by sampler and width: channels sharing a sampler share its
    /// curve (one 200k-key sampler cost ~6 MB per extra `<channel>`).
    curve_cache: HashMap<(usize, usize), Rc<Curve>>,
    info: Vec<NodeInfo<'a>>,
    nodes_by_element: HashMap<usize, Vec<usize>>,
    /// Each `<node>` element's transform stack, parsed once however many
    /// `instance_node` copies re-enter it.
    ops_by_element: HashMap<usize, Rc<Vec<Op>>>,
    /// Morph controllers → the scene nodes that draw them, for weight channels.
    morph_nodes: HashMap<usize, Vec<usize>>,
    /// MORPH_WEIGHT `<source>`s and their arrays → (morph controller, target
    /// count), so weight channels resolve without a library scan each.
    weight_arrays: Option<HashMap<usize, (usize, usize)>>,
    /// Which nodes are roots (the unit/up-axis correction applies to them).
    is_root: Vec<bool>,
    /// Lookup tables for joint resolution, built on first use.
    global_sid: Option<HashMap<String, usize>>,
    global_id: Option<HashMap<String, usize>>,
    global_name: Option<HashMap<String, usize>>,
    subtree_sid: HashMap<Vec<usize>, Rc<HashMap<String, usize>>>,
    /// The document's `<unit>`, in metres: what a camera's clipping
    /// distances and magnifications, a light's range and the distance in its
    /// attenuation are measured in.
    meter: f64,
    /// Each node's first `<instance_light>` and first `<instance_camera>`,
    /// read once the node walk is done ([`Reader::lights_and_cameras`]): a
    /// node places one of each at most, so these hold at most a node each.
    instanced_lights: Vec<(usize, &'a Element)>,
    instanced_cameras: Vec<(usize, &'a Element)>,
}

impl<'a> Reader<'a> {
    fn new(doc: Doc<'a>, sidecars: &'a dyn Sidecars) -> Reader<'a> {
        Reader {
            doc,
            sidecars,
            scene: Scene::default(),
            warnings: Vec::new(),
            warned: HashSet::new(),
            parsed: 0,
            total_vertices: 0,
            total_indices: 0,
            total_keys: 0,
            correction: None,
            meter: 1.0,
            instanced_lights: Vec::new(),
            instanced_cameras: Vec::new(),
            f32_cache: HashMap::new(),
            f64_cache: HashMap::new(),
            name_cache: HashMap::new(),
            geom_cache: HashMap::new(),
            controller_cache: HashMap::new(),
            effect_cache: HashMap::new(),
            image_cache: HashMap::new(),
            texture_cache: HashMap::new(),
            material_cache: HashMap::new(),
            mesh_cache: HashMap::new(),
            skin_cache: HashMap::new(),
            instance_skins: HashMap::new(),
            morph_values: 0,
            curve_cache: HashMap::new(),
            info: Vec::new(),
            nodes_by_element: HashMap::new(),
            ops_by_element: HashMap::new(),
            morph_nodes: HashMap::new(),
            weight_arrays: None,
            is_root: Vec::new(),
            global_sid: None,
            global_id: None,
            global_name: None,
            subtree_sid: HashMap::new(),
        }
    }

    fn warn(&mut self, w: String) {
        self.warnings.push(clip(&w, MAX_DETAIL));
    }

    /// Warn once per `key`, so a property repeated on every primitive of a
    /// large file does not bury every other warning.
    fn warn_once(&mut self, key: &str, w: String) {
        if self.warned.insert(key.to_string()) {
            self.warn(w);
        }
    }

    // ── arrays ──────────────────────────────────────────────────────────────

    fn charge(&mut self, n: usize, what: &str) -> Result<(), MeshError> {
        self.parsed = self.parsed.saturating_add(n);
        if self.parsed > MAX_PARSED_VALUES {
            return Err(unsupported(format!(
                "parsing {what} takes the document past {MAX_PARSED_VALUES} numbers; it describes more data than \
                 this converter will hold"
            )));
        }
        Ok(())
    }

    /// Token count of an array element, checked against its `count`.
    fn tokens(&mut self, arr: &Element) -> Result<usize, MeshError> {
        let n = arr.text.split_ascii_whitespace().count();
        let id = arr.attr("id").unwrap_or("");
        if let Some(declared) = attr_count(arr, "count", &format!("<{}> {}", clip(&arr.name, 20), q(id)))? {
            if declared != n {
                return Err(malformed(format!(
                    "<{}> {} declares {declared} values and holds {n}",
                    clip(&arr.name, 20),
                    q(id)
                )));
            }
        }
        self.charge(n, &format!("array {}", q(id)))?;
        Ok(n)
    }

    fn floats_f32(&mut self, arr: &'a Element) -> Result<Rc<Vec<f32>>, MeshError> {
        if let Some(v) = self.f32_cache.get(&mat_ptr(arr)) {
            return Ok(v.clone());
        }
        if arr.name != "float_array" {
            return Err(malformed(format!("{} is a <{}>, not a float_array", q(arr.attr("id").unwrap_or("")), clip(&arr.name, 20))));
        }
        let n = self.tokens(arr)?;
        let mut out = Vec::with_capacity(n);
        for t in arr.text.split_ascii_whitespace() {
            out.push(
                t.parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| malformed(format!("float_array {} holds {}, not a finite number", q(arr.attr("id").unwrap_or("")), q(t))))?,
            );
        }
        let rc = Rc::new(out);
        self.f32_cache.insert(mat_ptr(arr), rc.clone());
        Ok(rc)
    }

    fn floats_f64(&mut self, arr: &'a Element) -> Result<Rc<Vec<f64>>, MeshError> {
        if let Some(v) = self.f64_cache.get(&mat_ptr(arr)) {
            return Ok(v.clone());
        }
        if arr.name != "float_array" {
            return Err(malformed(format!("{} is a <{}>, not a float_array", q(arr.attr("id").unwrap_or("")), clip(&arr.name, 20))));
        }
        let n = self.tokens(arr)?;
        let mut out = Vec::with_capacity(n);
        for t in arr.text.split_ascii_whitespace() {
            out.push(
                t.parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| malformed(format!("float_array {} holds {}, not a finite number", q(arr.attr("id").unwrap_or("")), q(t))))?,
            );
        }
        let rc = Rc::new(out);
        self.f64_cache.insert(mat_ptr(arr), rc.clone());
        Ok(rc)
    }

    fn names(&mut self, arr: &'a Element) -> Result<Rc<Vec<String>>, MeshError> {
        if let Some(v) = self.name_cache.get(&mat_ptr(arr)) {
            return Ok(v.clone());
        }
        if !matches!(arr.name.as_str(), "Name_array" | "IDREF_array") {
            return Err(malformed(format!("{} is a <{}>, not a Name_array", q(arr.attr("id").unwrap_or("")), clip(&arr.name, 20))));
        }
        self.tokens(arr)?;
        let rc = Rc::new(arr.text.split_ascii_whitespace().map(str::to_string).collect::<Vec<_>>());
        self.name_cache.insert(mat_ptr(arr), rc.clone());
        Ok(rc)
    }

    /// Non-negative integers (`<p>`, `<vcount>`).
    fn indices(&mut self, e: &Element, what: &str) -> Result<Vec<u32>, MeshError> {
        let n = e.text.split_ascii_whitespace().count();
        self.charge(n, what)?;
        let mut out = Vec::with_capacity(n);
        for t in e.text.split_ascii_whitespace() {
            out.push(t.parse::<u32>().map_err(|_| malformed(format!("{what} holds {}, not an index", q(t))))?);
        }
        Ok(out)
    }

    /// Signed integers (`<v>`, where -1 names the bind shape).
    fn signed(&mut self, e: &Element, what: &str) -> Result<Vec<i64>, MeshError> {
        let n = e.text.split_ascii_whitespace().count();
        self.charge(n, what)?;
        let mut out = Vec::with_capacity(n);
        for t in e.text.split_ascii_whitespace() {
            out.push(t.parse::<i64>().map_err(|_| malformed(format!("{what} holds {}, not an integer", q(t))))?);
        }
        Ok(out)
    }

    /// A float source, its accessor proven against its array, with at least
    /// `need` named components.
    fn stream_f32(&mut self, source: &'a Element, need: usize, what: &str) -> Result<(Rc<Vec<f32>>, View<'a>), MeshError> {
        let v = view_of(&self.doc, source)?;
        if v.comps.len() < need {
            return Err(malformed(format!("{what} source {} has {} components, {need} needed", q(v.source_id), v.comps.len())));
        }
        let vals = self.floats_f32(v.array)?;
        v.check(vals.len())?;
        Ok((vals, v))
    }

    fn stream_f64(&mut self, source: &'a Element, need: usize, what: &str) -> Result<(Rc<Vec<f64>>, View<'a>), MeshError> {
        let v = view_of(&self.doc, source)?;
        if v.comps.len() < need {
            return Err(malformed(format!("{what} source {} has {} components, {need} needed", q(v.source_id), v.comps.len())));
        }
        let vals = self.floats_f64(v.array)?;
        v.check(vals.len())?;
        Ok((vals, v))
    }

    fn stream_names(&mut self, source: &'a Element, what: &str) -> Result<(Rc<Vec<String>>, View<'a>), MeshError> {
        let v = view_of(&self.doc, source)?;
        if v.comps.is_empty() {
            return Err(malformed(format!("{what} source {} has no components", q(v.source_id))));
        }
        let vals = self.names(v.array)?;
        v.check(vals.len())?;
        Ok((vals, v))
    }

    // ── geometry ────────────────────────────────────────────────────────────

    fn geometry(&mut self, geom: &'a Element) -> Result<Rc<GeomData>, MeshError> {
        if let Some(g) = self.geom_cache.get(&mat_ptr(geom)) {
            return Ok(g.clone());
        }
        let gid = geom.attr("id").unwrap_or("");
        let mut data = GeomData {
            name: geom.attr("name").filter(|s| !s.is_empty()).map(str::to_string),
            prims: Vec::new(),
            vertex_count: 0,
            double_sided: double_sided_extra(geom),
        };
        match kid(geom, "mesh") {
            None => {
                let kind = geom.children.iter().find(|c| c.name != "asset" && c.name != "extra").map_or("nothing", |c| c.name.as_str());
                self.warn(format!("geometry {} is a <{}>, not a polygon mesh; it was skipped", q(gid), clip(kind, 30)));
            }
            Some(mesh) => {
                let vertices = kid(mesh, "vertices");
                if let Some(v) = vertices {
                    data.vertex_count = self.vertex_count(gid, v)?;
                }
                for prim in &mesh.children {
                    let kind = prim.name.as_str();
                    match kind {
                        "triangles" | "polylist" | "polygons" | "trifans" | "tristrips" | "lines" | "linestrips" => {}
                        _ => continue,
                    }
                    let vertices = vertices
                        .ok_or_else(|| malformed(format!("geometry {} has primitives but no <vertices>", q(gid))))?;
                    if let Some(p) = self.primitive(gid, prim, vertices)? {
                        data.prims.push(p);
                    }
                }
            }
        }
        let rc = Rc::new(data);
        self.geom_cache.insert(mat_ptr(geom), rc.clone());
        Ok(rc)
    }

    /// Elements of a `<vertices>`: the count of its POSITION source.
    fn vertex_count(&mut self, gid: &str, vertices: &'a Element) -> Result<usize, MeshError> {
        let pos = kids(vertices, "input")
            .find(|i| i.attr("semantic") == Some("POSITION"))
            .ok_or_else(|| malformed(format!("geometry {} <vertices> has no POSITION input", q(gid))))?;
        let url = pos.attr("source").unwrap_or("");
        let src = self.doc.resolve(url, "source", &format!("geometry {} POSITION", q(gid)))?;
        let (_, v) = self.stream_f32(src, 3, "POSITION")?;
        Ok(v.count)
    }

    /// One geometric primitive (`<triangles>`, `<trifans>`, `<tristrips>`,
    /// `<polygons>` with its holes, `<polylist>` with `<vcount>`) as triangles.
    /// COLLADA 1.4.1 (https://www.khronos.org/files/collada_spec_1_4.pdf), 5-126, 5-128, 5-130, 5-96 and 5-99; each `<input>`'s
    /// `offset` is its slot in a `<p>` tuple, inputs sharing an offset share an
    /// index, and `set` groups inputs of one semantic (5-39 to 5-41).
    fn primitive(&mut self, gid: &str, prim: &'a Element, vertices: &'a Element) -> Result<Option<GeomPrim>, MeshError> {
        let kind = prim.name.as_str();
        let at = format!("geometry {} <{kind}>", q(gid));
        struct In<'a> {
            semantic: &'a str,
            source: &'a Element,
            offset: usize,
            set: u32,
            in_vertices: bool,
        }
        let mut ins: Vec<In<'a>> = Vec::new();
        let mut stride = 0usize;
        for inp in kids(prim, "input") {
            let semantic = inp.attr("semantic").ok_or_else(|| malformed(format!("{at} has an <input> without a semantic")))?;
            let offset = attr_count(inp, "offset", &at)?
                .ok_or_else(|| malformed(format!("{at} input {semantic} has no offset")))?;
            if offset > MAX_INPUT_OFFSET {
                return Err(malformed(format!("{at} input {semantic} has offset {offset}")));
            }
            stride = stride.max(offset + 1);
            let set = match inp.attr("set") {
                None => 0,
                Some(s) => s.trim().parse::<u32>().map_err(|_| malformed(format!("{at} input {semantic} set {} is not a number", q(s))))?,
            };
            let url = inp.attr("source").ok_or_else(|| malformed(format!("{at} input {semantic} has no source")))?;
            if semantic == "VERTEX" {
                let target = self.doc.get(url).ok_or_else(|| malformed(format!("{at} VERTEX names {}, which is not in the document", q(url))))?;
                if !std::ptr::eq(target, vertices) {
                    return Err(malformed(format!("{at} VERTEX names {}, not its mesh's <vertices>", q(url))));
                }
                for vi in kids(vertices, "input") {
                    let vsem = vi.attr("semantic").ok_or_else(|| malformed(format!("{at} <vertices> input without a semantic")))?;
                    let vurl = vi.attr("source").unwrap_or("");
                    let src = self.doc.resolve(vurl, "source", &format!("{at} <vertices> {vsem}"))?;
                    ins.push(In { semantic: vsem, source: src, offset, set: 0, in_vertices: true });
                }
            } else {
                let src = self.doc.resolve(url, "source", &format!("{at} input {semantic}"))?;
                ins.push(In { semantic, source: src, offset, set, in_vertices: false });
            }
        }
        if stride == 0 {
            return Err(malformed(format!("{at} has no inputs")));
        }
        let pos = ins
            .iter()
            .find(|i| i.semantic == "POSITION")
            .ok_or_else(|| malformed(format!("{at} has no POSITION (VERTEX) input")))?;
        if !pos.in_vertices {
            return Err(malformed(format!("{at} takes POSITION outside <vertices>, where skins and morphs cannot index it")));
        }
        let nrm = ins.iter().find(|i| i.semantic == "NORMAL");
        let mut tex: Vec<&In> = Vec::new();
        for i in ins.iter().filter(|i| i.semantic == "TEXCOORD") {
            if !tex.iter().any(|t| t.set == i.set) {
                tex.push(i);
            }
        }
        tex.sort_by_key(|t| t.set);
        if tex.len() > MAX_READ_UV_SETS {
            self.warn_once("uvsets", format!("{at} has {} texture coordinate sets; the first {MAX_READ_UV_SETS} were kept", tex.len()));
            tex.truncate(MAX_READ_UV_SETS);
        }
        let col = ins.iter().filter(|i| i.semantic == "COLOR").min_by_key(|i| i.set);
        for i in &ins {
            if !matches!(i.semantic, "POSITION" | "NORMAL" | "TEXCOORD" | "COLOR") {
                self.warn_once(
                    &format!("input {}", i.semantic),
                    format!("vertex input {} is not carried (it is derivable or not modelled)", q(i.semantic)),
                );
            }
        }

        // Streams, each proven in range of its array.
        let (pv, pview) = self.stream_f32(pos.source, 3, "POSITION")?;
        let nrm_s = match nrm {
            Some(n) => Some((n.offset, self.stream_f32(n.source, 3, "NORMAL")?)),
            None => None,
        };
        let mut tex_s = Vec::with_capacity(tex.len());
        for t in &tex {
            tex_s.push((t.offset, t.set, self.stream_f64(t.source, 2, "TEXCOORD")?));
        }
        let col_s = match col {
            Some(c) => Some((c.offset, self.stream_f32(c.source, 3, "COLOR")?)),
            None => None,
        };

        // Corners and their triangulation, or the segments of a line element.
        let topology = if matches!(kind, "lines" | "linestrips") { Topology::Lines } else { Topology::Triangles };
        let (p, elements) = if topology == Topology::Lines {
            let (p, segments) = self.line_corners(prim, &at, stride)?;
            (p, Elements::Segments(segments))
        } else {
            let (p, tris) = self.corners(prim, &at, stride, pos.offset, &pv, &pview)?;
            (p, Elements::Triangles(tris))
        };
        if elements.is_empty() {
            return Ok(None);
        }

        // One scene vertex per distinct combination of indices.
        let corners = elements.len().saturating_mul(elements.corners_each());
        if corners > MAX_SCENE_INDICES {
            let what = if topology == Topology::Lines { "line segments" } else { "triangles" };
            return Err(unsupported(format!("{at} has {} {what}, over the scene's limit", elements.len())));
        }
        let mut out = GeomPrim {
            topology,
            symbol: prim.attr("material").map(str::to_string),
            positions: Vec::new(),
            normals: nrm_s.as_ref().map(|_| Vec::new()),
            uvs: vec![Vec::new(); tex_s.len()],
            uv_sets: tex_s.iter().map(|(_, set, _)| *set).collect(),
            colors: col_s.as_ref().map(|_| Vec::new()),
            vertex: Vec::new(),
            indices: Vec::with_capacity(corners),
        };
        // Distinct index tuples, then numbered in tuple order: a file whose
        // inputs share one index (this writer's, and most) gets its vertices
        // back in their own order, and any file gets a deterministic one.
        let mut map: HashMap<[u32; 12], u32> = HashMap::new();
        let mut uniques: Vec<[u32; 12]> = Vec::new();
        let mut corner_ids: Vec<u32> = Vec::with_capacity(corners);
        for c in elements.corners() {
            let base = c * stride;
            let mut key = [0u32; 12];
            key[0] = p[base + pos.offset];
            let mut k = 1;
            if let Some((off, _)) = &nrm_s {
                key[k] = p[base + off];
                k += 1;
            }
            for (off, _, _) in &tex_s {
                key[k] = p[base + off];
                k += 1;
            }
            if let Some((off, _)) = &col_s {
                key[k] = p[base + off];
            }
            let id = match map.get(&key) {
                Some(&id) => id,
                None => {
                    let id = u32::try_from(uniques.len()).map_err(|_| unsupported(format!("{at} has too many vertices")))?;
                    uniques.push(key);
                    map.insert(key, id);
                    id
                }
            };
            corner_ids.push(id);
        }
        drop(map);
        let mut order: Vec<u32> = (0..uniques.len() as u32).collect();
        order.sort_unstable_by_key(|&i| uniques[i as usize]);
        let mut remap = vec![0u32; uniques.len()];
        for (new, &old) in order.iter().enumerate() {
            remap[old as usize] = new as u32;
        }
        for &old in &order {
            let key = uniques[old as usize];
            let pi = key[0] as usize;
            if pi >= pview.count {
                return Err(malformed(format!("{at} position index {pi} is out of range for {} positions", pview.count)));
            }
            out.positions.push([pv[pview.at(pi, 0)], pv[pview.at(pi, 1)], pv[pview.at(pi, 2)]]);
            out.vertex.push(key[0]);
            let mut k = 1;
            if let (Some((_, (nv, nview))), Some(normals)) = (&nrm_s, out.normals.as_mut()) {
                let ni = key[k] as usize;
                k += 1;
                if ni >= nview.count {
                    return Err(malformed(format!("{at} normal index {ni} is out of range for {} normals", nview.count)));
                }
                normals.push([nv[nview.at(ni, 0)], nv[nview.at(ni, 1)], nv[nview.at(ni, 2)]]);
            }
            for (si, (_, _, (tv, tview))) in tex_s.iter().enumerate() {
                let ti = key[k] as usize;
                k += 1;
                if ti >= tview.count {
                    return Err(malformed(format!("{at} texcoord index {ti} is out of range for {} coordinates", tview.count)));
                }
                // Collada's V runs up the image, the scene's down. The flip is
                // done in f64 so `1 - (1 - v)` gives v back exactly when this
                // writer's file is read.
                out.uvs[si].push([tv[tview.at(ti, 0)] as f32, (1.0 - tv[tview.at(ti, 1)]) as f32]);
            }
            if let (Some((_, (cv, cview))), Some(colors)) = (&col_s, out.colors.as_mut()) {
                let ci = key[k] as usize;
                if ci >= cview.count {
                    return Err(malformed(format!("{at} colour index {ci} is out of range for {} colours", cview.count)));
                }
                let mut c = [cv[cview.at(ci, 0)], cv[cview.at(ci, 1)], cv[cview.at(ci, 2)], 1.0];
                if cview.comps.len() >= 4 {
                    c[3] = cv[cview.at(ci, 3)];
                }
                // Linear, wherever it sits (see the module docs).
                colors.push(c);
            }
        }
        out.indices = corner_ids.into_iter().map(|id| remap[id as usize]).collect();
        Ok(Some(out))
    }

    /// The `<p>` index list of a primitive element (all of its `<p>`s joined)
    /// and its triangles as corner numbers, triangulated by kind.
    #[allow(clippy::type_complexity)]
    fn corners(
        &mut self,
        prim: &'a Element,
        at: &str,
        stride: usize,
        pos_offset: usize,
        pv: &[f32],
        pview: &View<'a>,
    ) -> Result<(Vec<u32>, Vec<[usize; 3]>), MeshError> {
        let kind = prim.name.as_str();
        let declared = attr_count(prim, "count", at)?;
        let mut tris: Vec<[usize; 3]> = Vec::new();
        let mut p: Vec<u32> = Vec::new();
        // Polygons as (first corner, corner count); strips and fans by kind.
        let mut polys: Vec<(usize, usize)> = Vec::new();
        match kind {
            "triangles" | "polylist" => {
                if let Some(pe) = kid(prim, "p") {
                    p = self.indices(pe, &format!("{at} <p>"))?;
                }
                if !p.len().is_multiple_of(stride) {
                    return Err(malformed(format!("{at} <p> holds {} indices, not a multiple of its {stride} inputs", p.len())));
                }
                let n_corners = p.len() / stride;
                if kind == "triangles" {
                    let count = declared.unwrap_or(n_corners / 3);
                    if count.checked_mul(3) != Some(n_corners) {
                        return Err(malformed(format!("{at} declares {count} triangles and holds {n_corners} corners")));
                    }
                    tris.reserve(count);
                    for t in 0..count {
                        tris.push([3 * t, 3 * t + 1, 3 * t + 2]);
                    }
                    return Ok((p, tris));
                }
                let vcount = match kid(prim, "vcount") {
                    Some(v) => self.indices(v, &format!("{at} <vcount>"))?,
                    None => Vec::new(),
                };
                if let Some(c) = declared {
                    if c != vcount.len() {
                        return Err(malformed(format!("{at} declares {c} polygons and its <vcount> lists {}", vcount.len())));
                    }
                }
                let total: usize = vcount.iter().map(|v| *v as usize).fold(0usize, |a, b| a.saturating_add(b));
                if total != n_corners {
                    return Err(malformed(format!("{at} <vcount> sums to {total} corners and <p> holds {n_corners}")));
                }
                let mut start = 0;
                for n in vcount {
                    polys.push((start, n as usize));
                    start += n as usize;
                }
            }
            "polygons" | "trifans" | "tristrips" => {
                if kid(prim, "ph").is_some() {
                    return Err(unsupported(format!("{at} has polygons with holes (<ph>), which this converter does not triangulate")));
                }
                let ps: Vec<&Element> = kids(prim, "p").collect();
                if let Some(c) = declared {
                    if c != ps.len() {
                        return Err(malformed(format!("{at} declares {c} and holds {} <p> elements", ps.len())));
                    }
                }
                for pe in ps {
                    let one = self.indices(pe, &format!("{at} <p>"))?;
                    if !one.len().is_multiple_of(stride) {
                        return Err(malformed(format!("{at} has a <p> of {} indices, not a multiple of its {stride} inputs", one.len())));
                    }
                    polys.push((p.len() / stride, one.len() / stride));
                    p.extend(one);
                }
            }
            _ => return Ok((p, tris)),
        }
        let position_of = |c: usize| -> Option<[f64; 3]> {
            let i = *p.get(c * stride + pos_offset)? as usize;
            if i >= pview.count {
                return None;
            }
            Some([pv[pview.at(i, 0)] as f64, pv[pview.at(i, 1)] as f64, pv[pview.at(i, 2)] as f64])
        };
        let mut degenerate = 0usize;
        let mut fanned_concave = 0usize;
        for (start, n) in polys {
            if n < 3 {
                degenerate += 1;
                continue;
            }
            match kind {
                "tristrips" => {
                    for k in 0..n - 2 {
                        let (a, b, c) = (start + k, start + k + 1, start + k + 2);
                        tris.push(if k % 2 == 0 { [a, b, c] } else { [b, a, c] });
                    }
                }
                "trifans" => {
                    for k in 1..n - 1 {
                        tris.push([start, start + k, start + k + 1]);
                    }
                }
                _ if n == 3 => tris.push([start, start + 1, start + 2]),
                _ => {
                    let pts: Option<Vec<[f64; 3]>> = (start..start + n).map(position_of).collect();
                    let Some(pts) = pts else {
                        return Err(malformed(format!("{at} has a polygon whose position index is out of range")));
                    };
                    match triangulate(&pts) {
                        Triangulation::Fan => {
                            for k in 1..n - 1 {
                                tris.push([start, start + k, start + k + 1]);
                            }
                        }
                        Triangulation::Ears(ears) => {
                            for t in ears {
                                tris.push([start + t[0], start + t[1], start + t[2]]);
                            }
                        }
                        Triangulation::TooLarge => {
                            fanned_concave += 1;
                            for k in 1..n - 1 {
                                tris.push([start, start + k, start + k + 1]);
                            }
                        }
                    }
                }
            }
            if tris.len() > MAX_SCENE_INDICES / 3 {
                return Err(unsupported(format!("{at} triangulates past the scene's index limit")));
            }
        }
        if degenerate > 0 {
            self.warn(format!("{at} has {degenerate} polygon(s) of fewer than 3 corners; they were skipped"));
        }
        if fanned_concave > 0 {
            self.warn(format!(
                "{at} has {fanned_concave} concave polygon(s) of more than {MAX_EAR_CLIP} corners; they were fan-triangulated"
            ));
        }
        Ok((p, tris))
    }

    /// The index list of a `<lines>` or `<linestrips>` and its segments as
    /// corner numbers. COLLADA 1.4.1
    /// (https://www.khronos.org/files/collada_spec_1_4.pdf), 5-72 and 5-74:
    /// a `<lines>` has one `<p>`, a pair of corners a segment, `count` of
    /// them, and only that first `<p>` is read; a `<linestrips>` is a `<p>` a
    /// strip, `count` of them (the lists joined in order), and a strip of `c`
    /// corners is `c - 1` segments. A strip of fewer than 2
    /// corners has none, and is skipped, counted and said once for the element.
    /// The segments are counted from the `<p>`s' token counts and charged to
    /// the scene's index limit before any index is parsed or segment made.
    fn line_corners(&mut self, prim: &'a Element, at: &str, stride: usize) -> Result<(Vec<u32>, Vec<[usize; 2]>), MeshError> {
        let kind = prim.name.as_str();
        let declared = attr_count(prim, "count", at)?;
        // Each `<p>`, and the corners it lists (not indices yet).
        let ps: Vec<&Element> = if kind == "lines" { kid(prim, "p").into_iter().collect() } else { kids(prim, "p").collect() };
        let mut sizes: Vec<usize> = Vec::with_capacity(ps.len());
        for pe in &ps {
            let tokens = pe.text.split_ascii_whitespace().count();
            if !tokens.is_multiple_of(stride) {
                return Err(malformed(format!("{at} has a <p> of {tokens} indices, not a multiple of its {stride} inputs")));
            }
            sizes.push(tokens / stride);
        }
        let segments = if kind == "lines" {
            let corners = sizes.first().copied().unwrap_or(0);
            let count = declared.unwrap_or(corners / 2);
            if count.checked_mul(2) != Some(corners) {
                return Err(malformed(format!("{at} declares {count} lines and holds {corners} corners")));
            }
            count
        } else {
            if let Some(c) = declared {
                if c != ps.len() {
                    return Err(malformed(format!("{at} declares {c} strips and holds {} <p> elements", ps.len())));
                }
            }
            sizes.iter().fold(0usize, |sum, &c| sum.saturating_add(c.saturating_sub(1)))
        };
        if segments.saturating_mul(2) > MAX_SCENE_INDICES {
            return Err(unsupported(format!("{at} has {segments} line segments, over the scene's limit")));
        }
        let mut p: Vec<u32> = Vec::new();
        for pe in ps {
            p.extend(self.indices(pe, &format!("{at} <p>"))?);
        }
        let mut out: Vec<[usize; 2]> = Vec::with_capacity(segments);
        if kind == "lines" {
            out.extend((0..segments).map(|i| [2 * i, 2 * i + 1]));
            return Ok((p, out));
        }
        let (mut start, mut degenerate) = (0usize, 0usize);
        for n in sizes {
            if n < 2 {
                degenerate += 1;
            } else {
                out.extend((0..n - 1).map(|k| [start + k, start + k + 1]));
            }
            start += n;
        }
        if degenerate > 0 {
            self.warn(format!("{at} has {degenerate} line strip(s) of fewer than 2 corners; they were skipped"));
        }
        Ok((p, out))
    }

    // ── materials ───────────────────────────────────────────────────────────

    fn binding(&mut self, inst: &'a Element) -> Result<Binding<'a>, MeshError> {
        let mut entries = Vec::new();
        for bm in kids(inst, "bind_material") {
            for tc in kids(bm, "technique_common") {
                for im in kids(tc, "instance_material") {
                    let symbol = im.attr("symbol").ok_or_else(|| malformed("an <instance_material> has no symbol"))?;
                    let target = im.attr("target").ok_or_else(|| malformed(format!("instance_material {} has no target", q(symbol))))?;
                    let mat = self.doc.resolve(target, "material", &format!("instance_material {}", q(symbol)))?;
                    let mut inputs = Vec::new();
                    for b in kids(im, "bind_vertex_input") {
                        let (Some(sem), Some(isem)) = (b.attr("semantic"), b.attr("input_semantic")) else { continue };
                        if isem != "TEXCOORD" {
                            continue;
                        }
                        let set = match b.attr("input_set") {
                            None => 0,
                            Some(s) => s.trim().parse::<u32>().map_err(|_| malformed(format!("bind_vertex_input input_set {} is not a number", q(s))))?,
                        };
                        inputs.push((sem.to_string(), set));
                    }
                    entries.push((symbol, mat, inputs));
                }
            }
        }
        let mut signature = String::new();
        for (symbol, mat, inputs) in &entries {
            let _ = write!(signature, "{symbol}\u{1}{:x}", mat_ptr(mat));
            for (s, set) in inputs {
                let _ = write!(signature, "\u{1}{s}={set}");
            }
            signature.push('\u{2}');
        }
        Ok(Binding { entries, signature })
    }

    fn effect(&mut self, effect: &'a Element) -> Result<Rc<EffectData>, MeshError> {
        if let Some(e) = self.effect_cache.get(&mat_ptr(effect)) {
            return Ok(e.clone());
        }
        let eid = effect.attr("id").unwrap_or("");
        let mut data = EffectData {
            shading: Shading::Lambert,
            emission: None,
            diffuse: None,
            transparent: None,
            transparency: None,
            shininess: None,
            ior: None,
            double_sided: double_sided_extra(effect),
        };
        let Some(profile) = kid(effect, "profile_COMMON") else {
            self.warn(format!("effect {} has no profile_COMMON (only shader profiles); it reads as a white material", q(eid)));
            let rc = Rc::new(data);
            self.effect_cache.insert(mat_ptr(effect), rc.clone());
            return Ok(rc);
        };
        data.double_sided |= double_sided_extra(profile);
        let technique = kid(profile, "technique").ok_or_else(|| malformed(format!("effect {} profile_COMMON has no technique", q(eid))))?;
        // newparams may sit on the effect, the profile or its technique (the
        // 1.4.1 schema's profile_COMMON/technique); the innermost declaration
        // of a sid is the one in scope.
        let mut params: HashMap<&'a str, &'a Element> = HashMap::new();
        for np in kids(technique, "newparam").chain(kids(profile, "newparam")).chain(kids(effect, "newparam")) {
            if let Some(sid) = np.attr("sid") {
                params.entry(sid).or_insert(np);
            }
        }
        data.double_sided |= double_sided_extra(technique);
        let Some(shader) = technique.children.iter().find(|c| matches!(c.name.as_str(), "constant" | "lambert" | "phong" | "blinn")) else {
            self.warn(format!("effect {} has no constant/lambert/phong/blinn shader; it reads as white", q(eid)));
            let rc = Rc::new(data);
            self.effect_cache.insert(mat_ptr(effect), rc.clone());
            return Ok(rc);
        };
        data.shading = match shader.name.as_str() {
            "constant" => Shading::Constant,
            "lambert" => Shading::Lambert,
            _ => Shading::Phong,
        };
        for c in &shader.children {
            let at = format!("effect {} <{}>", q(eid), clip(&c.name, 30));
            match c.name.as_str() {
                "emission" => data.emission = self.color_or_texture(c, &params, &at)?,
                "diffuse" => data.diffuse = self.color_or_texture(c, &params, &at)?,
                "transparent" => {
                    let mode = c.attr("opaque").unwrap_or("A_ONE").to_string();
                    if let Some(v) = self.color_or_texture(c, &params, &at)? {
                        data.transparent = Some((mode, v));
                    }
                }
                "transparency" => data.transparency = self.float_param(c, &params, &at)?,
                "shininess" => data.shininess = self.float_param(c, &params, &at)?,
                "index_of_refraction" => data.ior = self.index_of_refraction(c, &params, &at),
                "bump" => self.warn_once(
                    "bump",
                    format!("{at}: bump maps are not carried (Collada does not say whether one is a height or a normal map)"),
                ),
                _ => {}
            }
        }
        for e in kids(technique, "extra").chain(kids(profile, "extra")) {
            if !e.descendants_named("bump").is_empty() {
                self.warn_once(
                    "bump",
                    format!("effect {}: bump maps are not carried (Collada does not say whether one is a height or a normal map)", q(eid)),
                );
            }
        }
        let rc = Rc::new(data);
        self.effect_cache.insert(mat_ptr(effect), rc.clone());
        Ok(rc)
    }

    fn color_or_texture(
        &mut self,
        e: &'a Element,
        params: &HashMap<&'a str, &'a Element>,
        at: &str,
    ) -> Result<Option<TexSource>, MeshError> {
        for c in &e.children {
            match c.name.as_str() {
                "color" => return Ok(Some(TexSource::Color(color4(&c.text, at)?))),
                "param" => {
                    let Some(np) = c.attr("ref").and_then(|r| params.get(r)) else {
                        self.warn(format!("{at} names a <param> that is not declared; it was ignored"));
                        return Ok(None);
                    };
                    if let Some(f4) = kid(np, "float4").or_else(|| kid(np, "float3")) {
                        return Ok(Some(TexSource::Color(color4(&f4.text, at)?)));
                    }
                    self.warn(format!("{at} names a <param> that is not a colour; it was ignored"));
                    return Ok(None);
                }
                "texture" => return self.texture(c, params, at),
                _ => {}
            }
        }
        Ok(None)
    }

    fn float_param(&mut self, e: &'a Element, params: &HashMap<&'a str, &'a Element>, at: &str) -> Result<Option<f32>, MeshError> {
        for c in &e.children {
            let text = match c.name.as_str() {
                "float" => &c.text,
                "param" => match c.attr("ref").and_then(|r| params.get(r)).and_then(|np| kid(np, "float")) {
                    Some(f) => &f.text,
                    None => continue,
                },
                _ => continue,
            };
            let v = text
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| malformed(format!("{at} holds {}, not a number", q(text))))?;
            return Ok(Some(v));
        }
        Ok(None)
    }

    /// `<index_of_refraction>` as glTF's ior (KHR_materials_ior), a float or
    /// the float of the `<newparam>` it names. An exporter's default is unset
    /// ([`DEFAULT_IORS`]). One of 0 or below is no refractive index, the
    /// element's meaning, and is unset too, where KHR's 0 would be a perfect
    /// mirror (Ruling A42), as OBJ's `Ni 0` is. That, a value
    /// KHR_materials_ior can't hold, one that is no number, or a `<param>`
    /// naming no float is left out with a warning, and the file still
    /// converts.
    fn index_of_refraction(&mut self, e: &'a Element, params: &HashMap<&'a str, &'a Element>, at: &str) -> Option<f32> {
        let child = e.children.iter().find(|c| matches!(c.name.as_str(), "float" | "param"))?;
        let text = if child.name == "float" {
            child.text.as_str()
        } else {
            match child.attr("ref").and_then(|r| params.get(r)).and_then(|np| kid(np, "float")) {
                Some(f) => f.text.as_str(),
                None => {
                    self.warn(format!("{at} names a <param> that is not declared or not a float; the ior was left out"));
                    return None;
                }
            }
        };
        match text.trim().parse::<f32>() {
            Ok(v) if DEFAULT_IORS.contains(&v) => None,
            Ok(v) if v > 0.0 && super::scene::ior_in_schema(v) => Some(v),
            Ok(v) if v.is_finite() && v <= 0.0 => {
                self.warn(format!("{at} {v} is 0 or below, which is no refractive index; the ior was left out"));
                None
            }
            Ok(v) if v.is_finite() => {
                self.warn(format!("{at} is {v}, which KHR_materials_ior does not allow (0, or 1 and above); the ior was left out"));
                None
            }
            _ => {
                self.warn(format!("{at} holds {}, not a number; the ior was left out", q(text)));
                None
            }
        }
    }

    /// `<texture texture="sampler sid" texcoord="...">` → the scene texture,
    /// through the sampler2D → surface → image chain.
    fn texture(&mut self, tex: &'a Element, params: &HashMap<&'a str, &'a Element>, at: &str) -> Result<Option<TexSource>, MeshError> {
        let name = tex.attr("texture").unwrap_or("");
        let texcoord = tex.attr("texcoord").unwrap_or("").to_string();
        let mut sampler = unspecified_sampler();
        let image_el: Option<&'a Element> = match params.get(name).and_then(|np| kid(np, "sampler2D")) {
            Some(s2d) => {
                sampler = sampler_of(s2d, at, &mut self.warnings);
                // 1.5 names the image directly; 1.4 goes through a surface.
                if let Some(ii) = kid(s2d, "instance_image") {
                    ii.attr("url").and_then(|u| self.doc.get(u))
                } else {
                    let surface_sid = kid(s2d, "source").map(|s| s.text.trim()).unwrap_or("");
                    let init = params
                        .get(surface_sid)
                        .and_then(|np| kid(np, "surface"))
                        .and_then(|s| kid(s, "init_from"))
                        .map(|i| i.text.trim());
                    init.and_then(|id| self.doc.by_id(id))
                }
            }
            None => {
                // Several exporters name the image itself here; three.js
                // accepts that too (its "#12530" fallback).
                let direct = self.doc.by_id(name).filter(|e| e.name == "image");
                if direct.is_some() {
                    self.warn_once(
                        "direct texture",
                        format!("{at} names image {} directly instead of a sampler; it was used as one", q(name)),
                    );
                }
                direct
            }
        };
        if let Some(extra_wrap) = maya_wrap(tex) {
            if params.get(name).and_then(|np| kid(np, "sampler2D")).is_none_or(|s| kid(s, "wrap_s").is_none()) {
                sampler.wrap_s = extra_wrap.0;
                sampler.wrap_t = extra_wrap.1;
            }
        }
        let Some(image_el) = image_el.filter(|e| e.name == "image") else {
            self.warn(format!("{at} names texture {} whose image cannot be resolved; it was dropped", q(name)));
            return Ok(None);
        };
        let Some(image) = self.image(image_el)? else { return Ok(None) };
        let key = (image, format!("{sampler:?}"));
        let texture = match self.texture_cache.get(&key) {
            Some(&t) => t,
            None => {
                self.scene.textures.push(Texture { name: self.scene.images[image].name.clone(), image, sampler });
                let t = self.scene.textures.len() - 1;
                self.texture_cache.insert(key, t);
                t
            }
        };
        Ok(Some(TexSource::Texture { texture, image, texcoord }))
    }

    fn image(&mut self, img: &'a Element) -> Result<Option<usize>, MeshError> {
        if let Some(i) = self.image_cache.get(&mat_ptr(img)) {
            return Ok(*i);
        }
        let iid = img.attr("id").unwrap_or("");
        let bytes: Option<Vec<u8>> = if let Some(data) = kid(img, "data") {
            Some(decode_hex(&data.text).ok_or_else(|| malformed(format!("image {} <data> is not hexadecimal", q(iid))))?)
        } else if let Some(init) = kid(img, "init_from") {
            // 1.5 wraps the URI in <ref> (or embeds <hex>).
            let uri = kid(init, "ref").map(|r| r.text.trim()).unwrap_or(init.text.trim()).to_string();
            if let Some(hex) = kid(init, "hex") {
                Some(decode_hex(&hex.text).ok_or_else(|| malformed(format!("image {} <hex> is not hexadecimal", q(iid))))?)
            } else {
                self.load_sidecar(&uri, &format!("image {}", q(iid)))
            }
        } else {
            self.warn(format!("image {} has neither <init_from> nor <data>; its textures were dropped", q(iid)));
            None
        };
        let out = match bytes {
            None => None,
            Some(data) => match ImageFormat::sniff(&data) {
                Some(format) => {
                    self.scene.images.push(Image {
                        name: img.attr("name").filter(|s| !s.is_empty()).map(str::to_string),
                        format,
                        data,
                    });
                    Some(self.scene.images.len() - 1)
                }
                None => {
                    self.warn(format!("image {} is not PNG, JPEG, WebP or KTX2; its textures were dropped", q(iid)));
                    None
                }
            },
        };
        self.image_cache.insert(mat_ptr(img), out);
        Ok(out)
    }

    /// Bytes of a file the document refers to, from the caller's sidecars.
    ///
    /// Relative references resolve inside the package. Absolute paths and
    /// `file:` URIs — what DCC tools write when they export from a project
    /// directory — cannot, but the same file shipped beside the document under
    /// its own name is unambiguous, so that is tried before giving up.
    fn load_sidecar(&mut self, uri: &str, what: &str) -> Option<Vec<u8>> {
        if uri.is_empty() {
            self.warn(format!("{what} has an empty reference; its textures were dropped"));
            return None;
        }
        if package::is_data_uri(uri) {
            self.warn(format!("{what} is a data: URI, which this reader does not decode; its textures were dropped"));
            return None;
        }
        if let Some(bytes) = package::normalize_relative(uri).and_then(|p| self.sidecars.get(&p)) {
            return Some(bytes);
        }
        let decoded = package::percent_decode(uri);
        let base = decoded.rsplit(['/', '\\']).next().unwrap_or("");
        if base != decoded.as_str() || package::normalize_relative(uri).is_none() {
            if let Some(bytes) = package::normalize_relative(base).and_then(|p| self.sidecars.get(&p)) {
                self.warn(format!("{what} refers to {}; the file of that name beside the document was used", q(uri)));
                return Some(bytes);
            }
        }
        self.warn(format!("{what} refers to {}, which was not supplied; its textures were dropped", q(uri)));
        None
    }

    /// The scene material for a Collada material bound with `inputs`, as
    /// drawn by a primitive whose texture coordinate sets are `prim_sets`.
    fn material(
        &mut self,
        mat: &'a Element,
        inputs: &[(String, u32)],
        prim_sets: &[u32],
        double_sided: bool,
    ) -> Result<Option<usize>, MeshError> {
        let mid = mat.attr("id").unwrap_or("");
        let effect_url = kid(mat, "instance_effect").and_then(|i| i.attr("url"));
        let Some(effect_url) = effect_url else {
            self.warn(format!("material {} has no instance_effect; it reads as white", q(mid)));
            return Ok(None);
        };
        let effect = self.doc.resolve(effect_url, "effect", &format!("material {}", q(mid)))?;
        let fx = self.effect(effect)?;
        // Which scene UV set a texcoord name means for this primitive.
        let uv_index = |texcoord: &str| -> Option<u32> {
            let set = inputs.iter().find(|(s, _)| s == texcoord).map(|(_, set)| *set).or(prim_sets.first().copied())?;
            prim_sets.iter().position(|s| *s == set).map(|i| i as u32)
        };
        let mut key = format!("{:x}\u{1}{}", mat_ptr(mat), double_sided);
        for src in [&fx.diffuse, &fx.emission].into_iter().flatten() {
            if let TexSource::Texture { texcoord, .. } = src {
                let _ = write!(key, "\u{1}{:?}", uv_index(texcoord));
            }
        }
        if let Some(i) = self.material_cache.get(&key) {
            return Ok(*i);
        }
        let mut m = Material {
            name: mat.attr("name").filter(|s| !s.is_empty()).map(str::to_string),
            metallic: 0.0,
            roughness: 1.0,
            double_sided: fx.double_sided || double_sided,
            ior: fx.ior,
            ..Default::default()
        };
        let texture_ref = |src: &TexSource, what: &str, warnings: &mut Vec<String>| -> Option<TextureRef> {
            let TexSource::Texture { texture, texcoord, .. } = src else { return None };
            match uv_index(texcoord) {
                Some(uv_set) => Some(TextureRef { texture: *texture, uv_set, transform: None }),
                None => {
                    warnings.push(clip(
                        &format!("material {}: the {what} texture's coordinates ({}) are not on the primitive; it was dropped", q(mid), q(texcoord)),
                        MAX_DETAIL,
                    ));
                    None
                }
            }
        };
        let mut diffuse_image = None;
        match &fx.diffuse {
            Some(TexSource::Color(c)) => {
                m.base_color = [srgb_decode(c[0]), srgb_decode(c[1]), srgb_decode(c[2]), 1.0];
            }
            Some(t @ TexSource::Texture { image, .. }) => {
                m.base_color_texture = texture_ref(t, "diffuse", &mut self.warnings);
                diffuse_image = Some(*image);
            }
            None => {}
        }
        match &fx.emission {
            Some(TexSource::Color(c)) => m.emissive = [srgb_decode(c[0]), srgb_decode(c[1]), srgb_decode(c[2])],
            Some(t @ TexSource::Texture { .. }) => {
                m.emissive_texture = texture_ref(t, "emission", &mut self.warnings);
                if m.emissive_texture.is_some() {
                    m.emissive = [1.0; 3];
                }
            }
            None => {}
        }
        match fx.shading {
            Shading::Constant => {
                // A constant shader's colour IS its emission (spec §8); the
                // scene says that with an unlit material.
                m.unlit = true;
                if fx.diffuse.is_none() {
                    match &fx.emission {
                        Some(TexSource::Color(c)) => {
                            m.base_color = [srgb_decode(c[0]), srgb_decode(c[1]), srgb_decode(c[2]), 1.0];
                            m.emissive = [0.0; 3];
                        }
                        // A map as much as a colour: the unlit material's
                        // base colour map.
                        Some(TexSource::Texture { image, .. }) if m.emissive_texture.is_some() => {
                            m.base_color_texture = m.emissive_texture.take();
                            m.base_color = [1.0; 4];
                            m.emissive = [0.0; 3];
                            diffuse_image = Some(*image);
                        }
                        _ => {}
                    }
                }
            }
            Shading::Lambert => m.roughness = 1.0,
            Shading::Phong => m.roughness = roughness_from_shininess(fx.shininess.unwrap_or(0.0)),
        }
        // Transparency (spec §8 "Determining Transparency").
        let factor = fx.transparency.unwrap_or(1.0);
        let (opacity, from_texture) = match &fx.transparent {
            None => (factor, false),
            Some((mode, TexSource::Color(c))) => {
                let lum = 0.212_671 * c[0] + 0.715_160 * c[1] + 0.072_169 * c[2];
                let o = match mode.as_str() {
                    "RGB_ZERO" => 1.0 - lum * factor,
                    "A_ZERO" => 1.0 - c[3] * factor,
                    "RGB_ONE" => lum * factor,
                    _ => c[3] * factor,
                };
                (o, false)
            }
            Some((mode, TexSource::Texture { image, .. })) => {
                if mode == "A_ONE" && Some(*image) == diffuse_image {
                    (factor, true)
                } else {
                    self.warn(format!(
                        "material {}: opacity from a separate texture ({mode}) is not carried; it was ignored",
                        q(mid)
                    ));
                    (1.0, false)
                }
            }
        };
        if from_texture || opacity < 1.0 - 1e-6 {
            m.alpha_mode = AlphaMode::Blend;
            m.base_color[3] = opacity.clamp(0.0, 1.0);
            if opacity <= 0.0 && !from_texture {
                self.warn(format!(
                    "material {} is fully transparent by the spec's transparency rules; some exporters invert \
                     <transparency>, so check its source",
                    q(mid)
                ));
            }
        }
        self.scene.materials.push(m);
        let i = Some(self.scene.materials.len() - 1);
        self.material_cache.insert(key, i);
        Ok(i)
    }

    // ── controllers ─────────────────────────────────────────────────────────

    fn controller(&mut self, ctrl: &'a Element) -> Result<Rc<Controller<'a>>, MeshError> {
        if let Some(c) = self.controller_cache.get(&mat_ptr(ctrl)) {
            return Ok(c.clone());
        }
        let cid = ctrl.attr("id").unwrap_or("");
        let out = if let Some(skin) = kid(ctrl, "skin") {
            Controller::Skin(Rc::new(self.skin(ctrl, skin)?))
        } else if let Some(morph) = kid(ctrl, "morph") {
            Controller::Morph(Rc::new(self.morph(ctrl, morph)?))
        } else {
            return Err(unsupported(format!("controller {} is neither a skin nor a morph", q(cid))));
        };
        let rc = Rc::new(out);
        self.controller_cache.insert(mat_ptr(ctrl), rc.clone());
        Ok(rc)
    }

    /// A `<skin>` controller: `bind_shape_matrix` (identity when absent), the
    /// `<joints>` with their `INV_BIND_MATRIX`es (16 floats each), and the
    /// `<vertex_weights>` (`<vcount>`, then joint/weight index pairs, joint -1 the
    /// bind shape). COLLADA 1.4.1 (https://www.khronos.org/files/collada_spec_1_4.pdf), 5-112 to 5-114, 5-60 and 5-132; the
    /// skinning equation, 4-6 to 4-7.
    fn skin(&mut self, ctrl: &'a Element, skin: &'a Element) -> Result<SkinData<'a>, MeshError> {
        let cid = ctrl.attr("id").unwrap_or("");
        let at = format!("skin controller {}", q(cid));
        let src_url = skin.attr("source").ok_or_else(|| malformed(format!("{at} has no source")))?;
        let target = self.doc.get(src_url).ok_or_else(|| malformed(format!("{at} source {} is not in the document", q(src_url))))?;
        let (geometry, morph) = match target.name.as_str() {
            "geometry" => (target, None),
            // Only a morph may sit under a skin. Checked on the element, before
            // anything is resolved: resolving a skin's source skin would
            // recurse as deep as a hostile chain goes (or forever, for a skin
            // that names itself).
            "controller" if kid(target, "morph").is_some() => match &*self.controller(target)? {
                Controller::Morph(m) => (m.base, Some(m.clone())),
                Controller::Skin(_) => return Err(unsupported(format!("{at} skins another skin"))),
            },
            "controller" => return Err(unsupported(format!("{at} skins another skin; only a morph may sit under a skin"))),
            other => return Err(malformed(format!("{at} source is a <{}>", clip(other, 30)))),
        };
        let bind_shape = match kid(skin, "bind_shape_matrix") {
            None => math::IDENTITY,
            Some(b) => {
                let v = parse_f64_list(&b.text, &format!("{at} bind_shape_matrix"))?;
                matrix_from_row_major(&v).ok_or_else(|| malformed(format!("{at} bind_shape_matrix has {} values", v.len())))?
            }
        };
        let joints_el = kid(skin, "joints").ok_or_else(|| malformed(format!("{at} has no <joints>")))?;
        let mut joint_src = None;
        let mut ibm_src = None;
        for i in kids(joints_el, "input") {
            let url = i.attr("source").unwrap_or("");
            match i.attr("semantic") {
                Some("JOINT") => joint_src = Some(self.doc.resolve(url, "source", &format!("{at} JOINT"))?),
                Some("INV_BIND_MATRIX") => ibm_src = Some(self.doc.resolve(url, "source", &format!("{at} INV_BIND_MATRIX"))?),
                _ => {}
            }
        }
        let joint_src = joint_src.ok_or_else(|| malformed(format!("{at} <joints> has no JOINT input")))?;
        let (names, nview) = self.stream_names(joint_src, "JOINT")?;
        let joints_are_ids = nview.array.name == "IDREF_array";
        let joints: Vec<String> = (0..nview.count).map(|i| names[nview.at(i, 0)].clone()).collect();
        if joints.is_empty() || joints.len() > MAX_JOINTS {
            return Err(malformed(format!("{at} has {} joints", joints.len())));
        }
        let ibms: Vec<Mat4> = match ibm_src {
            None => vec![bind_shape; joints.len()],
            Some(src) => {
                let (vals, v) = self.stream_f64(src, 16, "INV_BIND_MATRIX")?;
                if v.count != joints.len() {
                    return Err(malformed(format!("{at} has {} inverse bind matrices for {} joints", v.count, joints.len())));
                }
                (0..v.count)
                    .map(|i| {
                        let row: Vec<f64> = (0..16).map(|c| vals[v.at(i, c)]).collect();
                        let ibm = matrix_from_row_major(&row).unwrap_or(math::IDENTITY);
                        // The scene's skinning has no bind-shape matrix: it
                        // is folded into each joint's inverse bind.
                        math::mat_mul(&ibm, &bind_shape)
                    })
                    .collect()
            }
        };
        let vw = kid(skin, "vertex_weights").ok_or_else(|| malformed(format!("{at} has no <vertex_weights>")))?;
        let count = attr_count(vw, "count", &at)?.ok_or_else(|| malformed(format!("{at} vertex_weights has no count")))?;
        let mut w_stride = 0usize;
        let mut joint_off = None;
        let mut weight = None;
        for i in kids(vw, "input") {
            let off = attr_count(i, "offset", &at)?.ok_or_else(|| malformed(format!("{at} vertex_weights input has no offset")))?;
            if off > MAX_INPUT_OFFSET {
                return Err(malformed(format!("{at} vertex_weights input offset {off}")));
            }
            w_stride = w_stride.max(off + 1);
            match i.attr("semantic") {
                Some("JOINT") => joint_off = Some(off),
                Some("WEIGHT") => {
                    let url = i.attr("source").unwrap_or("");
                    weight = Some((off, self.doc.resolve(url, "source", &format!("{at} WEIGHT"))?));
                }
                _ => {}
            }
        }
        let joint_off = joint_off.ok_or_else(|| malformed(format!("{at} vertex_weights has no JOINT input")))?;
        let (weight_off, weight_src) = weight.ok_or_else(|| malformed(format!("{at} vertex_weights has no WEIGHT input")))?;
        let (wvals, wview) = self.stream_f32(weight_src, 1, "WEIGHT")?;
        let vcount = match kid(vw, "vcount") {
            Some(v) => self.indices(v, &format!("{at} <vcount>"))?,
            None => Vec::new(),
        };
        if vcount.len() != count {
            return Err(malformed(format!("{at} declares {count} weighted vertices and its <vcount> lists {}", vcount.len())));
        }
        let v = match kid(vw, "v") {
            Some(v) => self.signed(v, &format!("{at} <v>"))?,
            None => Vec::new(),
        };
        let total: usize = vcount.iter().map(|c| *c as usize).fold(0usize, |a, b| a.saturating_add(b));
        if total.checked_mul(w_stride) != Some(v.len()) {
            return Err(malformed(format!("{at} <vcount> sums to {total} influences and <v> holds {} values", v.len())));
        }
        let mut starts = Vec::with_capacity(count + 1);
        let mut pairs = Vec::with_capacity(total);
        let mut cursor = 0usize;
        for n in &vcount {
            starts.push(pairs.len());
            for _ in 0..*n {
                let j = v[cursor + joint_off];
                let wi = v[cursor + weight_off];
                cursor += w_stride;
                if j < -1 || j >= joints.len() as i64 {
                    return Err(malformed(format!("{at} names joint {j} of {}", joints.len())));
                }
                if wi < 0 || wi as usize >= wview.count {
                    return Err(malformed(format!("{at} names weight {wi} of {}", wview.count)));
                }
                pairs.push((j, wvals[wview.at(wi as usize, 0)]));
            }
        }
        starts.push(pairs.len());
        Ok(SkinData {
            name: ctrl.attr("name").filter(|s| !s.is_empty()).map(str::to_string),
            geometry,
            morph,
            joints,
            joints_are_ids,
            ibms,
            bind_shape,
            starts,
            pairs,
        })
    }

    fn morph(&mut self, ctrl: &'a Element, morph: &'a Element) -> Result<MorphData<'a>, MeshError> {
        let cid = ctrl.attr("id").unwrap_or("");
        let at = format!("morph controller {}", q(cid));
        let url = morph.attr("source").ok_or_else(|| malformed(format!("{at} has no source")))?;
        let base = self.doc.get(url).ok_or_else(|| malformed(format!("{at} source {} is not in the document", q(url))))?;
        if base.name != "geometry" {
            return Err(unsupported(format!("{at} morphs a <{}>; only geometry bases are supported", clip(&base.name, 30))));
        }
        let relative = match morph.attr("method").unwrap_or("NORMALIZED") {
            "NORMALIZED" => false,
            "RELATIVE" => true,
            other => return Err(malformed(format!("{at} method {} is neither NORMALIZED nor RELATIVE", q(other)))),
        };
        let targets_el = kid(morph, "targets").ok_or_else(|| malformed(format!("{at} has no <targets>")))?;
        let mut target_src = None;
        let mut weight_src = None;
        for i in kids(targets_el, "input") {
            let url = i.attr("source").unwrap_or("");
            match i.attr("semantic") {
                Some("MORPH_TARGET") => target_src = Some(self.doc.resolve(url, "source", &format!("{at} MORPH_TARGET"))?),
                Some("MORPH_WEIGHT") => weight_src = Some(self.doc.resolve(url, "source", &format!("{at} MORPH_WEIGHT"))?),
                _ => {}
            }
        }
        let target_src = target_src.ok_or_else(|| malformed(format!("{at} has no MORPH_TARGET input")))?;
        let weight_src = weight_src.ok_or_else(|| malformed(format!("{at} has no MORPH_WEIGHT input")))?;
        let (ids, tview) = self.stream_names(target_src, "MORPH_TARGET")?;
        let (wvals, wview) = self.stream_f32(weight_src, 1, "MORPH_WEIGHT")?;
        if tview.count != wview.count {
            return Err(malformed(format!("{at} has {} targets and {} weights", tview.count, wview.count)));
        }
        if tview.count > MAX_MORPH_TARGETS {
            return Err(unsupported(format!("{at} has {} morph targets, over the {MAX_MORPH_TARGETS} limit", tview.count)));
        }
        let mut targets = Vec::with_capacity(tview.count);
        let mut names = Vec::with_capacity(tview.count);
        for i in 0..tview.count {
            let id = &ids[tview.at(i, 0)];
            let g = self
                .doc
                .by_id(id)
                .filter(|e| e.name == "geometry")
                .ok_or_else(|| malformed(format!("{at} target {} is not a geometry of the document", q(id))))?;
            names.push(g.attr("name").unwrap_or(id).to_string());
            targets.push(g);
        }
        let weights = (0..wview.count).map(|i| wvals[wview.at(i, 0)]).collect();
        Ok(MorphData { controller: mat_ptr(ctrl), base, relative, targets, names, weights })
    }

    // ── visual scene ────────────────────────────────────────────────────────

    fn read_asset(&mut self) -> Result<(), MeshError> {
        let Some(asset) = kid(self.doc.root, "asset") else { return Ok(()) };
        let meter = match kid(asset, "unit").and_then(|u| u.attr("meter")) {
            None => 1.0,
            Some(m) => m
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|m| m.is_finite() && *m > 0.0)
                .ok_or_else(|| malformed(format!("<unit meter={}> is not a positive length", q(m))))?,
        };
        self.meter = meter;
        let up = kid(asset, "up_axis").map(|u| u.text.trim().to_string()).unwrap_or_else(|| "Y_UP".into());
        let rotation = match up.as_str() {
            "Y_UP" => None,
            // Z up: right +x, up +z, in −y (spec 1.4.1 §5 asset): a −90° turn about X.
            "Z_UP" => Some(math::quat_to_mat(math::quat_from_axis_angle([1.0, 0.0, 0.0], -std::f64::consts::FRAC_PI_2))),
            // X up: right −y, up +x, in +z: a +90° turn about Z.
            "X_UP" => Some(math::quat_to_mat(math::quat_from_axis_angle([0.0, 0.0, 1.0], std::f64::consts::FRAC_PI_2))),
            other => return Err(malformed(format!("<up_axis> is {}, not X_UP, Y_UP or Z_UP", q(other)))),
        };
        if meter != 1.0 || rotation.is_some() {
            let mut c = math::scaling([meter; 3]);
            if let Some(r) = rotation {
                c = math::mat_mul(&r, &c);
            }
            // Rotations of exactly ±90° in f64 leave 6e-17 where zero belongs;
            // snapping keeps axis-aligned data axis-aligned.
            for v in c.iter_mut() {
                if v.abs() < 1e-15 {
                    *v = 0.0;
                }
            }
            self.correction = Some(c);
        }
        Ok(())
    }

    fn read_nodes(&mut self) -> Result<(), MeshError> {
        let scene_el = kid(self.doc.root, "scene");
        let vs = match scene_el.and_then(|s| kid(s, "instance_visual_scene")).and_then(|i| i.attr("url")) {
            Some(url) => Some(self.doc.resolve(url, "visual_scene", "<scene>")?),
            None => {
                let first = self.doc.library("library_visual_scenes", "visual_scene").into_iter().next();
                if first.is_some() {
                    self.warn("the document has no <scene>; its first visual scene was used".into());
                }
                first
            }
        };
        let Some(vs) = vs else {
            // A library of geometry with no scene: take every mesh at identity,
            // as the glTF reader does for a scene-less document.
            self.warn("the document has no visual scene; every geometry was placed at the origin unmaterialled".into());
            for g in self.doc.library("library_geometries", "geometry") {
                let binding = Rc::new(Binding { entries: Vec::new(), signature: String::new() });
                let ir = self.push_node(None, None, Rc::new(Vec::new()), None)?;
                self.info[ir].parts.push(Part { source: PartSource::Geometry(g), binding, skeletons: Vec::new() });
            }
            return Ok(());
        };
        self.scene.name = vs.attr("name").filter(|s| !s.is_empty()).map(str::to_string);

        enum Work<'a> {
            Enter { el: &'a Element, parent: Option<usize>, depth: usize },
            Leave(usize),
        }
        // Element addresses on the current path: an instance_node naming one
        // of them would expand forever.
        let mut active: HashSet<usize> = HashSet::new();
        let mut stack: Vec<Work<'a>> =
            kids(vs, "node").collect::<Vec<_>>().into_iter().rev().map(|el| Work::Enter { el, parent: None, depth: 1 }).collect();
        // Instances past a node's first light and first camera: a scene node
        // places one of each.
        let mut more = 0usize;
        while let Some(w) = stack.pop() {
            let (el, parent, depth) = match w {
                Work::Leave(p) => {
                    active.remove(&p);
                    continue;
                }
                Work::Enter { el, parent, depth } => (el, parent, depth),
            };
            if depth > MAX_NODE_DEPTH {
                return Err(unsupported(format!("nodes nest deeper than {MAX_NODE_DEPTH}")));
            }
            let ops = self.node_ops(el)?;
            let ir = self.push_node(Some(el), parent, ops, el.attr("name"))?;
            let (mut light, mut camera) = (false, false);
            for c in &el.children {
                match c.name.as_str() {
                    "instance_geometry" => {
                        let url = c.attr("url").unwrap_or("");
                        let g = self.doc.resolve(url, "geometry", &format!("node {} instance_geometry", q(el.attr("id").unwrap_or(""))))?;
                        let binding = Rc::new(self.binding(c)?);
                        self.info[ir].parts.push(Part { source: PartSource::Geometry(g), binding, skeletons: Vec::new() });
                    }
                    "instance_controller" => {
                        let url = c.attr("url").unwrap_or("");
                        let ctrl = self.doc.resolve(url, "controller", &format!("node {} instance_controller", q(el.attr("id").unwrap_or(""))))?;
                        let binding = Rc::new(self.binding(c)?);
                        let skeletons = kids(c, "skeleton").map(|s| s.text.trim().to_string()).collect();
                        self.info[ir].parts.push(Part { source: PartSource::Controller(ctrl), binding, skeletons });
                    }
                    "instance_camera" if !camera => {
                        camera = true;
                        self.instanced_cameras.push((ir, c));
                    }
                    "instance_light" if !light => {
                        light = true;
                        self.instanced_lights.push((ir, c));
                    }
                    "instance_camera" | "instance_light" => more += 1,
                    _ => {}
                }
            }
            active.insert(mat_ptr(el));
            stack.push(Work::Leave(mat_ptr(el)));
            for c in el.children.iter().rev() {
                match c.name.as_str() {
                    "node" => stack.push(Work::Enter { el: c, parent: Some(ir), depth: depth + 1 }),
                    "instance_node" => {
                        let url = c.attr("url").unwrap_or("");
                        let target = self.doc.resolve(url, "node", &format!("node {} instance_node", q(el.attr("id").unwrap_or(""))))?;
                        if active.contains(&mat_ptr(target)) {
                            return Err(malformed(format!("instance_node {} instances its own ancestor; the node graph is a cycle", q(url))));
                        }
                        stack.push(Work::Enter { el: target, parent: Some(ir), depth: depth + 1 });
                    }
                    _ => {}
                }
            }
        }
        if more > 0 {
            self.warn(format!(
                "{more} more light or camera instance(s) were dropped: a node places one light and one camera at most, and keeps \
                 the first of each"
            ));
        }
        Ok(())
    }

    /// `el`'s transform stack, parsed and charged on first use and shared
    /// after: an `instance_node` re-enters its target once per copy, and
    /// re-parsing per copy made memory grow as copies x transforms.
    fn node_ops(&mut self, el: &'a Element) -> Result<Rc<Vec<Op>>, MeshError> {
        if let Some(ops) = self.ops_by_element.get(&mat_ptr(el)) {
            return Ok(ops.clone());
        }
        let ops = parse_ops(el)?;
        self.charge(ops.iter().map(|o| o.values.len()).sum(), "node transforms")?;
        let ops = Rc::new(ops);
        self.ops_by_element.insert(mat_ptr(el), ops.clone());
        Ok(ops)
    }

    fn push_node(&mut self, el: Option<&'a Element>, parent: Option<usize>, ops: Rc<Vec<Op>>, name: Option<&'a str>) -> Result<usize, MeshError> {
        if self.scene.nodes.len() >= MAX_NODES {
            return Err(unsupported(format!("the scene has more than {MAX_NODES} nodes (counting instance_node copies)")));
        }
        let m = compose_ops(&ops)?;
        let node = Node {
            name: name.filter(|s| !s.is_empty()).map(str::to_string),
            transform: Transform::from_matrix(m),
            ..Default::default()
        };
        self.scene.nodes.push(node);
        let ir = self.scene.nodes.len() - 1;
        match parent {
            Some(p) => self.scene.nodes[p].children.push(ir),
            None => self.scene.roots.push(ir),
        }
        self.info.push(NodeInfo {
            ops,
            parts: Vec::new(),
            sid: el.and_then(|e| e.attr("sid")),
            id: el.and_then(|e| e.attr("id")),
            name: el.and_then(|e| e.attr("name")),
        });
        if let Some(e) = el {
            self.nodes_by_element.entry(mat_ptr(e)).or_default().push(ir);
        }
        Ok(ir)
    }

    /// Turn every node's instances into scene meshes and skins.
    fn assemble(&mut self) -> Result<(), MeshError> {
        for ir in 0..self.info.len() {
            let parts = std::mem::take(&mut self.info[ir].parts);
            if !parts.is_empty() {
                self.assemble_node(ir, parts)?;
            }
        }
        Ok(())
    }

    fn assemble_node(&mut self, ir: usize, parts: Vec<Part<'a>>) -> Result<(), MeshError> {
        struct Resolved<'a> {
            key: String,
            geom: Rc<GeomData>,
            binding: Rc<Binding<'a>>,
            skin: Option<(Rc<SkinData<'a>>, usize, Option<usize>)>,
            morph: Option<(usize, Rc<MorphData<'a>>)>,
        }
        let mut resolved: Vec<Resolved<'a>> = Vec::new();
        for part in parts {
            let (elem, geom, skin, morph) = match part.source {
                PartSource::Geometry(g) => (g, self.geometry(g)?, None, None),
                PartSource::Controller(c) => match &*self.controller(c)? {
                    Controller::Skin(sd) => {
                        let geom = self.geometry(sd.geometry)?;
                        let (skin_index, bind_shape_joint) = self.skin_for(c, sd, &part.skeletons, ir, &geom)?;
                        let morph = sd.morph.clone().map(|m| (m.controller, m));
                        (c, geom, Some((sd.clone(), skin_index, bind_shape_joint)), morph)
                    }
                    Controller::Morph(md) => (c, self.geometry(md.base)?, None, Some((md.controller, md.clone()))),
                },
            };
            if geom.prims.is_empty() {
                continue;
            }
            let key = format!(
                "{:x}\u{1}{}\u{1}{:?}",
                mat_ptr(elem),
                part.binding.signature,
                skin.as_ref().map(|(_, s, j)| (*s, *j))
            );
            resolved.push(Resolved { key, geom, binding: part.binding, skin, morph });
        }
        // Parts that share a skin (or have none) and carry no morph merge into
        // one mesh; anything else needs a node of its own.
        let mut groups: Vec<(Option<usize>, Option<usize>, Vec<Resolved<'a>>)> = Vec::new();
        let mut by_skin: HashMap<Option<usize>, usize> = HashMap::new();
        for r in resolved {
            let skin = r.skin.as_ref().map(|(_, s, _)| *s);
            let morph = r.morph.as_ref().map(|(c, _)| *c);
            match (morph, by_skin.get(&skin)) {
                (None, Some(&g)) => groups[g].2.push(r),
                _ => {
                    if morph.is_none() {
                        by_skin.insert(skin, groups.len());
                    }
                    groups.push((skin, morph, vec![r]));
                }
            }
        }
        for (gi, (skin, morph, members)) in groups.into_iter().enumerate() {
            let key = members.iter().map(|m| m.key.as_str()).collect::<Vec<_>>().join("\u{3}");
            let mesh = match self.mesh_cache.get(&key) {
                Some(&m) => m,
                None => {
                    let m = self.build_mesh(&members.iter().map(|r| (r.geom.clone(), r.binding.clone(), r.skin.clone(), r.morph.clone())).collect::<Vec<_>>())?;
                    self.mesh_cache.insert(key, m);
                    m
                }
            };
            let target = if gi == 0 {
                ir
            } else {
                let name = self.scene.nodes[ir].name.as_ref().map(|n| format!("{n}_part{gi}"));
                let child = self.push_node(None, Some(ir), Rc::new(Vec::new()), None)?;
                self.scene.nodes[child].name = name;
                child
            };
            self.scene.nodes[target].mesh = Some(mesh);
            self.scene.nodes[target].skin = skin;
            if let Some(c) = morph {
                self.morph_nodes.entry(c).or_default().push(target);
            }
        }
        Ok(())
    }

    #[allow(clippy::type_complexity)]
    fn build_mesh(
        &mut self,
        members: &[(Rc<GeomData>, Rc<Binding<'a>>, Option<(Rc<SkinData<'a>>, usize, Option<usize>)>, Option<(usize, Rc<MorphData<'a>>)>)],
    ) -> Result<usize, MeshError> {
        let verts: usize = members.iter().flat_map(|m| &m.0.prims).map(|p| p.positions.len()).sum();
        let idx: usize = members.iter().flat_map(|m| &m.0.prims).map(|p| p.indices.len()).sum();
        self.total_vertices = self.total_vertices.saturating_add(verts);
        self.total_indices = self.total_indices.saturating_add(idx);
        if self.total_vertices > MAX_SCENE_VERTICES || self.total_indices > MAX_SCENE_INDICES {
            return Err(unsupported(format!(
                "the document instances more geometry than the scene's {MAX_SCENE_VERTICES}-vertex / \
                 {MAX_SCENE_INDICES}-index limits"
            )));
        }
        let mut mesh = SceneMesh { name: members.first().and_then(|m| m.0.name.clone()), ..Default::default() };
        for (geom, binding, skin, morph) in members {
            let morph_offsets = match morph {
                Some((_, md)) => Some(self.morph_offsets(md, geom)?),
                None => None,
            };
            if let Some((_, md)) = morph {
                mesh.weights = md.weights.clone();
                mesh.target_names = md.names.clone();
            }
            for gp in &geom.prims {
                let material = match &gp.symbol {
                    None => None,
                    Some(sym) => match binding.entries.iter().find(|(s, _, _)| s == sym) {
                        Some((_, mat, inputs)) => self.material(mat, inputs, &gp.uv_sets, geom.double_sided)?,
                        None => match self.doc.by_id(sym).filter(|e| e.name == "material") {
                            Some(mat) => {
                                self.warn_once(
                                    &format!("unbound {sym}"),
                                    format!("material symbol {} is not bound by the instance; the material with that id was used", q(sym)),
                                );
                                self.material(mat, &[], &gp.uv_sets, geom.double_sided)?
                            }
                            None => {
                                self.warn_once(
                                    &format!("unbound {sym}"),
                                    format!("material symbol {} is bound to nothing; those primitives have no material", q(sym)),
                                );
                                None
                            }
                        },
                    },
                };
                let mut p = Primitive {
                    positions: gp.positions.clone(),
                    normals: gp.normals.clone(),
                    tangents: None,
                    uvs: gp.uvs.clone(),
                    colors: gp.colors.clone(),
                    skin: None,
                    indices: gp.indices.clone(),
                    material,
                    targets: Vec::new(),
                    topology: gp.topology,
                };
                if let Some((sd, _, bind_shape_joint)) = skin {
                    p.skin = Some(self.skin_weights(sd, gp, geom.vertex_count, *bind_shape_joint)?);
                }
                if let Some(offsets) = &morph_offsets {
                    let need = offsets.len().saturating_mul(gp.vertex.len()).saturating_mul(6);
                    self.morph_values = self.morph_values.saturating_add(need);
                    if self.morph_values > super::scene::MAX_MORPH_VALUES {
                        return Err(unsupported(format!("the morph targets expand past {} values", super::scene::MAX_MORPH_VALUES)));
                    }
                    for (positions, normals) in offsets {
                        let tp: Vec<[f32; 3]> = gp.vertex.iter().map(|v| positions[*v as usize]).collect();
                        let tn = normals.as_ref().map(|n| gp.vertex.iter().map(|v| n[*v as usize]).collect());
                        p.targets.push(MorphTarget { positions: tp, normals: tn });
                    }
                    if p.targets.iter().any(|t| t.normals.is_some()) {
                        for t in p.targets.iter_mut() {
                            if t.normals.is_none() {
                                t.normals = Some(vec![[0.0; 3]; p.positions.len()]);
                            }
                        }
                    }
                }
                mesh.primitives.push(p);
            }
        }
        self.scene.meshes.push(mesh);
        Ok(self.scene.meshes.len() - 1)
    }

    /// Per `<vertices>` element offsets of every morph target: position and,
    /// when both base and target keep normals in `<vertices>`, normal.
    #[allow(clippy::type_complexity)]
    fn morph_offsets(&mut self, md: &MorphData<'a>, base: &GeomData) -> Result<Vec<(Vec<[f32; 3]>, Option<Vec<[f32; 3]>>)>, MeshError> {
        let base_pos = self.vertices_stream(md.base, "POSITION")?.ok_or_else(|| malformed("a morph base has no POSITION"))?;
        let base_nrm = self.vertices_stream(md.base, "NORMAL")?;
        let mut out = Vec::with_capacity(md.targets.len());
        for t in &md.targets {
            let tid = t.attr("id").unwrap_or("");
            // Charged before the target's streams are decoded: one geometry
            // listed 1,024 times is decoded and copied 1,024 times.
            let need = base_pos.len().saturating_mul(if base_nrm.is_some() { 6 } else { 3 });
            self.morph_values = self.morph_values.saturating_add(need);
            if self.morph_values > super::scene::MAX_MORPH_VALUES {
                return Err(unsupported(format!(
                    "the morph targets (at {}) expand past {} values",
                    q(tid),
                    super::scene::MAX_MORPH_VALUES
                )));
            }
            let tp = self.vertices_stream(t, "POSITION")?.ok_or_else(|| malformed(format!("morph target {} has no POSITION", q(tid))))?;
            if tp.len() != base_pos.len() || tp.len() != base.vertex_count {
                return Err(malformed(format!(
                    "morph target {} has {} vertices and its base {}",
                    q(tid),
                    tp.len(),
                    base_pos.len()
                )));
            }
            let positions: Vec<[f32; 3]> = if md.relative {
                tp
            } else {
                tp.iter().zip(&base_pos).map(|(t, b)| [t[0] - b[0], t[1] - b[1], t[2] - b[2]]).collect()
            };
            let normals = match (&base_nrm, self.vertices_stream(t, "NORMAL")?) {
                (Some(bn), Some(tn)) if tn.len() == bn.len() && bn.len() == base_pos.len() => Some(if md.relative {
                    tn
                } else {
                    tn.iter().zip(bn).map(|(t, b)| [t[0] - b[0], t[1] - b[1], t[2] - b[2]]).collect()
                }),
                _ => None,
            };
            out.push((positions, normals));
        }
        Ok(out)
    }

    /// One `<vertices>` input of a geometry, as plain triples.
    fn vertices_stream(&mut self, geom: &'a Element, semantic: &str) -> Result<Option<Vec<[f32; 3]>>, MeshError> {
        let Some(v) = kid(geom, "mesh").and_then(|m| kid(m, "vertices")) else { return Ok(None) };
        let Some(input) = kids(v, "input").find(|i| i.attr("semantic") == Some(semantic)) else { return Ok(None) };
        let src = self.doc.resolve(input.attr("source").unwrap_or(""), "source", &format!("<vertices> {semantic}"))?;
        let (vals, view) = self.stream_f32(src, 3, semantic)?;
        Ok(Some((0..view.count).map(|i| [vals[view.at(i, 0)], vals[view.at(i, 1)], vals[view.at(i, 2)]]).collect()))
    }

    fn skin_weights(&mut self, sd: &SkinData<'a>, gp: &GeomPrim, vertex_count: usize, bind_shape_joint: Option<usize>) -> Result<SkinWeights, MeshError> {
        if sd.starts.len() != vertex_count + 1 {
            return Err(malformed(format!(
                "skin {} weights {} vertices and its geometry has {vertex_count}",
                q(sd.name.as_deref().unwrap_or("")),
                sd.starts.len().saturating_sub(1)
            )));
        }
        let mut per_vertex: Vec<Vec<(u16, f32)>> = Vec::with_capacity(gp.vertex.len());
        let mut widest = 1usize;
        let mut clipped = 0usize;
        // A `<vertices>` index's influences, gathered once however many
        // corners share it: rebuilding (and keeping the capacity of) 20k of
        // them per corner was 751 MB for 3,000 corners.
        let mut gathered: HashMap<u32, Vec<(u16, f32)>> = HashMap::new();
        for &v in &gp.vertex {
            if let Some(infl) = gathered.get(&v) {
                widest = widest.max(infl.len());
                per_vertex.push(infl.clone());
                continue;
            }
            let key = v;
            let v = v as usize;
            let mut infl: Vec<(u16, f32)> = Vec::new();
            for &(j, w) in &sd.pairs[sd.starts[v]..sd.starts[v + 1]] {
                let joint = if j < 0 {
                    match bind_shape_joint {
                        Some(b) => b,
                        None => continue,
                    }
                } else {
                    j as usize
                };
                infl.push((u16::try_from(joint).map_err(|_| malformed("a skin joint index exceeds 65535"))?, w));
            }
            if infl.len() > MAX_INFLUENCES {
                infl.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                infl.truncate(MAX_INFLUENCES);
                infl.shrink_to_fit();
                clipped += 1;
            }
            widest = widest.max(infl.len());
            gathered.insert(key, infl.clone());
            per_vertex.push(infl);
        }
        if clipped > 0 {
            self.warn(format!("{clipped} skinned vertices have more than {MAX_INFLUENCES} influences; the strongest were kept"));
        }
        let influences = widest.div_ceil(4) * 4;
        let mut joints = Vec::with_capacity(per_vertex.len() * influences);
        let mut weights = Vec::with_capacity(per_vertex.len() * influences);
        for infl in per_vertex {
            for k in 0..influences {
                let (j, w) = infl.get(k).copied().unwrap_or((0, 0.0));
                joints.push(j);
                weights.push(w);
            }
        }
        Ok(SkinWeights { influences, joints, weights })
    }

    /// The scene skin for a controller instanced with these skeleton roots,
    /// and the joint standing for the bind shape when `<v>` names joint -1.
    fn skin_for(
        &mut self,
        ctrl: &'a Element,
        sd: &SkinData<'a>,
        skeletons: &[String],
        instance_node: usize,
        geom: &GeomData,
    ) -> Result<(usize, Option<usize>), MeshError> {
        let needs_bind_shape = sd.pairs.iter().any(|(j, w)| *j < 0 && *w != 0.0) && geom.vertex_count > 0;
        let instance_key = (mat_ptr(ctrl), skeletons.to_vec(), needs_bind_shape.then_some(instance_node));
        if let Some(&hit) = self.instance_skins.get(&instance_key) {
            return Ok(hit);
        }
        let resolved = self.skin_for_uncached(ctrl, sd, skeletons, instance_node, geom)?;
        self.instance_skins.insert(instance_key, resolved);
        Ok(resolved)
    }

    fn skin_for_uncached(
        &mut self,
        ctrl: &'a Element,
        sd: &SkinData<'a>,
        skeletons: &[String],
        instance_node: usize,
        geom: &GeomData,
    ) -> Result<(usize, Option<usize>), MeshError> {
        let roots: Vec<usize> = skeletons
            .iter()
            .filter_map(|url| self.doc.get(url))
            .filter_map(|e| self.nodes_by_element.get(&mat_ptr(e)).and_then(|v| v.first().copied()))
            .collect();
        if roots.len() < skeletons.len() {
            self.warn(format!(
                "skin {}: a <skeleton> names no node of the scene; its joints were looked up everywhere",
                q(ctrl.attr("id").unwrap_or(""))
            ));
        }
        let mut joints = Vec::with_capacity(sd.joints.len());
        for name in &sd.joints {
            let found = if sd.joints_are_ids {
                self.lookup_global(name, LookupKey::Id)
            } else {
                let scoped = if roots.is_empty() { None } else { self.lookup_scoped(&roots, name) };
                scoped
                    .or_else(|| self.lookup_global(name, LookupKey::Sid))
                    .or_else(|| self.lookup_global(name, LookupKey::Id))
                    .or_else(|| self.lookup_global(name, LookupKey::Name))
            };
            joints.push(found.ok_or_else(|| {
                malformed(format!("skin {} joint {} matches no node of the scene", q(ctrl.attr("id").unwrap_or("")), q(name)))
            })?);
        }
        let mut ibms = sd.ibms.clone();
        let needs_bind_shape = sd.pairs.iter().any(|(j, w)| *j < 0 && *w != 0.0) && geom.vertex_count > 0;
        let bind_shape_joint = if needs_bind_shape {
            // "-1 refers to the bind shape": the vertex follows the instancing
            // node itself, which the scene says with that node as a joint.
            joints.push(instance_node);
            ibms.push(sd.bind_shape);
            Some(joints.len() - 1)
        } else {
            None
        };
        // By content, not by element: an exporter (this one included) writes a
        // controller per primitive, and controllers binding the same joints
        // the same way are one skin, whose primitives belong in one mesh.
        let mut key = format!("{joints:?}\u{1}{:?}", sd.name);
        for m in &ibms {
            for v in m {
                let _ = write!(key, " {:x}", v.to_bits());
            }
        }
        let index = match self.skin_cache.get(&key) {
            Some(&s) => s,
            None => {
                let skeleton = if roots.len() == 1 { roots.first().copied() } else { None };
                self.scene.skins.push(Skin { name: sd.name.clone(), joints, inverse_bind_matrices: ibms, skeleton });
                let s = self.scene.skins.len() - 1;
                self.skin_cache.insert(key, s);
                s
            }
        };
        Ok((index, bind_shape_joint))
    }

    fn lookup_scoped(&mut self, roots: &[usize], name: &str) -> Option<usize> {
        if !self.subtree_sid.contains_key(roots) {
            // Breadth-first from each root in order: the spec's sid scoping,
            // and the first match is the nearest one.
            let mut map: HashMap<String, usize> = HashMap::new();
            let mut queue: std::collections::VecDeque<usize> = roots.iter().copied().collect();
            let mut seen: HashSet<usize> = HashSet::new();
            while let Some(n) = queue.pop_front() {
                if !seen.insert(n) {
                    continue;
                }
                if let Some(sid) = self.info.get(n).and_then(|i| i.sid) {
                    map.entry(sid.to_string()).or_insert(n);
                }
                if let Some(node) = self.scene.nodes.get(n) {
                    queue.extend(node.children.iter().copied());
                }
            }
            self.subtree_sid.insert(roots.to_vec(), Rc::new(map));
        }
        self.subtree_sid.get(roots).and_then(|m| m.get(name).copied())
    }

    fn lookup_global(&mut self, name: &str, by: LookupKey) -> Option<usize> {
        let table = match by {
            LookupKey::Sid => &mut self.global_sid,
            LookupKey::Id => &mut self.global_id,
            LookupKey::Name => &mut self.global_name,
        };
        if table.is_none() {
            let mut map = HashMap::new();
            for (i, info) in self.info.iter().enumerate() {
                let k = match by {
                    LookupKey::Sid => info.sid,
                    LookupKey::Id => info.id,
                    LookupKey::Name => info.name,
                };
                if let Some(k) = k {
                    map.entry(k.to_string()).or_insert(i);
                }
            }
            *table = Some(map);
        }
        table.as_ref().and_then(|m| m.get(name).copied())
    }

    // ── animation ───────────────────────────────────────────────────────────

    fn read_animations(&mut self) -> Result<(), MeshError> {
        let tops = self.doc.library("library_animations", "animation");
        if tops.is_empty() {
            return Ok(());
        }
        self.is_root = vec![false; self.scene.nodes.len()];
        for &r in &self.scene.roots {
            if let Some(x) = self.is_root.get_mut(r) {
                *x = true;
            }
        }
        // Every channel, and for every <animation> the channels under it. The
        // walk is depth-first, so an animation's own and descendant channels
        // are one contiguous run of `channels`: a [start, end) range each.
        // (Listing every channel under every enclosing animation instead cost
        // channels x nesting depth — gigabytes from a 20 MB document.)
        let mut channels: Vec<&'a Element> = Vec::new();
        let mut under: HashMap<usize, (usize, usize)> = HashMap::new();
        enum Walk<'a> {
            Enter(&'a Element),
            Leave(usize, usize),
        }
        let mut stack: Vec<Walk<'a>> = tops.iter().rev().map(|a| Walk::Enter(a)).collect();
        while let Some(w) = stack.pop() {
            match w {
                Walk::Leave(animation, start) => {
                    under.insert(animation, (start, channels.len()));
                }
                Walk::Enter(a) => {
                    let start = channels.len();
                    channels.extend(a.children.iter().filter(|c| c.name == "channel"));
                    stack.push(Walk::Leave(mat_ptr(a), start));
                    for c in a.children.iter().rev().filter(|c| c.name == "animation") {
                        stack.push(Walk::Enter(c));
                    }
                }
            }
        }
        let clips = self.doc.library("library_animation_clips", "animation_clip");
        // (name, channels, the clip's start and end in seconds).
        let mut groups: Vec<(Option<String>, Vec<usize>, Option<(f32, Option<f32>)>)> = Vec::new();
        if clips.is_empty() {
            let name = if tops.len() == 1 { tops[0].attr("name").filter(|s| !s.is_empty()).map(str::to_string) } else { None };
            groups.push((name, (0..channels.len()).collect(), None));
        } else {
            // Clips may each instance the same large animation; every clip
            // then holds (and later resamples) all of its channels again.
            let mut referenced = 0usize;
            let mut in_a_clip = vec![false; channels.len()];
            for clip_el in clips {
                let mut chans: BTreeSet<usize> = BTreeSet::new();
                for ia in kids(clip_el, "instance_animation") {
                    let url = ia.attr("url").unwrap_or("");
                    let a = self.doc.resolve(url, "animation", "animation_clip instance_animation")?;
                    if let Some(&(start, end)) = under.get(&mat_ptr(a)) {
                        referenced = referenced.saturating_add(end - start);
                        if referenced > MAX_CLIP_CHANNELS {
                            return Err(unsupported(format!(
                                "the animation clips reference more than {MAX_CLIP_CHANNELS} channels in total"
                            )));
                        }
                        chans.extend(start..end);
                    }
                }
                for &c in &chans {
                    if let Some(f) = in_a_clip.get_mut(c) {
                        *f = true;
                    }
                }
                let name = clip_el.attr("name").or(clip_el.attr("id")).filter(|s| !s.is_empty()).map(str::to_string);
                // 1.4.1: `start` defaults to 0, `end` to the end of the
                // longest animation the clip instances.
                let time = |k: &str| clip_el.attr(k).map(|v| v.trim().parse::<f32>().ok().filter(|f| f.is_finite()));
                let window = match (time("start"), time("end")) {
                    (Some(None), _) | (_, Some(None)) => {
                        self.warn_once("clip-time", format!("animation clip {} has a start or end that is not a number; it plays whole", q(clip_el.attr("id").unwrap_or(""))));
                        None
                    }
                    (s, e) => {
                        let (s, e) = (s.flatten().unwrap_or(0.0), e.flatten());
                        if e.is_some_and(|e| e < s) {
                            self.warn_once("clip-time", format!("animation clip {} ends before it starts; it plays whole", q(clip_el.attr("id").unwrap_or(""))));
                            None
                        } else {
                            Some((s, e))
                        }
                    }
                };
                groups.push((name, chans.into_iter().collect(), window));
            }
            let unclipped = in_a_clip.iter().filter(|f| !**f).count();
            if unclipped > 0 {
                self.warn_once(
                    "unclipped",
                    format!("{unclipped} animation channel(s) are in no <animation_clip>; with clips present they were not read"),
                );
            }
        }
        for (name, idx, window) in groups {
            let chans: Vec<&'a Element> = idx.iter().filter_map(|i| channels.get(*i).copied()).collect();
            let mut anim = self.animation(name, &chans)?;
            if let Some((start, end)) = window {
                clip_window(&mut anim, start, end);
            }
            if !anim.channels.is_empty() {
                self.scene.animations.push(anim);
            }
        }
        Ok(())
    }

    fn animation(&mut self, name: Option<String>, chans: &[&'a Element]) -> Result<Animation, MeshError> {
        let mut per_node: BTreeMap<usize, Vec<NodeCurve>> = BTreeMap::new();
        let mut per_weights: BTreeMap<usize, Vec<WeightCurve>> = BTreeMap::new();
        for ch in chans {
            let target = ch.attr("target").unwrap_or("");
            let source = ch.attr("source").unwrap_or("");
            let at = format!("animation channel {}", q(target));
            match self.resolve_target(target)? {
                Target::Ignored(why) => {
                    self.warn_once(&format!("target {target}"), format!("{at}: {why}; it was skipped"));
                }
                Target::Transform { element, op, select, dim } => {
                    let sampler = self.doc.resolve(source, "sampler", &at)?;
                    let curve = self.shared_curve(sampler, dim, &at)?;
                    for &ir in self.nodes_by_element.get(&element).into_iter().flatten() {
                        per_node.entry(ir).or_default().push((op, select, curve.clone()));
                    }
                }
                Target::Weights { controller, index, targets } => {
                    let sampler = self.doc.resolve(source, "sampler", &at)?;
                    let dim = if index.is_some() { 1 } else { targets };
                    let curve = self.shared_curve(sampler, dim, &at)?;
                    let nodes = self.morph_nodes.get(&controller).cloned().unwrap_or_default();
                    if nodes.is_empty() {
                        self.warn_once(
                            &format!("unplaced {target}"),
                            format!("{at} animates the weights of a morph no node of the scene draws; it was skipped"),
                        );
                    }
                    for ir in nodes {
                        per_weights.entry(ir).or_default().push((index, curve.clone()));
                    }
                }
            }
        }
        let mut out = Animation { name, channels: Vec::new() };
        for (ir, curves) in per_node {
            self.node_channels(ir, &curves, &mut out)?;
        }
        for (ir, curves) in per_weights {
            self.weight_channel(ir, &curves, &mut out)?;
        }
        Ok(out)
    }

    /// Every morph controller's MORPH_WEIGHT source and array, by address.
    fn weight_arrays(&mut self) -> &HashMap<usize, (usize, usize)> {
        if self.weight_arrays.is_none() {
            let mut map = HashMap::new();
            for ctrl in self.doc.library("library_controllers", "controller") {
                let Some(m) = kid(ctrl, "morph") else { continue };
                let weights = kid(m, "targets")
                    .into_iter()
                    .flat_map(|t| kids(t, "input").collect::<Vec<_>>())
                    .find(|i| i.attr("semantic") == Some("MORPH_WEIGHT"))
                    .and_then(|i| i.attr("source"))
                    .and_then(|u| self.doc.get(u));
                let Some(src) = weights else { continue };
                let array = src.children.iter().find(|c| c.name == "float_array");
                let count = array.map_or(0, |a| a.text.split_ascii_whitespace().count());
                map.entry(mat_ptr(src)).or_insert((mat_ptr(ctrl), count));
                if let Some(a) = array {
                    map.entry(mat_ptr(a)).or_insert((mat_ptr(ctrl), count));
                }
            }
            self.weight_arrays = Some(map);
        }
        self.weight_arrays.get_or_insert_with(HashMap::new)
    }

    fn resolve_target(&mut self, target: &str) -> Result<Target, MeshError> {
        let (id, sids, select) = parse_target(target).ok_or_else(|| malformed(format!("animation target {} does not parse", q(target))))?;
        let Some(el) = self.doc.by_id(id) else {
            return Ok(Target::Ignored(format!("it names {}, which is not in the document", q(id))));
        };
        if el.name == "node" {
            if sids.len() != 1 {
                return Ok(Target::Ignored("it addresses something inside a node other than a transform".into()));
            }
            let Some(info) = self.nodes_by_element.get(&mat_ptr(el)).and_then(|v| v.first()).and_then(|i| self.info.get(*i)) else {
                return Ok(Target::Ignored("its node is not part of the visual scene".into()));
            };
            let Some(op) = info.ops.iter().position(|o| o.sid.as_deref() == Some(sids[0])) else {
                return Ok(Target::Ignored(format!("node {} has no transform with sid {}", q(id), q(sids[0]))));
            };
            let kind = info.ops[op].kind;
            let width = info.ops[op].values.len();
            let comp = match (&select, kind) {
                (Select::All, _) => None,
                (_, OpKind::LookAt) => return Ok(Target::Ignored("animated <lookat> is not supported".into())),
                (Select::Member(m), OpKind::Translate | OpKind::Scale) => match m.as_str() {
                    "X" => Some(0),
                    "Y" => Some(1),
                    "Z" => Some(2),
                    _ => return Err(malformed(format!("animation target {} selects member {}", q(target), q(m)))),
                },
                (Select::Member(m), OpKind::Rotate) => match m.as_str() {
                    "X" => Some(0),
                    "Y" => Some(1),
                    "Z" => Some(2),
                    "ANGLE" => Some(3),
                    _ => return Err(malformed(format!("animation target {} selects member {}", q(target), q(m)))),
                },
                (Select::Member(m), OpKind::Matrix) => {
                    return Err(malformed(format!("animation target {} selects member {} of a matrix", q(target), q(m))))
                }
                (Select::Index(ix), OpKind::Matrix) => match ix.as_slice() {
                    // (row)(column) of the matrix as written.
                    [r, c] if *r < 4 && *c < 4 => Some(r * 4 + c),
                    [i] if *i < 16 => Some(*i),
                    _ => return Err(malformed(format!("animation target {} indexes outside a 4x4 matrix", q(target)))),
                },
                (Select::Index(ix), _) => match ix.as_slice() {
                    [i] if *i < width => Some(*i),
                    _ => return Err(malformed(format!("animation target {} indexes outside its transform", q(target)))),
                },
            };
            let dim = if comp.is_some() { 1 } else { width };
            return Ok(Target::Transform { element: mat_ptr(el), op, select: comp, dim });
        }
        // Morph weights: the MORPH_WEIGHT array, or its <source>, by id.
        if matches!(el.name.as_str(), "float_array" | "source") {
            if let Some(&(controller, targets)) = self.weight_arrays().get(&mat_ptr(el)) {
                let index = match select {
                    Select::All => None,
                    Select::Index(ix) if ix.len() == 1 && ix[0] < targets => Some(ix[0]),
                    _ => return Err(malformed(format!("animation target {} selects outside the morph weights", q(target)))),
                };
                return Ok(Target::Weights { controller, index, targets });
            }
        }
        Ok(Target::Ignored(format!("it animates a <{}>, which the scene does not model", clip(&el.name, 30))))
    }

    fn shared_curve(&mut self, sampler: &'a Element, dim: usize, at: &str) -> Result<Rc<Curve>, MeshError> {
        if let Some(c) = self.curve_cache.get(&(mat_ptr(sampler), dim)) {
            return Ok(Rc::clone(c));
        }
        let c = Rc::new(self.curve(sampler, dim, at)?);
        self.curve_cache.insert((mat_ptr(sampler), dim), Rc::clone(&c));
        Ok(c)
    }

    /// A `<sampler>`'s curve: INPUT times, OUTPUT values and the required
    /// INTERPOLATION, with BEZIER and HERMITE tangents mapped to control points.
    /// COLLADA 1.4.1 (https://www.khronos.org/files/collada_spec_1_4.pdf), 5-102 to 5-104, and "Curve Interpolation", 4-1 to 4-6
    /// (on 4-3 the second tangent line is misprinted "T0 = 3(P1 - C1)"; it is T1).
    fn curve(&mut self, sampler: &'a Element, dim: usize, at: &str) -> Result<Curve, MeshError> {
        let mut input = None;
        let mut output = None;
        let mut interp = None;
        let mut in_tan = None;
        let mut out_tan = None;
        for i in kids(sampler, "input") {
            let url = i.attr("source").unwrap_or("");
            let sem = i.attr("semantic").unwrap_or("");
            let slot = match sem {
                "INPUT" => &mut input,
                "OUTPUT" => &mut output,
                "INTERPOLATION" => &mut interp,
                "IN_TANGENT" => &mut in_tan,
                "OUT_TANGENT" => &mut out_tan,
                _ => continue,
            };
            *slot = Some(self.doc.resolve(url, "source", &format!("{at} {sem}"))?);
        }
        let input = input.ok_or_else(|| malformed(format!("{at}: sampler has no INPUT")))?;
        let output = output.ok_or_else(|| malformed(format!("{at}: sampler has no OUTPUT")))?;
        let (tv, tview) = self.stream_f32(input, 1, "animation INPUT")?;
        let times: Vec<f64> = (0..tview.count).map(|i| tv[tview.at(i, 0)] as f64).collect();
        if times.is_empty() {
            return Err(malformed(format!("{at}: sampler has no keys")));
        }
        if times.windows(2).any(|w| w[1] < w[0]) {
            return Err(malformed(format!("{at}: key times decrease")));
        }
        let (ov, oview) = self.stream_f64(output, dim, "animation OUTPUT")?;
        if oview.count != times.len() {
            return Err(malformed(format!("{at}: {} keys but {} output values", times.len(), oview.count)));
        }
        if oview.comps.len() != dim {
            return Err(malformed(format!("{at}: output has {} components where its target takes {dim}", oview.comps.len())));
        }
        let mut values = Vec::with_capacity(times.len() * dim);
        for k in 0..times.len() {
            for c in 0..dim {
                values.push(ov[oview.at(k, c)]);
            }
        }
        let modes: Vec<Interp> = match interp {
            None => vec![Interp::Linear; times.len()],
            Some(src) => {
                let (names, nview) = self.stream_names(src, "INTERPOLATION")?;
                if nview.count != times.len() {
                    return Err(malformed(format!("{at}: {} keys but {} interpolations", times.len(), nview.count)));
                }
                (0..nview.count)
                    .map(|i| match names[nview.at(i, 0)].as_str() {
                        "LINEAR" => Ok(Interp::Linear),
                        "STEP" => Ok(Interp::Step),
                        "BEZIER" => Ok(Interp::Bezier),
                        "HERMITE" => Ok(Interp::Hermite),
                        other => Err(unsupported(format!("{at}: {} interpolation is not supported", q(other)))),
                    })
                    .collect::<Result<_, _>>()?
            }
        };
        let curved = modes[..modes.len().saturating_sub(1)].iter().any(|m| matches!(m, Interp::Bezier | Interp::Hermite));
        let mut tangents = None;
        if curved {
            let (Some(i), Some(o)) = (in_tan, out_tan) else {
                return Err(malformed(format!("{at}: BEZIER/HERMITE keys without IN_TANGENT and OUT_TANGENT")));
            };
            let read = |r: &mut Self, src: &'a Element| -> Result<(Vec<f64>, usize), MeshError> {
                let (v, view) = r.stream_f64(src, 1, "animation tangent")?;
                if view.count != times.len() {
                    return Err(malformed(format!("{at}: {} keys but {} tangents", times.len(), view.count)));
                }
                let w = view.comps.len();
                if w != dim && w != dim + 1 && w != 2 * dim {
                    return Err(malformed(format!("{at}: tangents of {w} values for a {dim}-value output")));
                }
                let mut out = Vec::with_capacity(view.count * w);
                for k in 0..view.count {
                    for c in 0..w {
                        out.push(v[view.at(k, c)]);
                    }
                }
                Ok((out, w))
            };
            let (iv, iw) = read(self, i)?;
            let (ov2, ow) = read(self, o)?;
            if iw != ow {
                return Err(malformed(format!("{at}: in and out tangents differ in width")));
            }
            tangents = Some((iv, ov2, iw));
        }
        Ok(Curve { times, values, dim, modes, tangents })
    }

    /// Resampled TRS channels for one node from the curves animating it.
    fn node_channels(&mut self, ir: usize, curves: &[NodeCurve], out: &mut Animation) -> Result<(), MeshError> {
        let ops = self.info[ir].ops.clone();
        let all_step = curves.iter().all(|(_, _, c)| c.all_step());
        let whole_matrices = curves.iter().all(|(op, sel, _)| sel.is_none() && ops.get(*op).is_some_and(|o| o.kind == OpKind::Matrix));
        let splines = curves.iter().any(|(_, _, c)| c.has_spline());
        // Whole-matrix keys are decomposed and their TRS interpolated, which is
        // how three.js plays them; anything else is composed per sample, and
        // between samples nothing but dense keys can follow it.
        let grid = !all_step && (!whole_matrices || splines);
        let times = self.sample_times(curves.iter().map(|(_, _, c)| c.as_ref()), all_step, grid)?;
        let is_root = self.is_root.get(ir).copied().unwrap_or(false);
        let mut tr = Vec::with_capacity(times.len() * 3);
        let mut rot = Vec::with_capacity(times.len() * 4);
        let mut sc = Vec::with_capacity(times.len() * 3);
        let mut prev_q: Option<[f64; 4]> = None;
        let mut scratch = vec![0.0f64; 16];
        for &t in &times {
            let mut now: Vec<Op> = ops.as_ref().clone();
            for (op, sel, c) in curves {
                let Some(o) = now.get_mut(*op) else { continue };
                scratch.resize(c.dim, 0.0);
                c.eval(t as f64, &mut scratch);
                match sel {
                    None => {
                        let n = o.values.len().min(c.dim);
                        o.values[..n].copy_from_slice(&scratch[..n]);
                    }
                    Some(i) => {
                        if let Some(v) = o.values.get_mut(*i) {
                            *v = scratch[0];
                        }
                    }
                }
            }
            let mut m = compose_ops(&now)?;
            if is_root {
                if let Some(cm) = &self.correction {
                    m = math::mat_mul(cm, &m);
                }
            }
            let (t3, mut q4, s3) = math::decompose_near(&m, prev_q).ok_or_else(|| {
                unsupported(format!("node {ir} is animated into a sheared transform at t={t}, which TRS keys cannot express"))
            })?;
            if let Some(p) = prev_q {
                if math::quat_dot(p, q4) < 0.0 {
                    q4 = [-q4[0], -q4[1], -q4[2], -q4[3]];
                }
            }
            prev_q = Some(q4);
            tr.extend(t3.iter().map(|v| *v as f32));
            rot.extend(q4.iter().map(|v| *v as f32));
            sc.extend(s3.iter().map(|v| *v as f32));
        }
        let interpolation = if all_step { Interpolation::Step } else { Interpolation::Linear };
        out.channels.push(Channel { node: ir, path: AnimPath::Translation, interpolation, times: times.clone(), values: tr });
        out.channels.push(Channel { node: ir, path: AnimPath::Rotation, interpolation, times: times.clone(), values: rot });
        out.channels.push(Channel { node: ir, path: AnimPath::Scale, interpolation, times, values: sc });
        Ok(())
    }

    fn weight_channel(&mut self, ir: usize, curves: &[WeightCurve], out: &mut Animation) -> Result<(), MeshError> {
        let Some(mi) = self.scene.nodes.get(ir).and_then(|n| n.mesh) else { return Ok(()) };
        let defaults = self.scene.meshes[mi].weights.clone();
        let n = self.scene.meshes[mi].target_count();
        if n == 0 {
            return Ok(());
        }
        let all_step = curves.iter().all(|(_, c)| c.all_step());
        let splines = curves.iter().any(|(_, c)| c.has_spline());
        let times = self.sample_times(curves.iter().map(|(_, c)| c.as_ref()), all_step, splines && !all_step)?;
        let mut values = Vec::with_capacity(times.len() * n);
        let mut scratch = vec![0.0f64; n.max(1)];
        for &t in &times {
            let mut w: Vec<f32> = if defaults.len() == n { defaults.clone() } else { vec![0.0; n] };
            for (index, c) in curves {
                scratch.resize(c.dim, 0.0);
                c.eval(t as f64, &mut scratch);
                match index {
                    Some(i) => {
                        if let Some(x) = w.get_mut(*i) {
                            *x = scratch[0] as f32;
                        }
                    }
                    None => {
                        for (x, v) in w.iter_mut().zip(&scratch) {
                            *x = *v as f32;
                        }
                    }
                }
            }
            values.extend(w);
        }
        let interpolation = if all_step { Interpolation::Step } else { Interpolation::Linear };
        out.channels.push(Channel { node: ir, path: AnimPath::Weights, interpolation, times, values });
        Ok(())
    }

    /// Key times for resampled curves: every key; a point just before each
    /// discontinuity (STEP, or two keys at one time) so linear interpolation
    /// jumps there; and, when `grid`, regular samples between.
    fn sample_times<'c>(&mut self, curves: impl Iterator<Item = &'c Curve> + Clone, all_step: bool, grid: bool) -> Result<Vec<f32>, MeshError> {
        let mut times: Vec<f64> = Vec::new();
        for c in curves.clone() {
            times.extend_from_slice(&c.times);
            if !all_step {
                c.jumps(&mut times);
            }
        }
        if grid {
            let lo = times.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = times.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            if hi > lo {
                let steps = (((hi - lo) * RESAMPLE_FPS as f64).ceil() as usize).clamp(1, MAX_RESAMPLES);
                for i in 0..=steps {
                    times.push(lo + (hi - lo) * i as f64 / steps as f64);
                }
            }
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut out: Vec<f32> = Vec::with_capacity(times.len());
        for t in times {
            let t = t as f32;
            if out.last().is_none_or(|p| t > *p) {
                out.push(t);
            }
        }
        self.total_keys = self.total_keys.saturating_add(out.len() * 3);
        if self.total_keys > MAX_ANIMATION_KEYS {
            return Err(unsupported(format!("the animation resamples past {MAX_ANIMATION_KEYS} keys")));
        }
        Ok(out)
    }

    fn apply_correction(&mut self) {
        let Some(c) = self.correction else { return };
        let animated: HashSet<usize> = self.scene.animations.iter().flat_map(|a| &a.channels).map(|c| c.node).collect();
        for &r in &self.scene.roots {
            let node = &mut self.scene.nodes[r];
            let _ = animated.contains(&r);
            node.transform = Transform::from_matrix(math::mat_mul(&c, &node.transform.matrix()));
        }
    }
}

#[derive(Clone, Copy)]
enum LookupKey {
    Sid,
    Id,
    Name,
}

enum Target {
    Transform { element: usize, op: usize, select: Option<usize>, dim: usize },
    Weights { controller: usize, index: Option<usize>, targets: usize },
    Ignored(String),
}

#[derive(Debug, PartialEq)]
enum Select {
    All,
    Member(String),
    Index(Vec<usize>),
}

/// `id/sid/…` with an optional `.member` or `(i)(j)` on the last part
/// (spec 1.4.1 §3 "COLLADA Target Addressing").
fn parse_target(t: &str) -> Option<(&str, Vec<&str>, Select)> {
    let t = t.trim();
    let mut parts: Vec<&str> = t.split('/').collect();
    let last = parts.pop()?;
    let (last, select) = if let Some(open) = last.find('(') {
        let mut ix = Vec::new();
        let mut rest = &last[open..];
        while let Some(r) = rest.strip_prefix('(') {
            let close = r.find(')')?;
            ix.push(r[..close].trim().parse::<usize>().ok()?);
            rest = &r[close + 1..];
        }
        if !rest.is_empty() || ix.is_empty() || ix.len() > 2 {
            return None;
        }
        (&last[..open], Select::Index(ix))
    } else if let Some(dot) = last.find('.') {
        (&last[..dot], Select::Member(last[dot + 1..].to_string()))
    } else {
        (last, Select::All)
    };
    parts.push(last);
    let id = parts.first().copied()?;
    if id.is_empty() {
        return None;
    }
    Some((id, parts[1..].to_vec(), select))
}

/// A curve animating one transform element of a node: (element index in the
/// node's ops, the component it drives or `None` for all, the curve).
type NodeCurve = (usize, Option<usize>, Rc<Curve>);

/// A curve animating morph weights: (target index or `None` for all, curve).
type WeightCurve = (Option<usize>, Rc<Curve>);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Interp {
    Linear,
    Step,
    Bezier,
    Hermite,
}

/// One sampler, ready to evaluate.
struct Curve {
    times: Vec<f64>,
    values: Vec<f64>,
    dim: usize,
    modes: Vec<Interp>,
    /// In-tangents, out-tangents and their per-key width (`dim`: 1-D values;
    /// `dim + 1`: one time then a value per dimension; `2 * dim`: a
    /// (time, value) pair per dimension).
    tangents: Option<(Vec<f64>, Vec<f64>, usize)>,
}

impl Curve {
    fn all_step(&self) -> bool {
        self.times.len() < 2 || self.modes[..self.modes.len() - 1].iter().all(|m| *m == Interp::Step)
    }

    fn has_spline(&self) -> bool {
        self.modes[..self.modes.len().saturating_sub(1)].iter().any(|m| matches!(m, Interp::Bezier | Interp::Hermite))
    }

    /// Times just before each value jump, for linear resampling to land on.
    fn jumps(&self, out: &mut Vec<f64>) {
        for k in 0..self.times.len().saturating_sub(1) {
            let (t0, t1) = (self.times[k], self.times[k + 1]);
            let step = self.modes[k] == Interp::Step && t1 > t0;
            let duplicate = t1 == t0;
            if step {
                let eps = (STEP_EPSILON as f64).max(t1.abs() * 1e-6).min((t1 - t0) / 2.0);
                out.push(t1 - eps);
            } else if duplicate && k > 0 && self.times[k - 1] < t0 {
                let eps = (STEP_EPSILON as f64).max(t0.abs() * 1e-6).min((t0 - self.times[k - 1]) / 2.0);
                out.push(t0 - eps);
            }
        }
    }

    fn key(&self, k: usize) -> &[f64] {
        &self.values[k * self.dim..(k + 1) * self.dim]
    }

    fn eval(&self, t: f64, out: &mut [f64]) {
        let n = self.times.len();
        let dim = self.dim;
        if n == 1 || t <= self.times[0] {
            // Before the first key, the first value; with keys duplicated at
            // the start, the last of them.
            let k = self.times.iter().rposition(|x| *x <= t).unwrap_or(0);
            out[..dim].copy_from_slice(self.key(k));
            return;
        }
        if t >= self.times[n - 1] {
            out[..dim].copy_from_slice(self.key(n - 1));
            return;
        }
        // The last key at or before t: with duplicate times, the later one.
        let k = self.times.partition_point(|x| *x <= t) - 1;
        let (t0, t1) = (self.times[k], self.times[k + 1]);
        let (a, b) = (self.key(k), self.key(k + 1));
        match self.modes[k] {
            Interp::Step => out[..dim].copy_from_slice(a),
            Interp::Linear => {
                let u = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
                for d in 0..dim {
                    out[d] = a[d] + (b[d] - a[d]) * u;
                }
            }
            mode @ (Interp::Bezier | Interp::Hermite) => {
                let Some((tin, tout, w)) = &self.tangents else {
                    out[..dim].copy_from_slice(a);
                    return;
                };
                for d in 0..dim {
                    // Control points (time, value) of this dimension.
                    let pick = |tan: &[f64], key: usize| -> (Option<f64>, f64) {
                        let row = &tan[key * w..(key + 1) * w];
                        if *w == 2 * dim {
                            (Some(row[2 * d]), row[2 * d + 1])
                        } else if *w == dim + 1 {
                            (Some(row[0]), row[1 + d])
                        } else {
                            (None, row[d])
                        }
                    };
                    let (ot, ov) = pick(tout, k);
                    let (it, iv) = pick(tin, k + 1);
                    let third = (t1 - t0) / 3.0;
                    let (c0, c1) = if mode == Interp::Bezier {
                        ((ot.unwrap_or(t0 + third), ov), (it.unwrap_or(t1 - third), iv))
                    } else {
                        // Hermite tangents T relate to Bézier controls by
                        // T0 = 3(C0 − P0), T1 = 3(P1 − C1) (spec §4).
                        let (dt0, dv0) = (ot.unwrap_or(t1 - t0), ov);
                        let (dt1, dv1) = (it.unwrap_or(t1 - t0), iv);
                        ((t0 + dt0 / 3.0, a[d] + dv0 / 3.0), (t1 - dt1 / 3.0, b[d] - dv1 / 3.0))
                    };
                    out[d] = bezier_at(t, (t0, a[d]), c0, c1, (t1, b[d]));
                }
            }
        }
    }
}

/// The value of a 2-D cubic Bézier at time `t`, solving the time polynomial
/// by bisection. Control times are clamped into the segment so time stays
/// monotonic, which is what animation editors enforce too.
fn bezier_at(t: f64, p0: (f64, f64), c0: (f64, f64), c1: (f64, f64), p1: (f64, f64)) -> f64 {
    let x1 = c0.0.clamp(p0.0, p1.0);
    let x2 = c1.0.clamp(p0.0, p1.0);
    let bez = |a: f64, b: f64, c: f64, d: f64, s: f64| {
        let r = 1.0 - s;
        r * r * r * a + 3.0 * r * r * s * b + 3.0 * r * s * s * c + s * s * s * d
    };
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if bez(p0.0, x1, x2, p1.0, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(p0.1, c0.1, c1.1, p1.1, 0.5 * (lo + hi))
}

fn parse_f64_list(text: &str, what: &str) -> Result<Vec<f64>, MeshError> {
    text.split_ascii_whitespace()
        .take(64)
        .map(|t| t.parse::<f64>().ok().filter(|v| v.is_finite()).ok_or_else(|| malformed(format!("{what} holds {}, not a number", q(t)))))
        .collect()
}

fn matrix_from_row_major(v: &[f64]) -> Option<Mat4> {
    if v.len() != 16 {
        return None;
    }
    let mut m = [0.0f64; 16];
    m.copy_from_slice(v);
    Some(math::transpose(&m))
}

fn color4(text: &str, at: &str) -> Result<[f32; 4], MeshError> {
    let v: Vec<f32> = text
        .split_ascii_whitespace()
        .take(5)
        .map(|t| t.parse::<f32>().ok().filter(|x| x.is_finite()).ok_or_else(|| malformed(format!("{at} colour holds {}", q(t)))))
        .collect::<Result<_, _>>()?;
    match v.len() {
        3 => Ok([v[0], v[1], v[2], 1.0]),
        4 => Ok([v[0], v[1], v[2], v[3]]),
        n => Err(malformed(format!("{at} colour has {n} components"))),
    }
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    let val = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    digits.chunks_exact(2).map(|p| Some(val(p[0])? << 4 | val(p[1])?)).collect()
}

/// `<double_sided>` anywhere in an `<extra>` of this element: GOOGLEEARTH,
/// MAYA, MAX3D and FCOLLADA all spell it the same.
fn double_sided_extra(e: &Element) -> bool {
    kids(e, "extra").any(|x| {
        x.descendants_named("double_sided").iter().any(|d| matches!(d.text.trim(), "1" | "true" | "TRUE" | "True"))
    })
}

/// Wrap modes from the MAYA `<extra>` of a `<texture>` (what three.js reads).
fn maya_wrap(tex: &Element) -> Option<(Wrap, Wrap)> {
    let tech = kids(tex, "extra").flat_map(|x| kids(x, "technique").collect::<Vec<_>>()).find(|t| t.attr("profile") == Some("MAYA"))?;
    let flag = |name: &str| kid(tech, name).map(|e| matches!(e.text.trim(), "1" | "true" | "TRUE"));
    let axis = |wrap: &str, mirror: &str| match (flag(wrap), flag(mirror)) {
        (_, Some(true)) => Some(Wrap::MirroredRepeat),
        (Some(false), _) => Some(Wrap::ClampToEdge),
        (Some(true), _) => Some(Wrap::Repeat),
        _ => None,
    };
    let (s, t) = (axis("wrapU", "mirrorU"), axis("wrapV", "mirrorV"));
    if s.is_none() && t.is_none() {
        return None;
    }
    Some((s.unwrap_or_default(), t.unwrap_or_default()))
}

/// The scene's "no filter specified" sampler (what glTF's reader produces for
/// a sampler without `minFilter`): mipmapped, readers' defaults.
fn unspecified_sampler() -> Sampler {
    Sampler { mipmaps: true, ..Default::default() }
}

fn sampler_of(s2d: &Element, at: &str, warnings: &mut Vec<String>) -> Sampler {
    let mut out = unspecified_sampler();
    let wrap = |e: Option<&Element>, warnings: &mut Vec<String>| -> Wrap {
        match e.map(|e| e.text.trim()) {
            None | Some("WRAP") => Wrap::Repeat,
            Some("MIRROR") => Wrap::MirroredRepeat,
            Some("CLAMP") => Wrap::ClampToEdge,
            Some(other) => {
                warnings.push(clip(&format!("{at}: wrap mode {} is read as clamp", q(other)), MAX_DETAIL));
                Wrap::ClampToEdge
            }
        }
    };
    out.wrap_s = wrap(kid(s2d, "wrap_s"), warnings);
    out.wrap_t = wrap(kid(s2d, "wrap_t"), warnings);
    let filter = |name: &str| -> Option<Filter> {
        match name {
            "NEAREST" => Some(Filter::Nearest),
            "LINEAR" => Some(Filter::Linear),
            _ => None,
        }
    };
    if let Some(m) = kid(s2d, "magfilter") {
        out.mag_filter = filter(m.text.trim());
    }
    if let Some(m) = kid(s2d, "minfilter") {
        let (min, mip) = match m.text.trim() {
            "NEAREST" => (Some(Filter::Nearest), None),
            "LINEAR" => (Some(Filter::Linear), None),
            "NEAREST_MIPMAP_NEAREST" => (Some(Filter::Nearest), Some(Filter::Nearest)),
            "LINEAR_MIPMAP_NEAREST" => (Some(Filter::Linear), Some(Filter::Nearest)),
            "NEAREST_MIPMAP_LINEAR" => (Some(Filter::Nearest), Some(Filter::Linear)),
            "LINEAR_MIPMAP_LINEAR" => (Some(Filter::Linear), Some(Filter::Linear)),
            _ => (None, None),
        };
        out.min_filter = min;
        // 1.4.1 may also name the mip filter on its own element.
        let mip = mip.or_else(|| kid(s2d, "mipfilter").and_then(|e| filter(e.text.trim())));
        out.mipmaps = mip.is_some();
        if let Some(f) = mip {
            out.mipmap_filter = f;
        }
        if min.is_none() {
            out.mipmaps = true;
        }
    }
    out
}

/// A node's transform elements, in document order.
fn parse_ops(node: &Element) -> Result<Vec<Op>, MeshError> {
    let mut ops = Vec::new();
    let nid = node.attr("id").unwrap_or("");
    for c in &node.children {
        let kind = match c.name.as_str() {
            "matrix" => OpKind::Matrix,
            "translate" => OpKind::Translate,
            "rotate" => OpKind::Rotate,
            "scale" => OpKind::Scale,
            "lookat" => OpKind::LookAt,
            "skew" => {
                return Err(unsupported(format!(
                    "node {} has a <skew> transform, which this converter does not model",
                    q(nid)
                )))
            }
            _ => continue,
        };
        let want = match kind {
            OpKind::Matrix => 16,
            OpKind::Rotate => 4,
            OpKind::LookAt => 9,
            OpKind::Translate | OpKind::Scale => 3,
        };
        let values = parse_f64_list(&c.text, &format!("node {} <{}>", q(nid), c.name))?;
        if values.len() != want {
            return Err(malformed(format!("node {} <{}> has {} values, expected {want}", q(nid), c.name, values.len())));
        }
        if ops.len() >= MAX_NODE_OPS {
            return Err(unsupported(format!(
                "node {} has more than {MAX_NODE_OPS} transform elements; every animated sample recomposes them all",
                q(nid)
            )));
        }
        ops.push(Op { sid: c.attr("sid").map(str::to_string), kind, values });
    }
    Ok(ops)
}

/// The local matrix of a transform stack: each element post-multiplied, so
/// the first written is the outermost (spec 1.4.1 §5 `<node>`).
fn compose_ops(ops: &[Op]) -> Result<Mat4, MeshError> {
    let mut m = math::IDENTITY;
    for op in ops {
        let v = &op.values;
        let local = match op.kind {
            OpKind::Matrix => matrix_from_row_major(v).unwrap_or(math::IDENTITY),
            OpKind::Translate => math::translation([v[0], v[1], v[2]]),
            OpKind::Scale => math::scaling([v[0], v[1], v[2]]),
            OpKind::Rotate => {
                let axis = [v[0], v[1], v[2]];
                let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
                if !(len > 1e-12) {
                    if v[3] == 0.0 {
                        math::IDENTITY
                    } else {
                        return Err(malformed("a <rotate> has a zero axis and a non-zero angle"));
                    }
                } else {
                    math::quat_to_mat(math::quat_from_axis_angle(axis, v[3].to_radians()))
                }
            }
            OpKind::LookAt => look_at(v)?,
        };
        m = math::mat_mul(&m, &local);
    }
    Ok(m)
}

/// `<lookat>` eye, interest, up: the object placed at the eye with its −Z
/// toward the interest point and +Y toward up — the inverse of the classic
/// gluLookAt view matrix, which is how the spec defines it for cameras.
fn look_at(v: &[f64]) -> Result<Mat4, MeshError> {
    let eye = [v[0], v[1], v[2]];
    let f = [v[3] - eye[0], v[4] - eye[1], v[5] - eye[2]];
    let up = [v[6], v[7], v[8]];
    let norm = |a: [f64; 3]| -> Option<[f64; 3]> {
        let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
        (l > 1e-12 && l.is_finite()).then(|| [a[0] / l, a[1] / l, a[2] / l])
    };
    let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let f = norm(f).ok_or_else(|| malformed("a <lookat> looks at its own position"))?;
    let s = norm(cross(f, up)).ok_or_else(|| malformed("a <lookat> up vector is parallel to its view"))?;
    let u = cross(s, f);
    Ok([s[0], s[1], s[2], 0.0, u[0], u[1], u[2], 0.0, -f[0], -f[1], -f[2], 0.0, eye[0], eye[1], eye[2], 1.0])
}

// ─── reader: entry points ───────────────────────────────────────────────────

/// Read a COLLADA document into a scene. Images are looked up in `sidecars`.
pub fn read_scene(bytes: &[u8], sidecars: &dyn Sidecars) -> Result<SceneRead, MeshError> {
    if bytes.len() > super::MAX_INPUT_BYTES {
        return Err(unsupported(format!("input is {} bytes, over the {}-byte limit", bytes.len(), super::MAX_INPUT_BYTES)));
    }
    sniff(bytes)?;
    let root = super::xml::parse(bytes).map_err(|e| match e {
        MeshError::Malformed(d) => malformed(format!("the XML does not parse ({d})")),
        other => other,
    })?;
    if root.name != "COLLADA" {
        return Err(not_dae(format!("the XML root element is <{}>, not <COLLADA>", clip(&root.name, 60))));
    }
    if let Some(v) = root.attr("version") {
        if !(v.starts_with("1.4") || v.starts_with("1.5")) {
            return Err(unsupported(format!("COLLADA version {}; only 1.4 and 1.5 documents are read", q(v))));
        }
    }
    let doc = Doc::new(&root);
    let duplicates = doc.duplicate_ids;
    let mut r = Reader::new(doc, sidecars);
    if duplicates > 0 {
        r.warn(format!("{duplicates} element id(s) repeat; each reference resolves to the first"));
    }
    r.read_asset()?;
    r.read_nodes()?;
    r.assemble()?;
    r.lights_and_cameras()?;
    r.read_animations()?;
    r.apply_correction();
    let Reader { scene, warnings, .. } = r;
    scene.validate()?;
    if scene.drawable_count() == 0 {
        return Err(MeshError::Empty);
    }
    Ok(SceneRead { scene, warnings })
}

/// Read a COLLADA document into the flat mesh (see `pose::flatten`).
pub fn read(bytes: &[u8]) -> Result<Mesh, MeshError> {
    let r = read_scene(bytes, &NoSidecars)?;
    let mesh = pose::flatten(&r.scene)?;
    if mesh.indices.is_empty() {
        return Err(MeshError::Empty);
    }
    Ok(mesh)
}

// ─── writer ─────────────────────────────────────────────────────────────────

/// Unique, deterministic XML ids. Every id in the document comes from here,
/// including the ones derived from others (`…-positions`), so a node that
/// happens to be called `box-mesh-positions` cannot collide with a source.
struct Ids {
    used: HashSet<String>,
    /// Where each base's numbering resumes, so repeats cost one probe each
    /// rather than a count up from `-2`.
    next: HashMap<String, usize>,
}

impl Ids {
    /// An `xs:ID`-safe id from a free-text name: ASCII letters, digits, `_`
    /// and `-`, starting with a letter or `_`. ASCII only, because the joint
    /// ids double as `Name_array` tokens that every reader splits on
    /// whitespace and compares byte for byte; no `.`, because an id starts a
    /// target address, where `.` selects a member (1.4.1 "Address Syntax":
    /// `node/sid.X`).
    fn make(&mut self, base: &str, fallback: &str) -> String {
        let mut s: String = base
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') { c } else { '_' })
            .take(64)
            .collect();
        if s.is_empty() {
            s = fallback.to_string();
        }
        if !s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            s.insert(0, '_');
        }
        let mut candidate = s.clone();
        let mut n = self.next.get(&s).copied().unwrap_or(2);
        while !self.used.insert(candidate.clone()) {
            candidate = format!("{s}-{n}");
            n += 1;
        }
        self.next.insert(s, n);
        candidate
    }
}

/// A `name` attribute for a free-text name: 1.4.1 types every `name`
/// `xs:NCName` (1.5 relaxed it to free text), and readers act on it — SceneKit
/// cuts a name at its first space ("Left Arm" loads as "Left"), and OpenCOLLADA,
/// the Max/Maya exporter, writes NCNames for that reason. Every character an
/// NCName cannot hold becomes `_`; letters of every script survive (XML 1.0
/// 5th edition name characters, which libxml2's schema validation accepts).
/// `None` for an empty name: no attribute at all.
fn ncname(name: Option<&str>) -> Option<String> {
    let name = name.filter(|n| !n.is_empty())?;
    let start = |c: char| {
        matches!(c, 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
            | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
            | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
            | '\u{10000}'..='\u{EFFFF}')
    };
    let rest = |c: char| start(c) || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}');
    let mut out: String = name.chars().map(|c| if rest(c) { c } else { '_' }).collect();
    if !out.starts_with(start) {
        out.insert(0, '_');
    }
    Some(out)
}

/// Escape for attribute values and text. Tabs and line breaks become
/// character references: XML attribute-value normalisation would otherwise
/// turn a name's newline into a space on the way back in. Other control
/// characters cannot be carried by XML 1.0 at all and are dropped.
fn esc(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c if (c as u32) < 0x20 => {}
            // Outside XML 1.0's `Char` production (§2.2).
            '\u{FFFE}' | '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// An indenting XML writer. Text is written inline with no surrounding
/// whitespace: three.js takes `<init_from>`'s text content verbatim as a path.
struct Out {
    s: String,
    depth: usize,
}

/// Indentation stops deepening here, as USD's writer does: an 8k-node chain
/// indented in full was a 184 MB document.
const MAX_INDENT: usize = 48;

impl Out {
    fn indent(&mut self) {
        for _ in 0..self.depth.min(MAX_INDENT) {
            self.s.push_str("  ");
        }
    }

    fn tag(&mut self, tag: &str, attrs: &[(&str, &str)]) {
        self.s.push('<');
        self.s.push_str(tag);
        for (k, v) in attrs {
            let _ = write!(self.s, " {k}=\"{}\"", esc(v));
        }
    }

    fn open(&mut self, tag: &str, attrs: &[(&str, &str)]) {
        self.indent();
        self.tag(tag, attrs);
        self.s.push_str(">\n");
        self.depth += 1;
    }

    fn close(&mut self, tag: &str) {
        self.depth = self.depth.saturating_sub(1);
        self.indent();
        let _ = writeln!(self.s, "</{tag}>");
    }

    fn empty(&mut self, tag: &str, attrs: &[(&str, &str)]) {
        self.indent();
        self.tag(tag, attrs);
        self.s.push_str("/>\n");
    }

    /// `<tag>text</tag>`, with `text` escaped here.
    fn leaf(&mut self, tag: &str, attrs: &[(&str, &str)], text: &str) {
        self.indent();
        self.tag(tag, attrs);
        let _ = writeln!(self.s, ">{}</{tag}>", esc(text));
    }

    /// `<tag>numbers</tag>`, the numbers already formatted.
    fn numbers(&mut self, tag: &str, attrs: &[(&str, &str)], text: &str) {
        self.indent();
        self.tag(tag, attrs);
        let _ = writeln!(self.s, ">{text}</{tag}>");
    }
}

/// Space-separated shortest round-trip decimals. Rust's `Display` for floats
/// writes the shortest text that parses back to the same bits and never
/// uses exponents, both of which Collada's `xs:float` lists accept.
fn join_f32(vals: impl IntoIterator<Item = f32>) -> String {
    let mut s = String::new();
    for v in vals {
        if !s.is_empty() {
            s.push(' ');
        }
        let _ = write!(s, "{v}");
    }
    s
}

fn join_f64(vals: impl IntoIterator<Item = f64>) -> String {
    let mut s = String::new();
    for v in vals {
        if !s.is_empty() {
            s.push(' ');
        }
        let _ = write!(s, "{v}");
    }
    s
}

fn wrap_name(w: Wrap) -> &'static str {
    match w {
        Wrap::Repeat => "WRAP",
        Wrap::ClampToEdge => "CLAMP",
        Wrap::MirroredRepeat => "MIRROR",
    }
}

/// Key times for a baked matrix channel: every source key and 30 fps between
/// (`pose::resample_times`), plus a key just before each STEP key, holding
/// the old value, so that a reader interpolating linearly — three.js does on
/// every matrix channel — jumps where the source steps instead of ramping.
/// The weight channels `DaeMorphs::Keep` writes follow the same rules, and
/// get that key too when `weights` says they are written.
fn bake_times(scene: &Scene, anim: usize, fps: f32, weights: bool) -> Vec<f32> {
    let mut times = pose::resample_times(scene, anim, fps);
    if let Some(a) = scene.animations.get(anim) {
        for ch in a.channels.iter().filter(|c| c.interpolation == Interpolation::Step && (weights || c.path != AnimPath::Weights)) {
            for w in ch.times.windows(2) {
                let (t0, t1) = (w[0], w[1]);
                let eps = STEP_EPSILON.max(t1.abs() * 4.0 * f32::EPSILON).min((t1 - t0) / 2.0);
                let before = t1 - eps;
                if before > t0 && before < t1 {
                    times.push(before);
                }
            }
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    times.dedup();
    times
}

/// The morph controllers of one mesh at one set of default weights
/// (`DaeMorphs::Keep`): a controller per primitive, since one `<geometry>` is
/// one primitive. `<instance_controller>` has no weight override, so nodes
/// that draw a mesh at different weights get different sets; nodes that agree,
/// and that no clip animates, share one.
struct MorphSet {
    mesh: usize,
    /// The default weights, one per target.
    weights: Vec<f32>,
    /// The `<controller>` id of each primitive.
    ids: Vec<String>,
    /// The id of each controller's MORPH_WEIGHT `<source>` (not its
    /// `<float_array>`): the address a weight channel needs. `None` until the
    /// controller is written, so a weight channel written before it fails
    /// loudly rather than addressing nothing.
    weight_sources: Vec<Option<String>>,
}

struct Writer<'s> {
    s: &'s Scene,
    /// Package mode: textures go out as sidecar files.
    images: bool,
    /// The rate baked matrix channels are keyed at between source keys.
    fps: f32,
    /// `DaeMorphs::Keep`: morph targets are written as controllers, with
    /// their weight animation.
    morphs: bool,
    /// The morph controller sets, and the set each node draws through (none
    /// for a node whose mesh has no targets, and always when `!morphs`).
    sets: Vec<MorphSet>,
    node_set: Vec<Option<usize>>,
    /// Per mesh and primitive, the ids of the target `<geometry>`s: empty for
    /// a mesh no controller morphs. Shared by every set of the mesh.
    target_geom_ids: Vec<Vec<Vec<String>>>,
    /// Nodes for the second and later primitives of a morphed mesh with no
    /// skin (see `skin_part_ids`, which serves a skinned one).
    morph_part_ids: Vec<Vec<String>>,
    ids: Ids,
    out: Out,
    scene_id: String,
    node_ids: Vec<String>,
    /// A joint that holds a mesh draws it from a child node: three.js builds
    /// a leaf node with exactly one instance AS that mesh, not as a bone, and
    /// the skeleton would lose the joint.
    wrapper_ids: Vec<Option<String>>,
    /// Nodes for a skinned mesh's second and later skinned primitives:
    /// SceneKit's importer keeps one `<instance_controller>` per node and
    /// silently drops the rest (measured: a two-controller node loads with
    /// one skinned geometry). Skinning ignores the holding node's transform,
    /// so an identity child changes nothing for any reader.
    skin_part_ids: Vec<Vec<String>>,
    is_joint: Vec<bool>,
    image_ids: Vec<Option<String>>,
    image_paths: Vec<Option<String>>,
    material_ids: Vec<String>,
    effect_ids: Vec<String>,
    geom_ids: Vec<Vec<String>>,
    /// (mesh, skin, morph set) → a controller per primitive that carries
    /// weights. A skin over a morph is a controller for each morph set.
    controllers: BTreeMap<(usize, usize, Option<usize>), Vec<Option<String>>>,
    files: Vec<PackageFile>,
    /// Each scene light's `<light>` id, and each camera's `<camera>` id.
    light_ids: Vec<String>,
    camera_ids: Vec<String>,
}

impl<'s> Writer<'s> {
    fn new(s: &'s Scene, images: bool, fps: f32, morphs: bool) -> Writer<'s> {
        Writer {
            s,
            images,
            fps,
            morphs,
            sets: Vec::new(),
            node_set: Vec::new(),
            target_geom_ids: Vec::new(),
            morph_part_ids: Vec::new(),
            light_ids: Vec::new(),
            camera_ids: Vec::new(),
            ids: Ids { used: HashSet::new(), next: HashMap::new() },
            out: Out { s: String::new(), depth: 0 },
            scene_id: String::new(),
            node_ids: Vec::new(),
            wrapper_ids: Vec::new(),
            skin_part_ids: Vec::new(),
            is_joint: Vec::new(),
            image_ids: Vec::new(),
            image_paths: Vec::new(),
            material_ids: Vec::new(),
            effect_ids: Vec::new(),
            geom_ids: Vec::new(),
            controllers: BTreeMap::new(),
            files: Vec::new(),
        }
    }

    /// Allocate every library id up front, in a fixed order, so the same
    /// scene always gets the same ids and the nodes get first pick of their
    /// own names.
    ///
    /// Refuses a scene whose kept morph controllers pass the ceiling
    /// ([`Writer::check_controller_budget`]) before the other ids are
    /// allocated or anything is written.
    fn plan(&mut self) -> Result<(), MeshError> {
        let s = self.s;
        self.scene_id = self.ids.make("Scene", "Scene");
        self.is_joint = s.joint_nodes();
        self.plan_morph_sets();
        self.check_controller_budget()?;
        for (i, n) in s.nodes.iter().enumerate() {
            let id = self.ids.make(n.name.as_deref().unwrap_or(""), &format!("node{i}"));
            self.node_ids.push(id);
        }
        for (i, n) in s.nodes.iter().enumerate() {
            let w = (self.is_joint[i] && n.mesh.is_some())
                .then(|| self.ids.make(&format!("{}-geometry", self.node_ids[i]), "geometry"));
            self.wrapper_ids.push(w);
            let skinned = match (n.mesh, n.skin) {
                (Some(m), Some(_)) => s.meshes[m].primitives.iter().filter(|p| p.skin.is_some()).count(),
                _ => 0,
            };
            let parts = (1..skinned).map(|k| self.ids.make(&format!("{}-skin{k}", self.node_ids[i]), "skin_part")).collect();
            self.skin_part_ids.push(parts);
        }
        let mut used_paths: HashSet<String> = HashSet::new();
        for (i, im) in s.images.iter().enumerate() {
            if !self.images || !im.format.is_universal() {
                self.image_ids.push(None);
                self.image_paths.push(None);
                continue;
            }
            let stem = package::safe_file_stem(im.name.as_deref().unwrap_or(""), &format!("image_{i}"));
            let mut path = format!("{TEXTURE_DIR}/{stem}.{}", im.format.extension());
            let mut n = 2;
            // Folded: paths that differ only in case are one file on macOS and Windows.
            while !used_paths.insert(path.to_ascii_lowercase()) {
                path = format!("{TEXTURE_DIR}/{stem}_{n}.{}", im.format.extension());
                n += 1;
            }
            let id = self.ids.make(&format!("{stem}-image"), "image");
            self.image_ids.push(Some(id));
            self.image_paths.push(Some(path));
        }
        for (i, m) in s.materials.iter().enumerate() {
            let base = m.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("material{i}"));
            let mid = self.ids.make(&format!("{base}-material"), "material");
            let eid = self.ids.make(&format!("{base}-effect"), "effect");
            self.material_ids.push(mid);
            self.effect_ids.push(eid);
        }
        for (mi, m) in s.meshes.iter().enumerate() {
            let base = m.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("mesh{mi}"));
            let mut ids = Vec::with_capacity(m.primitives.len());
            for pi in 0..m.primitives.len() {
                let b = if m.primitives.len() > 1 { format!("{base}-{pi}") } else { base.clone() };
                ids.push(self.ids.make(&format!("{b}-mesh"), "mesh"));
            }
            self.geom_ids.push(ids);
        }
        for (ni, n) in s.nodes.iter().enumerate() {
            let (Some(mi), Some(si)) = (n.mesh, n.skin) else { continue };
            // A skin over a morph needs a controller for each morph set.
            let key = (mi, si, self.node_set[ni]);
            if self.controllers.contains_key(&key) {
                continue;
            }
            let per_prim = s.meshes[mi]
                .primitives
                .iter()
                .enumerate()
                .map(|(pi, p)| p.skin.as_ref().map(|_| self.ids.make(&format!("{}-skin", self.geom_ids[mi][pi]), "skin")))
                .collect();
            self.controllers.insert(key, per_prim);
        }
        // Last, so a scene with none keeps every other id it always had.
        for (i, l) in s.lights.iter().enumerate() {
            let base = l.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("light{i}"));
            let id = self.ids.make(&format!("{base}-light"), "light");
            self.light_ids.push(id);
        }
        for (i, c) in s.cameras.iter().enumerate() {
            let base = c.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("camera{i}"));
            let id = self.ids.make(&format!("{base}-camera"), "camera");
            self.camera_ids.push(id);
        }
        // After every other id, so a scene with no morph controller keeps all
        // the ids it always had.
        self.plan_morph_ids();
        Ok(())
    }

    /// What `DaeMorphs::Keep` writes for each node is its controllers, and a
    /// controller lists every target of its mesh twice: an `IDREF_array` of
    /// the target ids and a `float_array` of the weights. Sets scale with the
    /// nodes (a node at weights of its own, or one a clip animates, gets a set
    /// of its own), and nothing else bounds `sets x primitives x targets`, so
    /// the geometry check in `write_scene`, which counts the base geometries
    /// once, does not see them (Ruling DM17).
    ///
    /// Counted here, where the sets are planned and so the same sets that are
    /// written, and before the other ids are allocated or a byte written: each
    /// (set, primitive, target) is one vertex-equivalent, charged with the
    /// vertices of the meshes drawn against [`MAX_SCENE_VERTICES`], the
    /// ceiling `pose::check_copied_geometry` holds a scene's geometry to. A
    /// scene with no set (`Bake`, or no morphed node) is not asked anything.
    ///
    /// A skin over a morph is a controller of its own for each (mesh, skin,
    /// set) and skinned primitive, with the primitive's joint and weight
    /// arrays per vertex, and the skin's joint names and bind poses, so every
    /// one of them also charges its primitive's vertices and 17 for each joint
    /// of the skin (a name and sixteen bind-pose numbers, the same convention
    /// as the vertex), in the same refusal (Rulings DM22, DM23).
    ///
    /// What the sum holds, then, is every array a `Keep` controller repeats
    /// for each set, and the meshes' own vertices:
    /// - the vertices of each mesh a node draws, once (the base geometries);
    /// - for each set, for each primitive of its mesh, one for each target
    ///   (the morph controller's target reference and weight);
    /// - for each (mesh, skin, set) a node draws through, for each skinned
    ///   primitive: its vertices (the joint and weight arrays per vertex) and
    ///   17 for each joint of the skin (the joint names and the bind poses).
    ///
    /// The target geometries themselves are written once, however many sets
    /// there are, and are bounded by the scene's own validation.
    fn check_controller_budget(&self) -> Result<(), MeshError> {
        if self.sets.is_empty() {
            return Ok(());
        }
        let s = self.s;
        let mut seen = vec![false; s.meshes.len()];
        let mut vertices = 0usize;
        for mi in s.nodes.iter().filter_map(|n| n.mesh) {
            if let Some(first) = seen.get_mut(mi).filter(|seen| !**seen) {
                *first = true;
                vertices = s.meshes[mi].primitives.iter().fold(vertices, |v, p| v.saturating_add(p.positions.len()));
            }
        }
        let references = self.sets.iter().fold(0usize, |sum, set| {
            let mesh = &s.meshes[set.mesh];
            sum.saturating_add(mesh.primitives.len().saturating_mul(mesh.target_count()))
        });
        // The skin controllers over a morph: one for each skinned primitive of
        // each (mesh, skin, set) a node draws through.
        let skins: HashSet<(usize, usize, usize)> = s
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(ni, n)| Some((n.mesh?, n.skin?, self.node_set[ni]?)))
            .collect();
        let (mut skin_controllers, mut skin_vertices, mut skin_joints) = (0usize, 0usize, 0usize);
        for (mi, ski, _) in skins {
            let joints = s.skins.get(ski).map_or(0, |k| k.joints.len());
            for p in s.meshes[mi].primitives.iter().filter(|p| p.skin.is_some()) {
                skin_controllers = skin_controllers.saturating_add(1);
                skin_vertices = skin_vertices.saturating_add(p.positions.len());
                skin_joints = skin_joints.saturating_add(joints);
            }
        }
        // A joint is a name and sixteen bind-pose numbers.
        let joint_numbers = skin_joints.saturating_mul(17);
        if vertices.saturating_add(references).saturating_add(skin_vertices).saturating_add(joint_numbers) > MAX_SCENE_VERTICES {
            return Err(MeshError::Unsupported(format!(
                "written with its morph targets kept, {} nodes would draw through {} sets of morph weights, each \
                 controller listing every target of its mesh: {references} target references, and {skin_controllers} \
                 skin controllers over them ({skin_vertices} vertices and {skin_joints} joints, each joint a name \
                 and sixteen bind-pose numbers), which with the meshes' {vertices} vertices pass \
                 {MAX_SCENE_VERTICES}; the document would carry more controller data than this converter will \
                 write (baking the morphs writes none)",
                self.node_set.iter().flatten().count(),
                self.sets.len()
            )));
        }
        Ok(())
    }

    /// Which morph controllers each node draws through (`DaeMorphs::Keep`).
    /// A node's controllers are its mesh's, at the node's effective default
    /// weights, and nodes at the same weights share them, unless a clip
    /// animates a node's weights: a weight channel addresses its controller,
    /// so an animated node keeps its own. Allocates no ids (see
    /// [`Writer::plan_morph_ids`]).
    fn plan_morph_sets(&mut self) {
        let s = self.s;
        self.node_set = vec![None; s.nodes.len()];
        if !self.morphs {
            return;
        }
        let pose = pose::default_pose(s);
        let animated: HashSet<usize> =
            s.animations.iter().flat_map(|a| &a.channels).filter(|c| c.path == AnimPath::Weights).map(|c| c.node).collect();
        let mut by_key: HashMap<(usize, Vec<u32>, Option<usize>), usize> = HashMap::new();
        for (ni, n) in s.nodes.iter().enumerate() {
            let Some(mi) = n.mesh else { continue };
            if s.meshes[mi].target_count() == 0 {
                continue;
            }
            let weights = pose.weights.get(ni).cloned().unwrap_or_default();
            let key = (mi, weights.iter().map(|w| w.to_bits()).collect(), animated.contains(&ni).then_some(ni));
            let set = match by_key.get(&key) {
                Some(&set) => set,
                None => {
                    self.sets.push(MorphSet { mesh: mi, weights, ids: Vec::new(), weight_sources: Vec::new() });
                    by_key.insert(key, self.sets.len() - 1);
                    self.sets.len() - 1
                }
            };
            self.node_set[ni] = Some(set);
        }
    }

    /// The ids of the morph controllers, of the target geometries they share
    /// and of the nodes that draw a morphed mesh's later primitives. Allocated
    /// after every other id, so a scene with no morph controller keeps all the
    /// ids it always had.
    fn plan_morph_ids(&mut self) {
        let s = self.s;
        self.target_geom_ids = vec![Vec::new(); s.meshes.len()];
        self.morph_part_ids = vec![Vec::new(); s.nodes.len()];
        for mi in 0..s.meshes.len() {
            if !self.sets.iter().any(|set| set.mesh == mi) {
                continue;
            }
            for (pi, p) in s.meshes[mi].primitives.iter().enumerate() {
                let mut ids = Vec::with_capacity(p.targets.len());
                for k in 0..p.targets.len() {
                    ids.push(self.ids.make(&format!("{}-target{k}", self.geom_ids[mi][pi]), "target"));
                }
                self.target_geom_ids[mi].push(ids);
            }
        }
        for si in 0..self.sets.len() {
            let mi = self.sets[si].mesh;
            let primitives = s.meshes[mi].primitives.len();
            for pi in 0..primitives {
                let id = self.ids.make(&format!("{}-morph", self.geom_ids[mi][pi]), "morph");
                self.sets[si].ids.push(id);
            }
            self.sets[si].weight_sources = vec![None; primitives];
        }
        for (ni, n) in s.nodes.iter().enumerate() {
            let (Some(_), None, Some(set)) = (n.mesh, n.skin, self.node_set[ni]) else { continue };
            for k in 1..self.sets[set].ids.len() {
                let id = self.ids.make(&format!("{}-morph{k}", self.node_ids[ni]), "morph_part");
                self.morph_part_ids[ni].push(id);
            }
        }
    }

    fn document(mut self) -> Result<Package, MeshError> {
        self.plan()?;
        self.out.s.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
        self.out.open("COLLADA", &[("xmlns", NS_COLLADA_141), ("version", "1.4.1")]);
        self.asset();
        if !self.s.cameras.is_empty() {
            self.library_cameras();
        }
        if !self.s.lights.is_empty() {
            self.library_lights();
        }
        if self.image_ids.iter().any(Option::is_some) {
            self.library_images();
        }
        if !self.s.materials.is_empty() {
            self.library_effects();
            self.library_materials();
        }
        self.library_geometries();
        if !self.sets.is_empty() || self.controllers.values().flatten().any(Option::is_some) {
            self.library_controllers();
        }
        let clips = self.library_animations();
        if !clips.is_empty() {
            self.library_animation_clips(&clips);
        }
        self.library_visual_scenes();
        self.out.open("scene", &[]);
        let url = format!("#{}", self.scene_id);
        self.out.empty("instance_visual_scene", &[("url", &url)]);
        self.out.close("scene");
        self.out.close("COLLADA");
        let mut pkg = Package::single(PRIMARY_FILE, std::mem::take(&mut self.out.s).into_bytes());
        pkg.files.append(&mut self.files);
        Ok(pkg)
    }

    fn asset(&mut self) {
        let o = &mut self.out;
        o.open("asset", &[]);
        o.open("contributor", &[]);
        o.leaf("authoring_tool", &[], AUTHORING_TOOL);
        o.close("contributor");
        o.leaf("created", &[], TIMESTAMP);
        o.leaf("modified", &[], TIMESTAMP);
        o.empty("unit", &[("name", "meter"), ("meter", "1")]);
        o.leaf("up_axis", &[], "Y_UP");
        o.close("asset");
    }

    fn library_images(&mut self) {
        self.out.open("library_images", &[]);
        for (i, im) in self.s.images.iter().enumerate() {
            let (Some(id), Some(path)) = (self.image_ids[i].clone(), self.image_paths[i].clone()) else { continue };
            let mut attrs = vec![("id", id.as_str())];
            let name_text = ncname(im.name.as_deref());
            if let Some(n) = name_text.as_deref() {
                attrs.push(("name", n));
            }
            self.out.open("image", &attrs);
            self.out.leaf("init_from", &[], &path);
            self.out.close("image");
            self.files.push(PackageFile { path, bytes: im.data.clone() });
        }
        self.out.close("library_images");
    }

    /// The image id a texture's sampler can name, when this package carries
    /// that image.
    fn texture_image(&self, texture: usize) -> Option<&str> {
        let t = self.s.textures.get(texture)?;
        self.image_ids.get(t.image)?.as_deref()
    }

    fn library_effects(&mut self) {
        self.out.open("library_effects", &[]);
        for mi in 0..self.s.materials.len() {
            self.effect(mi);
        }
        self.out.close("library_effects");
    }

    fn effect(&mut self, mi: usize) {
        let m = &self.s.materials[mi];
        let eid = self.effect_ids[mi].clone();
        let mut attrs = vec![("id", eid.as_str())];
        let name_text = ncname(m.name.as_deref());
        if let Some(n) = name_text.as_deref() {
            attrs.push(("name", n));
        }
        self.out.open("effect", &attrs);
        self.out.open("profile_COMMON", &[]);
        // One surface and sampler per texture the effect samples.
        let mut samplers: Vec<(usize, String)> = Vec::new();
        for r in [&m.base_color_texture, &m.emissive_texture].into_iter().flatten() {
            if samplers.iter().any(|(t, _)| *t == r.texture) {
                continue;
            }
            let Some(image) = self.texture_image(r.texture).map(str::to_string) else { continue };
            let k = samplers.len();
            let surface = format!("texture{k}-surface");
            let sampler = format!("texture{k}-sampler");
            let sm = &self.s.textures[r.texture].sampler;
            self.out.open("newparam", &[("sid", &surface)]);
            self.out.open("surface", &[("type", "2D")]);
            self.out.leaf("init_from", &[], &image);
            self.out.close("surface");
            self.out.close("newparam");
            self.out.open("newparam", &[("sid", &sampler)]);
            self.out.open("sampler2D", &[]);
            self.out.leaf("source", &[], &surface);
            self.out.leaf("wrap_s", &[], wrap_name(sm.wrap_s));
            self.out.leaf("wrap_t", &[], wrap_name(sm.wrap_t));
            if let Some(f) = sm.min_filter {
                let name = match (f, sm.mipmaps) {
                    (Filter::Nearest, false) => "NEAREST",
                    (Filter::Linear, false) => "LINEAR",
                    (Filter::Nearest, true) if sm.mipmap_filter == Filter::Nearest => "NEAREST_MIPMAP_NEAREST",
                    (Filter::Linear, true) if sm.mipmap_filter == Filter::Nearest => "LINEAR_MIPMAP_NEAREST",
                    (Filter::Nearest, true) => "NEAREST_MIPMAP_LINEAR",
                    (Filter::Linear, true) => "LINEAR_MIPMAP_LINEAR",
                };
                self.out.leaf("minfilter", &[], name);
            }
            if let Some(f) = sm.mag_filter {
                self.out.leaf("magfilter", &[], if f == Filter::Nearest { "NEAREST" } else { "LINEAR" });
            }
            self.out.close("sampler2D");
            self.out.close("newparam");
            samplers.push((r.texture, sampler));
        }
        let sampler_for = |r: &Option<TextureRef>| -> Option<(String, u32, usize)> {
            let r = r.as_ref()?;
            samplers.iter().find(|(t, _)| *t == r.texture).map(|(_, s)| (s.clone(), r.uv_set, r.texture))
        };
        let diffuse_tex = sampler_for(&m.base_color_texture);
        let emissive_tex = sampler_for(&m.emissive_texture);
        self.out.open("technique", &[("sid", "common")]);
        let shader = if m.roughness >= 1.0 { "lambert" } else { "phong" };
        self.out.open(shader, &[]);
        self.out.open("emission", &[]);
        match &emissive_tex {
            Some(t) => self.texture_element(t),
            None => {
                let e = m.emissive.map(srgb_encode);
                self.out.numbers("color", &[("sid", "emission")], &join_f32([e[0], e[1], e[2], 1.0]));
            }
        }
        self.out.close("emission");
        self.out.open("diffuse", &[]);
        match &diffuse_tex {
            Some(t) => self.texture_element(t),
            None => {
                let c = [srgb_encode(m.base_color[0]), srgb_encode(m.base_color[1]), srgb_encode(m.base_color[2]), 1.0];
                self.out.numbers("color", &[("sid", "diffuse")], &join_f32(c));
            }
        }
        self.out.close("diffuse");
        if shader == "phong" {
            // A dielectric's 4% reflectance; the scene's metallic does not
            // survive a Phong model (and is reported by `scene::losses`).
            let k = srgb_encode(0.04);
            self.out.open("specular", &[]);
            self.out.numbers("color", &[("sid", "specular")], &join_f32([k, k, k, 1.0]));
            self.out.close("specular");
            self.out.open("shininess", &[]);
            self.out.numbers("float", &[("sid", "shininess")], &join_f32([shininess_from_roughness(m.roughness)]));
            self.out.close("shininess");
        }
        if m.alpha_mode == AlphaMode::Blend {
            // A_ONE: opacity = transparent.a × transparency (spec §8); with a
            // texture, its alpha is the transparent channel.
            self.out.open("transparent", &[("opaque", "A_ONE")]);
            match &diffuse_tex {
                Some(t) => self.texture_element(t),
                None => self.out.numbers("color", &[], "1 1 1 1"),
            }
            self.out.close("transparent");
            self.out.open("transparency", &[]);
            self.out.numbers("float", &[("sid", "transparency")], &join_f32([m.base_color[3]]));
            self.out.close("transparency");
        }
        // Last of a lambert's or a phong's children in the 1.4.1 schema.
        if let Some(ior) = m.ior {
            self.out.open("index_of_refraction", &[]);
            self.out.numbers("float", &[("sid", "index_of_refraction")], &join_f32([ior]));
            self.out.close("index_of_refraction");
        }
        self.out.close(shader);
        if m.double_sided {
            // Read by three.js (`technique.extra.technique.double_sided`) and
            // assimp; GOOGLEEARTH is the profile SketchUp and Blender wrote.
            self.out.open("extra", &[]);
            self.out.open("technique", &[("profile", "GOOGLEEARTH")]);
            self.out.numbers("double_sided", &[], "1");
            self.out.close("technique");
            self.out.close("extra");
        }
        self.out.close("technique");
        self.out.close("profile_COMMON");
        self.out.close("effect");
    }

    /// `<texture>` naming a sampler and its coordinate set, with the MAYA
    /// wrap flags when the wrap is not plain repeat (see [`capabilities`]).
    fn texture_element(&mut self, (sampler, uv_set, texture): &(String, u32, usize)) {
        let texcoord = format!("UVSET{uv_set}");
        let sm = &self.s.textures[*texture].sampler;
        if sm.wrap_s == Wrap::Repeat && sm.wrap_t == Wrap::Repeat {
            self.out.empty("texture", &[("texture", sampler), ("texcoord", &texcoord)]);
            return;
        }
        let flag = |b: bool| if b { "1" } else { "0" };
        self.out.open("texture", &[("texture", sampler), ("texcoord", &texcoord)]);
        self.out.open("extra", &[]);
        self.out.open("technique", &[("profile", "MAYA")]);
        self.out.numbers("wrapU", &[], flag(sm.wrap_s != Wrap::ClampToEdge));
        self.out.numbers("wrapV", &[], flag(sm.wrap_t != Wrap::ClampToEdge));
        self.out.numbers("mirrorU", &[], flag(sm.wrap_s == Wrap::MirroredRepeat));
        self.out.numbers("mirrorV", &[], flag(sm.wrap_t == Wrap::MirroredRepeat));
        self.out.close("technique");
        self.out.close("extra");
        self.out.close("texture");
    }

    fn library_materials(&mut self) {
        self.out.open("library_materials", &[]);
        for (mi, m) in self.s.materials.iter().enumerate() {
            let mid = self.material_ids[mi].clone();
            let mut attrs = vec![("id", mid.as_str())];
            let name_text = ncname(m.name.as_deref());
            if let Some(n) = name_text.as_deref() {
                attrs.push(("name", n));
            }
            self.out.open("material", &attrs);
            let url = format!("#{}", self.effect_ids[mi]);
            self.out.empty("instance_effect", &[("url", &url)]);
            self.out.close("material");
        }
        self.out.close("library_materials");
    }

    /// A `<source>` with its array and accessor; returns the source id.
    #[allow(clippy::too_many_arguments)]
    fn source(&mut self, base: &str, array: &str, text: &str, values: usize, count: usize, stride: usize, params: &[(&str, &str)]) -> String {
        let sid = self.ids.make(base, "source");
        let aid = self.ids.make(&format!("{sid}-array"), "array");
        self.out.open("source", &[("id", &sid)]);
        self.out.numbers(array, &[("id", &aid), ("count", &values.to_string())], text);
        self.out.open("technique_common", &[]);
        let url = format!("#{aid}");
        self.out.open("accessor", &[("source", &url), ("count", &count.to_string()), ("stride", &stride.to_string())]);
        for (name, ty) in params {
            self.out.empty("param", &[("name", name), ("type", ty)]);
        }
        self.out.close("accessor");
        self.out.close("technique_common");
        self.out.close("source");
        sid
    }

    fn library_geometries(&mut self) {
        self.out.open("library_geometries", &[]);
        for mi in 0..self.s.meshes.len() {
            for pi in 0..self.s.meshes[mi].primitives.len() {
                self.geometry(mi, pi);
            }
        }
        self.out.close("library_geometries");
    }

    /// A primitive's `<geometry>`, and, when a controller morphs it
    /// (`DaeMorphs::Keep`), the geometry of each of its targets.
    fn geometry(&mut self, mi: usize, pi: usize) {
        let s = self.s;
        let mesh = &s.meshes[mi];
        let p = &mesh.primitives[pi];
        let gid = self.geom_ids[mi][pi].clone();
        let name_text = ncname(mesh.name.as_deref());
        // A morphed geometry carries its normals inside `<vertices>`, in the
        // base and in every target: the reader takes normal offsets from
        // nowhere else, and SceneKit morphs them (module docs).
        let morphed = !self.target_geom_ids[mi].is_empty();
        self.geometry_element(&gid, name_text.as_deref(), p, &p.positions, p.normals.as_deref(), morphed, true);
        if !morphed {
            return;
        }
        for (k, t) in p.targets.iter().enumerate() {
            // NORMALIZED blends absolute geometries, so the target is the base
            // moved by its offsets. A target with no normal offsets still
            // carries normals, the base's own: it must not blend toward none.
            let positions: Vec<[f32; 3]> =
                p.positions.iter().zip(&t.positions).map(|(b, o)| [b[0] + o[0], b[1] + o[1], b[2] + o[2]]).collect();
            let normals: Option<Vec<[f32; 3]>> = p.normals.as_ref().map(|base| {
                base.iter()
                    .enumerate()
                    .map(|(i, b)| {
                        let o = t.normals.as_ref().map_or([0.0; 3], |n| n[i]);
                        [b[0] + o[0], b[1] + o[1], b[2] + o[2]]
                    })
                    .collect()
            });
            let tid = self.target_geom_ids[mi][pi][k].clone();
            let tname = ncname(mesh.target_names.get(k).map(String::as_str));
            self.geometry_element(&tid, tname.as_deref(), p, &positions, normals.as_deref(), true, false);
        }
    }

    /// One `<geometry>` over `p`'s indices and material, holding `positions_in`
    /// and `normals_in`. `base` is the primitive itself, which also carries its
    /// uv sets and colours; a morph target is positions and normals alone.
    /// `normals_in_vertices` puts the `NORMAL` input inside `<vertices>`
    /// instead of on the primitive element.
    #[allow(clippy::too_many_arguments)]
    fn geometry_element(
        &mut self,
        gid: &str,
        name: Option<&str>,
        p: &Primitive,
        positions_in: &[[f32; 3]],
        normals_in: Option<&[[f32; 3]]>,
        normals_in_vertices: bool,
        base: bool,
    ) {
        let mut attrs = vec![("id", gid)];
        if let Some(n) = name {
            attrs.push(("name", n));
        }
        self.out.open("geometry", &attrs);
        self.out.open("mesh", &[]);
        let n = positions_in.len();
        let xyz = [("X", "float"), ("Y", "float"), ("Z", "float")];
        let positions = self.source(&format!("{gid}-positions"), "float_array", &join_f32(positions_in.iter().flatten().copied()), n * 3, n, 3, &xyz);
        let normals = normals_in
            .map(|ns| self.source(&format!("{gid}-normals"), "float_array", &join_f32(ns.iter().flatten().copied()), n * 3, n, 3, &xyz));
        let mut uv_sources = Vec::new();
        for (set, uvs) in p.uvs.iter().enumerate().take(if base { UV_SETS } else { 0 }) {
            // V flipped to Collada's bottom-left origin. Both values in f64:
            // the f32 `u` exactly, and `1 - v` exactly, so the reader's
            // `1 - (1 - v)` restores v to the bit.
            let text = join_f64(uvs.iter().flat_map(|t| [t[0] as f64, 1.0 - t[1] as f64]));
            let st = [("S", "float"), ("T", "float")];
            uv_sources.push((set, self.source(&format!("{gid}-map-{set}"), "float_array", &text, n * 2, n, 2, &st)));
        }
        let colors = p.colors.as_ref().filter(|_| base).map(|cs| {
            let rgba = [("R", "float"), ("G", "float"), ("B", "float"), ("A", "float")];
            self.source(&format!("{gid}-colors"), "float_array", &join_f32(cs.iter().flatten().copied()), n * 4, n, 4, &rgba)
        });
        let vid = self.ids.make(&format!("{gid}-vertices"), "vertices");
        self.out.open("vertices", &[("id", &vid)]);
        self.out.empty("input", &[("semantic", "POSITION"), ("source", &format!("#{positions}"))]);
        if let Some(nid) = normals.as_ref().filter(|_| normals_in_vertices) {
            self.out.empty("input", &[("semantic", "NORMAL"), ("source", &format!("#{nid}"))]);
        }
        if let Some(c) = &colors {
            // In <vertices>: the one place three.js reads colours unconverted
            // (see the module docs).
            self.out.empty("input", &[("semantic", "COLOR"), ("source", &format!("#{c}"))]);
        }
        self.out.close("vertices");
        // Triangles, or `<lines>`: a pair of corners a segment, never
        // `<linestrips>` (see [`capabilities`]). A points primitive has no
        // element in Collada 1.4.1, and `prepare` removed it; one that comes
        // anyway (a release build, which does not assert) leaves its geometry
        // drawing nothing.
        let element = match p.topology {
            Topology::Triangles => Some(("triangles", 3)),
            Topology::Lines => Some(("lines", 2)),
            Topology::Points => None,
        };
        if let Some((element, size)) = element {
            let count = (p.indices.len() / size).to_string();
            let mut tattrs = vec![("count", count.as_str())];
            let symbol = p.material.map(|m| self.material_ids[m].clone());
            if let Some(sym) = &symbol {
                tattrs.push(("material", sym));
            }
            self.out.open(element, &tattrs);
            self.out.empty("input", &[("semantic", "VERTEX"), ("source", &format!("#{vid}")), ("offset", "0")]);
            if let Some(nid) = normals.as_ref().filter(|_| !normals_in_vertices) {
                self.out.empty("input", &[("semantic", "NORMAL"), ("source", &format!("#{nid}")), ("offset", "0")]);
            }
            for (set, sid) in &uv_sources {
                self.out.empty(
                    "input",
                    &[("semantic", "TEXCOORD"), ("source", &format!("#{sid}")), ("offset", "0"), ("set", &set.to_string())],
                );
            }
            let mut idx = String::with_capacity(p.indices.len() * 6);
            for (k, i) in p.indices.iter().enumerate() {
                if k > 0 {
                    idx.push(' ');
                }
                let _ = write!(idx, "{i}");
            }
            self.out.numbers("p", &[], &idx);
            self.out.close(element);
        }
        self.out.close("mesh");
        self.out.close("geometry");
    }

    fn library_controllers(&mut self) {
        self.out.open("library_controllers", &[]);
        // The morphs first, as in the files SceneKit was measured on: a skin
        // over one names it.
        for set in 0..self.sets.len() {
            for pi in 0..self.sets[set].ids.len() {
                self.morph_controller(set, pi);
            }
        }
        let entries: Vec<(usize, usize, Option<usize>, Vec<Option<String>>)> =
            self.controllers.iter().map(|((m, s, set), v)| (*m, *s, *set, v.clone())).collect();
        for (mi, si, set, per_prim) in entries {
            for (pi, cid) in per_prim.into_iter().enumerate() {
                if let Some(cid) = cid {
                    self.controller(&cid, mi, pi, si, set);
                }
            }
        }
        self.out.close("library_controllers");
    }

    /// A morph controller (`DaeMorphs::Keep`) in the form SceneKit was
    /// measured playing (module docs): NORMALIZED, over the primitive's
    /// geometry, with each target's geometry holding absolute positions and
    /// normals, and the set's default weights.
    fn morph_controller(&mut self, set: usize, pi: usize) {
        let s = self.s;
        let mi = self.sets[set].mesh;
        let cid = self.sets[set].ids[pi].clone();
        let mesh = &s.meshes[mi];
        let count = mesh.target_count();
        let gid = self.geom_ids[mi][pi].clone();
        let mut attrs = vec![("id", cid.as_str())];
        let name_text = ncname(mesh.name.as_deref());
        if let Some(n) = name_text.as_deref() {
            attrs.push(("name", n));
        }
        self.out.open("controller", &attrs);
        self.out.open("morph", &[("source", &format!("#{gid}")), ("method", "NORMALIZED")]);
        let targets = self.target_geom_ids[mi][pi].join(" ");
        let targets = self.source(&format!("{cid}-targets"), "IDREF_array", &targets, count, count, 1, &[("IDREF", "IDREF")]);
        let defaults = join_f32(self.sets[set].weights.iter().copied());
        let weights = self.source(&format!("{cid}-weights"), "float_array", &defaults, count, count, 1, &[("MORPH_WEIGHT", "float")]);
        self.out.open("targets", &[]);
        self.out.empty("input", &[("semantic", "MORPH_TARGET"), ("source", &format!("#{targets}"))]);
        self.out.empty("input", &[("semantic", "MORPH_WEIGHT"), ("source", &format!("#{weights}"))]);
        self.out.close("targets");
        self.out.close("morph");
        self.out.close("controller");
        self.sets[set].weight_sources[pi] = Some(weights);
    }

    fn controller(&mut self, cid: &str, mi: usize, pi: usize, si: usize, set: Option<usize>) {
        let s = self.s;
        let p = &s.meshes[mi].primitives[pi];
        let skin = &s.skins[si];
        let Some(sw) = &p.skin else { return };
        let gid = self.geom_ids[mi][pi].clone();
        let mut attrs = vec![("id", cid)];
        let name_text = ncname(skin.name.as_deref());
        if let Some(n) = name_text.as_deref() {
            attrs.push(("name", n));
        }
        self.out.open("controller", &attrs);
        // Over the primitive's geometry, or over the morph that deforms it.
        let source = match set {
            Some(k) => self.sets[k].ids[pi].clone(),
            None => gid,
        };
        self.out.open("skin", &[("source", &format!("#{source}"))]);
        self.out.numbers("bind_shape_matrix", &[], &join_f64(row_major(&math::IDENTITY)));
        let names: Vec<&str> = skin.joints.iter().map(|j| self.node_ids[*j].as_str()).collect();
        let names = names.join(" ");
        let jn = skin.joints.len();
        let joints = self.source(&format!("{cid}-joints"), "Name_array", &names, jn, jn, 1, &[("JOINT", "name")]);
        let ibm_text = join_f64(skin.inverse_bind_matrices.iter().flat_map(row_major));
        let ibms = self.source(&format!("{cid}-bind_poses"), "float_array", &ibm_text, jn * 16, jn, 16, &[("TRANSFORM", "float4x4")]);
        // Influences: positive weights, normalised, one weight entry each. A
        // vertex with none is bound wholly to its first listed joint — what
        // the scene's skinning does with it (`pose::skin_in_place`).
        let mut vcount = String::new();
        let mut v = String::new();
        let mut weights: Vec<f32> = Vec::new();
        for vi in 0..p.positions.len() {
            let (js, ws) = sw.vertex(vi);
            let sum: f64 = ws.iter().filter(|w| **w > 0.0).map(|w| *w as f64).sum();
            let mut infl: Vec<(u16, f32)> = Vec::new();
            if sum > 0.0 && sum.is_finite() {
                for (j, w) in js.iter().zip(ws) {
                    if *w > 0.0 {
                        infl.push((*j, (*w as f64 / sum) as f32));
                    }
                }
            } else {
                infl.push((js.first().copied().unwrap_or(0), 1.0));
            }
            if !vcount.is_empty() {
                vcount.push(' ');
            }
            let _ = write!(vcount, "{}", infl.len());
            for (j, w) in infl {
                if !v.is_empty() {
                    v.push(' ');
                }
                let _ = write!(v, "{j} {}", weights.len());
                weights.push(w);
            }
        }
        let wn = weights.len();
        let wsrc = self.source(&format!("{cid}-weights"), "float_array", &join_f32(weights), wn, wn, 1, &[("WEIGHT", "float")]);
        self.out.open("joints", &[]);
        self.out.empty("input", &[("semantic", "JOINT"), ("source", &format!("#{joints}"))]);
        self.out.empty("input", &[("semantic", "INV_BIND_MATRIX"), ("source", &format!("#{ibms}"))]);
        self.out.close("joints");
        self.out.open("vertex_weights", &[("count", &p.positions.len().to_string())]);
        self.out.empty("input", &[("semantic", "JOINT"), ("source", &format!("#{joints}")), ("offset", "0")]);
        self.out.empty("input", &[("semantic", "WEIGHT"), ("source", &format!("#{wsrc}")), ("offset", "1")]);
        self.out.numbers("vcount", &[], &vcount);
        self.out.numbers("v", &[], &v);
        self.out.close("vertex_weights");
        self.out.close("skin");
        self.out.close("controller");
    }

    /// One `<animation>` per animated node and clip, each a baked matrix
    /// channel; returns, per clip, the animations it groups.
    fn library_animations(&mut self) -> Vec<(usize, Vec<String>)> {
        let s = self.s;
        let mut clips = Vec::new();
        let mut open = false;
        for (ai, a) in s.animations.iter().enumerate() {
            let nodes: BTreeSet<usize> =
                a.channels.iter().filter(|c| c.path != AnimPath::Weights).map(|c| c.node).collect();
            // `Keep` writes the weight channels as well, which makes a clip
            // of weights alone worth writing.
            let weight_nodes: BTreeSet<usize> = if self.morphs {
                a.channels.iter().filter(|c| c.path == AnimPath::Weights).map(|c| c.node).collect()
            } else {
                BTreeSet::new()
            };
            if nodes.is_empty() && weight_nodes.is_empty() {
                continue;
            }
            if !open {
                self.out.open("library_animations", &[]);
                open = true;
            }
            let times = bake_times(s, ai, self.fps, self.morphs);
            let mut mats: Vec<Vec<f64>> = vec![Vec::with_capacity(times.len() * 16); nodes.len()];
            // Each weight node's weights at every time, `targets` to a time.
            let mut weights: Vec<Vec<f32>> = vec![Vec::new(); weight_nodes.len()];
            for &t in &times {
                let pose = pose::sample_pose(s, ai, t);
                for (k, n) in weight_nodes.iter().enumerate() {
                    let at = pose.weights.get(*n);
                    let targets = s.nodes[*n].mesh.map_or(0, |m| s.meshes[m].target_count());
                    weights[k].extend((0..targets).map(|i| at.and_then(|w| w.get(i)).copied().unwrap_or(0.0)));
                }
                for (k, n) in nodes.iter().enumerate() {
                    let m = match pose.trs.get(*n).copied().flatten() {
                        // three.js decomposes every matrix key, dividing by
                        // each axis' length: an exactly-zero scale (a part
                        // scaled in from nothing) becomes a NaN rotation and
                        // the part vanishes for good. A millionth renders the
                        // same and decomposes.
                        Some((tr, rot, sc)) => math::compose(tr, rot, math::nonsingular_scale(sc)),
                        None => pose.matrices.get(*n).copied().unwrap_or(math::IDENTITY),
                    };
                    mats[k].extend(row_major(&m));
                }
            }
            let mut ids = Vec::new();
            for (k, n) in nodes.iter().enumerate() {
                let node_id = self.node_ids[*n].clone();
                let aid = self.ids.make(&format!("{node_id}-anim"), "anim");
                self.out.open("animation", &[("id", &aid)]);
                let count = times.len();
                let input = self.source(&format!("{aid}-input"), "float_array", &join_f32(times.iter().copied()), count, count, 1, &[("TIME", "float")]);
                let output = self.source(
                    &format!("{aid}-output"),
                    "float_array",
                    &join_f64(mats[k].iter().copied()),
                    count * 16,
                    count,
                    16,
                    &[("TRANSFORM", "float4x4")],
                );
                let interp_text = vec!["LINEAR"; count].join(" ");
                let interp = self.source(&format!("{aid}-interpolation"), "Name_array", &interp_text, count, count, 1, &[("INTERPOLATION", "name")]);
                let sampler = self.ids.make(&format!("{aid}-sampler"), "sampler");
                self.out.open("sampler", &[("id", &sampler)]);
                self.out.empty("input", &[("semantic", "INPUT"), ("source", &format!("#{input}"))]);
                self.out.empty("input", &[("semantic", "OUTPUT"), ("source", &format!("#{output}"))]);
                self.out.empty("input", &[("semantic", "INTERPOLATION"), ("source", &format!("#{interp}"))]);
                self.out.close("sampler");
                self.out.empty("channel", &[("source", &format!("#{sampler}")), ("target", &format!("{node_id}/transform"))]);
                self.out.close("animation");
                ids.push(aid);
            }
            for (k, n) in weight_nodes.iter().enumerate() {
                self.weight_animations(*n, &times, &weights[k], &mut ids);
            }
            clips.push((ai, ids));
        }
        if open {
            self.out.close("library_animations");
        }
        clips
    }

    /// The weight animation of the morph controllers node `n` draws
    /// (`DaeMorphs::Keep`): per controller (one a primitive) and per weight,
    /// one `<animation>` of a single sampler and channel, as in the files
    /// SceneKit was measured playing. `values` holds `times.len()` weights of
    /// every target, sampled as the matrix channels are, and written LINEAR.
    /// The channel addresses the weight as `<MORPH_WEIGHT source id>(i)`:
    /// SceneKit plays a channel that names the source and nothing for one
    /// that names the `<float_array>` inside it (module docs).
    ///
    /// That is `primitives x targets x 3` values at every time, which is what
    /// `write_scene`'s resample budget charges a weight channel under `Keep`.
    fn weight_animations(&mut self, n: usize, times: &[f32], values: &[f32], ids: &mut Vec<String>) {
        let Some(set) = self.node_set.get(n).copied().flatten() else { return };
        let targets = self.s.nodes[n].mesh.map_or(0, |m| self.s.meshes[m].target_count());
        let count = times.len();
        // The same text for every primitive and target, built once: the
        // times and the word LINEAR, and each target's weights.
        let times_text = join_f32(times.iter().copied());
        let interp_text = vec!["LINEAR"; count].join(" ");
        let weights_text: Vec<String> = (0..targets).map(|i| join_f32((0..count).map(|k| values[k * targets + i]))).collect();
        for pi in 0..self.sets[set].ids.len() {
            let cid = self.sets[set].ids[pi].clone();
            let weight_source = self.sets[set].weight_sources[pi]
                .clone()
                .expect("a morph controller is written before the animation that addresses its weights");
            for (i, weights) in weights_text.iter().enumerate() {
                let aid = self.ids.make(&format!("{cid}-w{i}-anim"), "anim");
                self.out.open("animation", &[("id", &aid)]);
                let input = self.source(&format!("{aid}-input"), "float_array", &times_text, count, count, 1, &[("TIME", "float")]);
                let output = self.source(&format!("{aid}-output"), "float_array", weights, count, count, 1, &[("WEIGHT", "float")]);
                let interp = self.source(&format!("{aid}-interpolation"), "Name_array", &interp_text, count, count, 1, &[("INTERPOLATION", "name")]);
                let sampler = self.ids.make(&format!("{aid}-sampler"), "sampler");
                self.out.open("sampler", &[("id", &sampler)]);
                self.out.empty("input", &[("semantic", "INPUT"), ("source", &format!("#{input}"))]);
                self.out.empty("input", &[("semantic", "OUTPUT"), ("source", &format!("#{output}"))]);
                self.out.empty("input", &[("semantic", "INTERPOLATION"), ("source", &format!("#{interp}"))]);
                self.out.close("sampler");
                self.out.empty("channel", &[("source", &format!("#{sampler}")), ("target", &format!("{weight_source}({i})"))]);
                self.out.close("animation");
                ids.push(aid);
            }
        }
    }

    fn library_animation_clips(&mut self, clips: &[(usize, Vec<String>)]) {
        self.out.open("library_animation_clips", &[]);
        for (ai, ids) in clips {
            let a = &self.s.animations[*ai];
            let (lo, hi) = a.time_range().unwrap_or((0.0, 0.0));
            let cid = self.ids.make(a.name.as_deref().unwrap_or(""), &format!("animation{ai}"));
            // The clip starts where playback does, at 0, not at its first key:
            // three.js takes `end - start` as the clip's duration
            // (ColladaComposer.buildAnimationClip) and clamps playback there,
            // so a clip whose keys begin at 1/24 s (CesiumMan) would stop
            // 1/24 s short of its last key.
            let (start, end) = (lo.min(0.0).to_string(), hi.to_string());
            let mut attrs = vec![("id", cid.as_str()), ("start", start.as_str()), ("end", end.as_str())];
            let name_text = ncname(a.name.as_deref());
            if let Some(n) = name_text.as_deref() {
                attrs.push(("name", n));
            }
            self.out.open("animation_clip", &attrs);
            for id in ids {
                self.out.empty("instance_animation", &[("url", &format!("#{id}"))]);
            }
            self.out.close("animation_clip");
        }
        self.out.close("library_animation_clips");
    }

    fn library_visual_scenes(&mut self) {
        let s = self.s;
        self.out.open("library_visual_scenes", &[]);
        let sid = self.scene_id.clone();
        let mut attrs = vec![("id", sid.as_str())];
        let name_text = ncname(s.name.as_deref());
        if let Some(n) = name_text.as_deref() {
            attrs.push(("name", n));
        }
        self.out.open("visual_scene", &attrs);
        enum Step {
            Open(usize),
            Close,
        }
        // Explicit stack: depth is whatever the scene has.
        let mut stack: Vec<Step> = s.roots.iter().rev().map(|r| Step::Open(*r)).collect();
        while let Some(step) = stack.pop() {
            let i = match step {
                Step::Close => {
                    self.out.close("node");
                    continue;
                }
                Step::Open(i) => i,
            };
            let n = &s.nodes[i];
            let id = self.node_ids[i].clone();
            let mut attrs = vec![("id", id.as_str())];
            let name_text = ncname(n.name.as_deref());
            if let Some(name) = name_text.as_deref() {
                attrs.push(("name", name));
            }
            if self.is_joint[i] {
                attrs.push(("sid", id.as_str()));
                attrs.push(("type", "JOINT"));
            } else {
                attrs.push(("type", "NODE"));
            }
            self.out.open("node", &attrs);
            // A node scaled to nothing keeps a millionth, as its keys do:
            // readers decompose this matrix (`math::nonsingular_scale`).
            let m = match n.transform.trs() {
                Some((t, r, s)) => math::compose(t, r, math::nonsingular_scale(s)),
                None => n.transform.matrix(),
            };
            self.out.numbers("matrix", &[("sid", "transform")], &join_f64(row_major(&m)));
            match self.wrapper_ids[i].clone() {
                Some(w) => {
                    let mut wattrs = vec![("id", w.as_str())];
                    let name_text = ncname(n.name.as_deref());
                    if let Some(name) = name_text.as_deref() {
                        wattrs.push(("name", name));
                    }
                    wattrs.push(("type", "NODE"));
                    self.out.open("node", &wattrs);
                    self.out.numbers("matrix", &[("sid", "transform")], &join_f64(row_major(&math::IDENTITY)));
                    self.instances(i);
                    self.out.close("node");
                }
                None => self.instances(i),
            }
            stack.push(Step::Close);
            for c in n.children.iter().rev() {
                stack.push(Step::Open(*c));
            }
        }
        self.out.close("visual_scene");
        self.out.close("library_visual_scenes");
    }

    /// The skeleton roots of a skin: its joints with no ancestor among its
    /// joints. three.js and assimp search below each `<skeleton>` for the
    /// joints' sids, so these must cover every joint, and disjointly.
    fn skeleton_roots(&self, si: usize) -> Vec<usize> {
        let s = self.s;
        let joints: HashSet<usize> = s.skins[si].joints.iter().copied().collect();
        let parents = s.parents();
        let mut roots = Vec::new();
        for &j in &s.skins[si].joints {
            let mut cur = parents.get(j).copied().flatten();
            let mut steps = 0;
            let mut covered = false;
            while let Some(p) = cur {
                if joints.contains(&p) {
                    covered = true;
                    break;
                }
                steps += 1;
                if steps > s.nodes.len() {
                    break;
                }
                cur = parents.get(p).copied().flatten();
            }
            if !covered && !roots.contains(&j) {
                roots.push(j);
            }
        }
        roots
    }

    /// What node `ni` draws and places, in the schema's order (1.4.1
    /// `<node>`: instance_camera, instance_controller, instance_geometry,
    /// instance_light, then child nodes). A joint's wrapper node draws all
    /// of it, as it draws the joint's mesh.
    fn instances(&mut self, ni: usize) {
        let s = self.s;
        let n = &s.nodes[ni];
        self.instance_camera(ni);
        let Some(mi) = n.mesh else {
            self.instance_light(ni);
            return;
        };
        let mesh = &s.meshes[mi];
        // The controller each primitive is drawn through: its skin's, which
        // sits over its morph if it has one, or else its morph's.
        let set = self.node_set[ni];
        let per_prim: Option<Vec<Option<String>>> = match (n.skin, set) {
            (Some(si), _) => self.controllers.get(&(mi, si, set)).cloned(),
            (None, Some(k)) => Some(self.sets[k].ids.iter().cloned().map(Some).collect()),
            (None, None) => None,
        };
        // Schema order: every instance_controller before any instance_geometry,
        // and child nodes last. The first controller's primitive is drawn
        // here, later ones from identity child nodes (see `skin_part_ids` and
        // `morph_part_ids`).
        let mut later: Vec<(usize, String)> = Vec::new();
        if let Some(per_prim) = &per_prim {
            let roots = n.skin.map(|si| self.skeleton_roots(si)).unwrap_or_default();
            let mut k = 0;
            for (pi, cid) in per_prim.iter().enumerate() {
                let Some(cid) = cid else { continue };
                if k > 0 {
                    later.push((pi, cid.clone()));
                    k += 1;
                    continue;
                }
                k += 1;
                self.instance_controller(cid, &roots, &mesh.primitives[pi]);
            }
        }
        for (pi, p) in mesh.primitives.iter().enumerate() {
            if per_prim.as_ref().is_some_and(|v| v.get(pi).is_some_and(Option::is_some)) {
                continue;
            }
            let url = format!("#{}", self.geom_ids[mi][pi]);
            if p.material.is_some() {
                self.out.open("instance_geometry", &[("url", &url)]);
                self.bind_material(p);
                self.out.close("instance_geometry");
            } else {
                self.out.empty("instance_geometry", &[("url", &url)]);
            }
        }
        self.instance_light(ni);
        if !later.is_empty() {
            let roots = n.skin.map(|si| self.skeleton_roots(si)).unwrap_or_default();
            for (k, (pi, cid)) in later.into_iter().enumerate() {
                let part_ids = if n.skin.is_some() { &self.skin_part_ids[ni] } else { &self.morph_part_ids[ni] };
                let Some(part_id) = part_ids.get(k).cloned() else { continue };
                let mut attrs = vec![("id", part_id.as_str())];
                let name_text = ncname(n.name.as_deref());
                if let Some(name) = name_text.as_deref() {
                    attrs.push(("name", name));
                }
                attrs.push(("type", "NODE"));
                self.out.open("node", &attrs);
                self.out.numbers("matrix", &[("sid", "transform")], &join_f64(row_major(&math::IDENTITY)));
                self.instance_controller(&cid, &roots, &mesh.primitives[pi]);
                self.out.close("node");
            }
        }
    }

    fn instance_controller(&mut self, cid: &str, roots: &[usize], p: &Primitive) {
        self.out.open("instance_controller", &[("url", &format!("#{cid}"))]);
        for r in roots {
            let url = format!("#{}", self.node_ids[*r]);
            self.out.leaf("skeleton", &[], &url);
        }
        self.bind_material(p);
        self.out.close("instance_controller");
    }

    fn bind_material(&mut self, p: &Primitive) {
        let Some(m) = p.material else { return };
        let mat = &self.s.materials[m];
        let mid = self.material_ids[m].clone();
        let mut sets: BTreeSet<u32> = BTreeSet::new();
        for r in [&mat.base_color_texture, &mat.emissive_texture].into_iter().flatten() {
            if self.texture_image(r.texture).is_some() {
                sets.insert(r.uv_set);
            }
        }
        self.out.open("bind_material", &[]);
        self.out.open("technique_common", &[]);
        let target = format!("#{mid}");
        if sets.is_empty() {
            self.out.empty("instance_material", &[("symbol", &mid), ("target", &target)]);
        } else {
            self.out.open("instance_material", &[("symbol", &mid), ("target", &target)]);
            for set in sets {
                let set_s = set.to_string();
                self.out.empty(
                    "bind_vertex_input",
                    &[("semantic", &format!("UVSET{set}")), ("input_semantic", "TEXCOORD"), ("input_set", &set_s)],
                );
            }
            self.out.close("instance_material");
        }
        self.out.close("technique_common");
        self.out.close("bind_material");
    }
}

// ─── writer: entry points ───────────────────────────────────────────────────

/// Write a scene — already reduced by [`prepare`] when it comes through
/// `super::write_scene` — as `model.dae` plus, unless `self_contained`, its
/// textures under `textures/`.
pub fn write_scene(scene: &Scene, opts: WriteOptions) -> Result<Package, MeshError> {
    scene.validate()?;
    // `prepare` strips the points Collada has no primitive for, so a writer
    // given one was not prepared: a debug build says so.
    pose::debug_assert_holds("dae", scene, &capabilities(opts));
    if scene.drawable_count() == 0 {
        return Err(MeshError::Empty);
    }
    let hoisted = glb::hoist_skinned_meshes(scene);
    Writer::new(&hoisted, !opts.self_contained, opts.key_rate(), opts.dae_morphs == DaeMorphs::Keep).document()
}

/// Write the flat mesh as one self-contained document.
pub fn write(mesh: &Mesh) -> Result<Vec<u8>, MeshError> {
    mesh.validate()?;
    if mesh.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    let mut pkg = write_scene(&pose::from_flat(mesh), WriteOptions::default())?;
    Ok(pkg.files.remove(0).bytes)
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::fixtures;
    use super::super::fixtures::lightcam;
    use super::super::package::MemorySidecars;
    use super::super::fixtures::sweeps::{layer_sweep, lean_layered, span};

    // ── builders ────────────────────────────────────────────────────────────

    fn package(scene: &Scene, opts: WriteOptions) -> Package {
        write_scene(&prepare(scene, opts).unwrap(), opts).unwrap()
    }

    fn read_package(pkg: &Package) -> SceneRead {
        read_scene(&pkg.files[0].bytes, &pkg.resolver()).unwrap()
    }

    fn text(pkg: &Package) -> String {
        String::from_utf8(pkg.files[0].bytes.clone()).unwrap()
    }

    /// A document around library snippets.
    fn doc(body: &str) -> Vec<u8> {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?><COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1"><asset><created>x</created><modified>x</modified></asset>{body}</COLLADA>"#
        )
        .into_bytes()
    }

    fn read_doc(body: &str) -> Result<SceneRead, MeshError> {
        read_scene(&doc(body), &NoSidecars)
    }

    fn floats(v: &[f32]) -> String {
        v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" ")
    }

    /// A geometry `id` with these positions and primitive elements, which
    /// reference its `<vertices>` as `#{id}-v`.
    fn geom(id: &str, pos: &[f32], prims: &str) -> String {
        format!(
            r##"<geometry id="{id}" name="{id}"><mesh><source id="{id}-p"><float_array id="{id}-pa" count="{}">{}</float_array><technique_common><accessor source="#{id}-pa" count="{}" stride="3"><param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/></accessor></technique_common></source><vertices id="{id}-v"><input semantic="POSITION" source="#{id}-p"/></vertices>{prims}</mesh></geometry>"##,
            pos.len(),
            floats(pos),
            pos.len() / 3
        )
    }

    fn scene_doc(geometries: &str, nodes: &str) -> String {
        format!(
            r##"<library_geometries>{geometries}</library_geometries><library_visual_scenes><visual_scene id="vs">{nodes}</visual_scene></library_visual_scenes><scene><instance_visual_scene url="#vs"/></scene>"##
        )
    }

    const TRI: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

    fn tri_doc(extra_node_content: &str) -> String {
        scene_doc(
            &geom("g", &TRI, r##"<triangles count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><p>0 1 2</p></triangles>"##),
            &format!(r##"<node id="n">{extra_node_content}<instance_geometry url="#g"/></node>"##),
        )
    }

    fn world_positions(s: &Scene) -> Vec<[f32; 3]> {
        pose::world_primitives(s, &pose::default_pose(s)).unwrap().into_iter().flat_map(|w| w.positions).collect()
    }

    fn close3(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() <= eps)
    }

    fn signed_area_z(s: &Scene) -> Vec<f32> {
        let mut out = Vec::new();
        for w in pose::world_primitives(s, &pose::default_pose(s)).unwrap() {
            for t in w.indices.chunks_exact(3) {
                let (a, b, c) = (w.positions[t[0] as usize], w.positions[t[1] as usize], w.positions[t[2] as usize]);
                out.push(((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) / 2.0);
            }
        }
        out
    }

    // ── round trips ─────────────────────────────────────────────────────────

    /// The scene the writer was handed, as the reader must give it back:
    /// `prepare`d, with skinned meshes at identity roots (the hoist is part of
    /// the encoding).
    fn written_form(fixture: &Scene, opts: WriteOptions) -> Scene {
        glb::hoist_skinned_meshes(&prepare(fixture, opts).unwrap())
    }

    fn assert_round_trip(name: &str, fixture: &Scene, opts: WriteOptions) {
        let want = written_form(fixture, opts);
        let got = read_package(&package(fixture, opts)).scene;
        let ctx = format!("{name} ({})", if opts.self_contained { "single file" } else { "package" });
        assert_eq!(got.nodes.len(), want.nodes.len(), "{ctx}: node count");
        for (i, (g, w)) in got.nodes.iter().zip(&want.nodes).enumerate() {
            assert_eq!(g.name, w.name, "{ctx}: node {i} name");
            assert_eq!(g.children, w.children, "{ctx}: node {i} children");
            assert_eq!(g.mesh.is_some(), w.mesh.is_some(), "{ctx}: node {i} mesh");
            assert_eq!(g.skin.is_some(), w.skin.is_some(), "{ctx}: node {i} skin");
            let (gm, wm) = (g.transform.matrix(), w.transform.matrix());
            assert!(gm.iter().zip(&wm).all(|(a, b)| (a - b).abs() < 1e-9), "{ctx}: node {i} transform {gm:?} vs {wm:?}");
        }
        assert_eq!(got.roots, want.roots, "{ctx}: roots");
        assert_eq!(got.meshes.len(), want.meshes.len(), "{ctx}: mesh count (instancing must survive)");
        for (mi, (g, w)) in got.meshes.iter().zip(&want.meshes).enumerate() {
            assert_eq!(g.name, w.name, "{ctx}: mesh {mi} name");
            assert_eq!(g.primitives.len(), w.primitives.len(), "{ctx}: mesh {mi} primitives");
            for (pi, (gp, wp)) in g.primitives.iter().zip(&w.primitives).enumerate() {
                let at = format!("{ctx}: mesh {mi} primitive {pi}");
                // Text, written shortest-round-trip, gives every float back
                // to the bit; UVs too, through the f64 flip.
                assert_eq!(gp.positions, wp.positions, "{at}: positions");
                assert_eq!(gp.normals, wp.normals, "{at}: normals");
                assert_eq!(gp.uvs, wp.uvs, "{at}: uvs");
                assert_eq!(gp.colors, wp.colors, "{at}: colours");
                assert_eq!(gp.indices, wp.indices, "{at}: indices");
                assert_eq!(gp.material, wp.material, "{at}: material");
                assert_eq!(gp.targets.len(), 0, "{at}: morph targets are never written");
                match (&gp.skin, &wp.skin) {
                    (None, None) => {}
                    (Some(gs), Some(ws)) => {
                        for v in 0..gp.positions.len() {
                            // Compared as normalised influence sets: the
                            // writer drops zero weights and normalises.
                            let norm = |js: &[u16], ws: &[f32]| -> Vec<(u16, i64)> {
                                let sum: f32 = ws.iter().filter(|w| **w > 0.0).sum();
                                let mut out: Vec<(u16, i64)> = js
                                    .iter()
                                    .zip(ws)
                                    .filter(|(_, w)| **w > 0.0)
                                    .map(|(j, w)| (*j, (w / sum * 1e5).round() as i64))
                                    .collect();
                                out.sort();
                                out
                            };
                            let (gj, gw) = gs.vertex(v);
                            let (wj, ww) = ws.vertex(v);
                            assert_eq!(norm(gj, gw), norm(wj, ww), "{at}: vertex {v} influences");
                        }
                    }
                    other => panic!("{at}: skin presence differs: {other:?}"),
                }
            }
        }
        assert_eq!(got.materials.len(), want.materials.len(), "{ctx}: material count");
        for (i, (g, w)) in got.materials.iter().zip(&want.materials).enumerate() {
            let at = format!("{ctx}: material {i}");
            assert_eq!(g.name, w.name, "{at}");
            for k in 0..4 {
                assert!((g.base_color[k] - w.base_color[k]).abs() < 1e-6, "{at}: base colour {:?} vs {:?}", g.base_color, w.base_color);
            }
            for k in 0..3 {
                assert!((g.emissive[k] - w.emissive[k]).abs() < 1e-6, "{at}: emissive");
            }
            assert!((g.roughness - w.roughness).abs() < 1e-6, "{at}: roughness {} vs {}", g.roughness, w.roughness);
            assert_eq!(g.metallic, w.metallic, "{at}: metallic");
            assert_eq!(g.alpha_mode, w.alpha_mode, "{at}: alpha mode");
            assert_eq!(g.double_sided, w.double_sided, "{at}: double-sided");
            assert_eq!(g.unlit, w.unlit, "{at}: unlit");
            assert_eq!(g.base_color_texture.as_ref().map(|t| t.uv_set), w.base_color_texture.as_ref().map(|t| t.uv_set), "{at}: base texture");
            assert_eq!(g.emissive_texture.is_some(), w.emissive_texture.is_some(), "{at}: emissive texture");
            if let (Some(gt), Some(wt)) = (&g.base_color_texture, &w.base_color_texture) {
                let gt = &got.textures[gt.texture];
                let wt = &want.textures[wt.texture];
                assert_eq!(got.images[gt.image].data, want.images[wt.image].data, "{at}: image bytes");
                assert_eq!(gt.sampler, wt.sampler, "{at}: sampler");
            }
        }
        assert_eq!(got.skins.len(), want.skins.len(), "{ctx}: skins");
        for (g, w) in got.skins.iter().zip(&want.skins) {
            assert_eq!(g.joints, w.joints, "{ctx}: skin joints");
            assert_eq!(g.inverse_bind_matrices, w.inverse_bind_matrices, "{ctx}: inverse binds (f64 text is exact)");
        }
        assert_eq!(got.animations.len(), want.animations.len(), "{ctx}: animations");
        for (ai, (g, w)) in got.animations.iter().zip(&want.animations).enumerate() {
            assert_eq!(g.name, w.name, "{ctx}: animation {ai} name");
            let (lo, hi) = w.time_range().unwrap();
            assert_eq!(g.time_range(), Some((lo, hi)), "{ctx}: animation {ai} range");
            // Between keys, where interpolation differences show: the
            // resampled matrices must follow the source within a hair.
            for k in 0..=97 {
                let t = lo + (hi - lo) * k as f32 / 97.0;
                let (gp, wp) = (pose::sample_pose(&got, ai, t).locals(), pose::sample_pose(&want, ai, t).locals());
                for (n, (a, b)) in gp.iter().zip(&wp).enumerate() {
                    let d = a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f64::max);
                    assert!(d < 2e-3, "{ctx}: animation {ai} node {n} at t={t} off by {d}");
                }
            }
        }
    }

    #[test]
    fn every_fixture_round_trips_as_a_package() {
        for (name, fixture) in fixtures::all() {
            assert_round_trip(name, &fixture, WriteOptions { self_contained: false, ..Default::default() });
        }
    }

    #[test]
    fn every_fixture_round_trips_as_a_single_file() {
        for (name, fixture) in fixtures::all() {
            assert_round_trip(name, &fixture, WriteOptions { self_contained: true, ..Default::default() });
        }
    }

    #[test]
    fn writing_is_deterministic() {
        for (name, fixture) in fixtures::all() {
            let opts = WriteOptions { self_contained: false, ..Default::default() };
            assert_eq!(package(&fixture, opts), package(&fixture, opts), "{name}");
        }
    }

    #[test]
    fn the_flat_api_round_trips_bit_exactly() {
        let mesh = pose::flatten(&fixtures::cube()).unwrap();
        let back = read(&write(&mesh).unwrap()).unwrap();
        assert_eq!(back.positions, mesh.positions);
        assert_eq!(back.indices, mesh.indices);
        assert_eq!(back.normals, mesh.normals);
        assert_eq!(back.uvs, mesh.uvs);
        let (b, m) = (back.base_color.unwrap(), mesh.base_color.unwrap());
        assert!((0..4).all(|k| (b[k] - m[k]).abs() < 1e-6), "{b:?} vs {m:?}");
    }

    // ── what the writer emits, and why ──────────────────────────────────────

    #[test]
    fn writes_a_collada_141_document_in_metres_y_up() {
        let t = text(&package(&fixtures::cube(), WriteOptions::default()));
        assert!(t.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<COLLADA xmlns=\"http://www.collada.org/2005/11/COLLADASchema\" version=\"1.4.1\">"));
        assert!(t.contains("<unit name=\"meter\" meter=\"1\"/>") && t.contains("<up_axis>Y_UP</up_axis>"));
        assert!(t.contains(&format!("<created>{TIMESTAMP}</created>")));
        // Matrices as Collada writes them: the translation in the 4th column.
        let mut s = fixtures::cube();
        s.nodes[0].transform = Transform::Trs { translation: [1.0, 2.0, 3.0], rotation: math::QUAT_IDENTITY, scale: [1.0; 3] };
        let t = text(&package(&s, WriteOptions::default()));
        assert!(t.contains(">1 0 0 1 0 1 0 2 0 0 1 3 0 0 0 1</matrix>"), "{t}");
    }

    #[test]
    fn material_colours_are_srgb_and_vertex_colours_linear_inside_vertices() {
        let s = fixtures::textured();
        let t = text(&package(&s, WriteOptions { self_contained: false, ..Default::default() }));
        // wedge_mat's linear 0.9 0.3 0.1 as three.js and SceneKit read it.
        let c = [0.9f32, 0.3, 0.1].map(srgb_encode);
        assert!(t.contains(&format!("<color sid=\"diffuse\">{} {} {} 1</color>", c[0], c[1], c[2])), "diffuse not sRGB-encoded");
        // The panel's colours: a COLOR input inside <vertices>, values as-is.
        let v = t.find("<vertices id=\"panel-mesh-vertices\">").unwrap();
        assert!(t[v..v + 300].contains("semantic=\"COLOR\""));
        let first = s.meshes[2].primitives[0].colors.as_ref().unwrap()[1];
        assert!(t.contains(&floats(&first)), "linear colours written as-is");
        let back = read_package(&package(&s, WriteOptions { self_contained: false, ..Default::default() })).scene;
        assert_eq!(back.meshes[2].primitives[0].colors, s.meshes[2].primitives[0].colors);
    }

    #[test]
    fn a_single_file_carries_no_images_and_says_so() {
        let w = super::super::write_scene(&fixtures::textured(), MeshFormat::Dae, WriteOptions::default()).unwrap();
        assert_eq!(w.package.files.len(), 1);
        let t = text(&w.package);
        assert!(!t.contains("<library_images>") && !t.contains("<texture "));
        assert!(w.losses.iter().any(|l| l.what.contains("texture binding")), "{:?}", w.losses);
    }

    #[test]
    fn a_package_carries_textures_beside_the_document() {
        let pkg = package(&fixtures::textured(), WriteOptions { self_contained: false, ..Default::default() });
        assert_eq!(pkg.files[0].path, "model.dae");
        let tex = pkg.files.iter().find(|f| f.path == "textures/quadrants.png").expect("texture sidecar");
        assert_eq!(tex.bytes, fixtures::quadrant_png());
        assert!(text(&pkg).contains("<init_from>textures/quadrants.png</init_from>"));
    }

    #[test]
    fn morph_targets_are_baked_and_the_loss_is_reported() {
        let s = fixtures::morph();
        let w = super::super::write_scene(&s, MeshFormat::Dae, WriteOptions::default()).unwrap();
        let t = text(&w.package);
        assert!(!t.contains("<morph") && !t.contains("<controller"), "no morph controller may be written");
        let losses: Vec<String> = w.losses.iter().map(|l| l.to_string()).collect();
        assert!(losses.iter().any(|l| l.contains("morph targets")), "{losses:?}");
        assert!(losses.iter().any(|l| l.contains("morph-weight animation")), "{losses:?}");
        // The geometry is the default-weight bake: bulge at 0.25.
        let back = read_package(&w.package).scene;
        let want = pose::bake_default(&s).unwrap();
        let (g, b) = (world_positions(&back), world_positions(&want));
        assert_eq!(g.len(), b.len());
        assert!(g.iter().zip(&b).all(|(x, y)| close3(*x, *y, 1e-5)));
    }

    #[test]
    fn step_keys_get_a_key_just_before_them() {
        // The skinned fixture's hip translation steps from 0 to 0.4 at t=1.
        let back = read_package(&package(&fixtures::skinned(), WriteOptions::default())).scene;
        let hip = |t: f32| pose::sample_pose(&back, 0, t).locals()[0][12];
        assert!((hip(0.9998) - 0.0).abs() < 1e-3, "{}", hip(0.9998));
        assert!((hip(1.0) - 0.4).abs() < 1e-6, "{}", hip(1.0));
        assert!((hip(0.5) - 0.0).abs() < 1e-6 && (hip(1.5) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn later_skinned_primitives_are_drawn_from_child_nodes() {
        // SceneKit keeps one instance_controller per node.
        let mut s = fixtures::skinned();
        let p = s.meshes[0].primitives[0].clone();
        s.meshes[0].primitives.push(p);
        let t = text(&package(&s, WriteOptions::default()));
        let node = t.find("<node id=\"tube_node\"").unwrap();
        let first_close = t[node..].find("</node>").unwrap() + node;
        assert_eq!(t[node..first_close].matches("<instance_controller").count(), 2, "one on the node, one on its child");
        assert!(t[node..first_close].contains("<node id=\"tube_node-skin1\""));
        // Both read back as one skin.
        let back = read_package(&package(&s, WriteOptions::default())).scene;
        assert_eq!(back.skins.len(), 1);
    }

    #[test]
    fn a_joint_holding_a_mesh_draws_it_from_a_child() {
        let mut s = fixtures::skinned();
        s.nodes[1].mesh = Some(0);
        s.nodes[1].skin = None;
        s.meshes.push(s.meshes[0].clone());
        s.meshes[1].primitives[0].skin = None;
        s.nodes[1].mesh = Some(1);
        let t = text(&package(&s, WriteOptions::default()));
        let knee = t.find("<node id=\"knee\"").unwrap();
        let geom = t.find("<node id=\"knee-geometry\"").unwrap();
        let inst = t[knee..].find("<instance_geometry").unwrap() + knee;
        assert!(geom > knee && inst > geom, "the instance sits in the child, not on the joint");
    }

    #[test]
    fn ids_are_unique_ncnames_whatever_the_names() {
        let mut s = fixtures::textured();
        for (i, n) in s.nodes.iter_mut().enumerate() {
            n.name = Some(["1 box", "box", "box", "<&>\"'", "ünï\n\tcødé"][i].to_string());
        }
        s.materials[0].name = Some("box".into());
        let pkg = package(&s, WriteOptions { self_contained: false, ..Default::default() });
        let t = text(&pkg);
        let root = super::super::xml::parse(&pkg.files[0].bytes).unwrap();
        let mut ids = HashSet::new();
        let mut stack = vec![&root];
        while let Some(e) = stack.pop() {
            if let Some(id) = e.attr("id") {
                assert!(ids.insert(id.to_string()), "duplicate id {id}");
                assert!(id.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_'), "{id}");
                assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)), "{id}");
            }
            stack.extend(e.children.iter());
        }
        // Names are NCNames too (1.4.1's type for `name`; SceneKit cuts a
        // name at its first space): letters of any script survive, the rest
        // become `_`.
        let names: Vec<String> = root
            .descendants_named("node")
            .iter()
            .filter_map(|n| n.attr("name").map(str::to_string))
            .collect();
        for want in ["_1_box", "box", "_____", "ünï__cødé"] {
            assert!(names.iter().any(|n| n == want), "{want} not among {names:?}");
        }
        assert!(!t.contains("name=\"1 box\""));
        let back = read_package(&pkg).scene;
        assert_eq!(back.nodes[4].name.as_deref(), Some("ünï__cødé"));
    }

    // ── sampled uv sets first, in Collada and FBX ───────────────────────────

    /// `scene` written as `to` and read back with the files it wrote.
    fn through(scene: &Scene, to: MeshFormat, self_contained: bool) -> Scene {
        let w = super::super::write_scene(scene, to, WriteOptions { self_contained, ..Default::default() })
            .unwrap_or_else(|e| panic!("write {to}: {e}"));
        super::super::read_scene(&w.package.files[0].bytes, to, &w.package.resolver())
            .unwrap_or_else(|e| panic!("read {to}: {e}"))
            .scene
    }

    /// Each distinct vertex of `p` as its position and its uv sets, in
    /// position order: a reader numbers vertices its own way (FBX's welds the
    /// corners of its polygons).
    fn corner_uvs(p: &Primitive) -> Vec<([f32; 3], Vec<[f32; 2]>)> {
        let mut out: Vec<([f32; 3], Vec<[f32; 2]>)> =
            (0..p.positions.len()).map(|v| (p.positions[v], p.uvs.iter().map(|set| set[v]).collect())).collect();
        out.sort_by_key(|(at, _)| at.map(f32::to_bits));
        out.dedup();
        out
    }

    /// `fixtures::multiuv`'s quad cut to its first two sets, the identity and
    /// a half-size window off centre (no value shared, and every one a
    /// multiple of 1/16, so each V flip is exact), with no texture bound.
    fn two_set_quad() -> Scene {
        let mut s = fixtures::multiuv();
        s.meshes[0].primitives[0].uvs.truncate(2);
        s.materials[0].base_color_texture = None;
        s.materials[0].occlusion_texture = None;
        s
    }

    #[test]
    fn fbx_and_dae_put_the_base_colour_set_first() {
        // three.js r183's ColladaLoader and FBXLoader sample every texture
        // from set 0 (the `prepare` docs here and in fbx.rs say why). A base
        // colour on set 1 of two, with nothing on set 0, is written on set 0
        // from set 1's coordinates, and the other set follows at index 1. A
        // single Collada file carries no texture, so its sets keep scene
        // order.
        let mut s = two_set_quad();
        s.materials[0].base_color_texture = Some(TextureRef { texture: 0, uv_set: 1, transform: None });
        let source = s.meshes[0].primitives[0].clone();
        let swapped = Primitive { uvs: vec![source.uvs[1].clone(), source.uvs[0].clone()], ..source.clone() };
        let got = [(MeshFormat::Dae, false), (MeshFormat::Dae, true), (MeshFormat::Fbx, false)].map(|(to, single)| {
            let back = through(&s, to, single);
            let p = &back.meshes[0].primitives[0];
            let base = p.material.and_then(|m| back.materials[m].base_color_texture.as_ref()).map(|t| t.uv_set);
            (to, single, base, corner_uvs(p))
        });
        let want = [
            (MeshFormat::Dae, false, Some(0), corner_uvs(&swapped)),
            (MeshFormat::Dae, true, None, corner_uvs(&source)),
            (MeshFormat::Fbx, false, Some(0), corner_uvs(&swapped)),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn collada_puts_an_emissive_only_set_first() {
        // A Collada package keeps emission as well as base colour
        // (`TEXTURED_SLOTS`), so an emissive map alone on set 1 of two, with
        // nothing on set 0, is written on set 0 from set 1's coordinates,
        // where three.js's ColladaLoader samples it, and the other set follows
        // at index 1.
        let mut s = two_set_quad();
        let m = &mut s.materials[0];
        m.emissive_texture = Some(TextureRef { texture: 0, uv_set: 1, transform: None });
        m.emissive = [1.0; 3];
        let source = s.meshes[0].primitives[0].clone();
        let swapped = Primitive { uvs: vec![source.uvs[1].clone(), source.uvs[0].clone()], ..source.clone() };
        let back = through(&s, MeshFormat::Dae, false);
        let p = &back.meshes[0].primitives[0];
        let emissive = p.material.and_then(|m| back.materials[m].emissive_texture.as_ref()).map(|t| t.uv_set);
        assert_eq!((emissive, corner_uvs(p)), (Some(0), corner_uvs(&swapped)));
    }

    #[test]
    fn a_dropped_map_never_takes_set_zero() {
        // Neither keeps an occlusion map (Phong), so its set is not put
        // first: an emissive map on set 0, beside occlusion on set 1, stays
        // on set 0, where three.js draws it, and no set moves.
        let mut s = two_set_quad();
        let m = &mut s.materials[0];
        m.emissive_texture = Some(TextureRef { texture: 0, uv_set: 0, transform: None });
        m.emissive = [1.0; 3];
        m.occlusion_texture = Some(TextureRef { texture: 1, uv_set: 1, transform: None });
        let source = s.meshes[0].primitives[0].clone();
        let got = [MeshFormat::Dae, MeshFormat::Fbx].map(|to| {
            let back = through(&s, to, false);
            let p = &back.meshes[0].primitives[0];
            let emissive = p.material.and_then(|m| back.materials[m].emissive_texture.as_ref()).map(|t| t.uv_set);
            (to, emissive, corner_uvs(p))
        });
        assert_eq!(got, [MeshFormat::Dae, MeshFormat::Fbx].map(|to| (to, Some(0), corner_uvs(&source))));
    }

    #[test]
    fn a_dropped_image_never_takes_set_zero() {
        // Neither carries a WebP texture, so a base colour drawn from one is
        // dropped and its set is not put first: a PNG emissive map on set 0,
        // beside it on set 1, stays on set 0, reading the source's set 0.
        let mut s = two_set_quad();
        s.images[1].format = ImageFormat::Webp;
        let m = &mut s.materials[0];
        m.base_color_texture = Some(TextureRef { texture: 1, uv_set: 1, transform: None });
        m.emissive_texture = Some(TextureRef { texture: 0, uv_set: 0, transform: None });
        m.emissive = [1.0; 3];
        let source = s.meshes[0].primitives[0].clone();
        let got = [MeshFormat::Dae, MeshFormat::Fbx].map(|to| {
            let back = through(&s, to, false);
            let p = &back.meshes[0].primitives[0];
            let m = p.material.map(|m| &back.materials[m]);
            let base = m.and_then(|m| m.base_color_texture.as_ref()).map(|t| t.uv_set);
            let emissive = m.and_then(|m| m.emissive_texture.as_ref()).map(|t| t.uv_set);
            (to, base, emissive, corner_uvs(p))
        });
        assert_eq!(got, [MeshFormat::Dae, MeshFormat::Fbx].map(|to| (to, None, Some(0), corner_uvs(&source))));
    }

    #[test]
    fn dae_prepare_handles_layer_textures() {
        // The wedge's sheen colour map is its image's one use. Collada keeps
        // no layer: no binding, no stale index and no image stay behind, and
        // the sheen is reported.
        use super::super::scene::{MaterialLayer, Sheen};
        let mut s = fixtures::textured();
        s.images.push(Image { name: Some("fuzz".into()), format: ImageFormat::Png, data: fixtures::bands_png() });
        s.textures.push(Texture { name: None, image: 1, sampler: Default::default() });
        let map = Some(TextureRef { texture: 1, uv_set: 0, transform: None });
        s.materials[1].sheen = Some(Sheen { color: [0.9, 0.6, 0.8], color_texture: map, ..Default::default() });
        let opts = WriteOptions { self_contained: false, ..Default::default() };
        let d = prepare(&s, opts).unwrap();
        d.validate().unwrap();
        assert!(d.materials.iter().all(|m| MaterialLayer::ALL.iter().all(|&l| !m.has_layer(l))), "{:?}", d.materials);
        assert_eq!(d.images.iter().map(|i| i.name.as_deref()).collect::<Vec<_>>(), [Some("quadrants")]);
        let losses = super::super::write_scene(&s, MeshFormat::Dae, opts).unwrap().losses;
        assert!(losses.iter().any(|l| l.kind == LossKind::Materials && l.what == "sheen on 1 material(s)"), "{losses:?}");
    }

    #[test]
    fn a_scene_textured_on_set_zero_is_unchanged() {
        // Putting the sampled sets first changes nothing for a scene whose
        // textures sit on set 0: every file `fixtures::textured()` writes as
        // Collada and as FBX, in both modes, is byte for byte what it was
        // before that change. The hashes are FNV-1a 64, recorded before it
        // (`DefaultHasher`'s algorithm can change between Rust releases). A
        // change meant to alter these files updates them and says why.
        fn fnv(bytes: &[u8]) -> u64 {
            let mut h: u64 = 0xcbf29ce484222325;
            for b in bytes {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x100000001b3);
            }
            h
        }
        let file = |path: &str, hash: u64| (path.to_string(), format!("{hash:016x}"));
        let got = [(MeshFormat::Dae, false), (MeshFormat::Dae, true), (MeshFormat::Fbx, false), (MeshFormat::Fbx, true)].map(
            |(to, self_contained)| {
                let opts = WriteOptions { self_contained, ..Default::default() };
                let w = super::super::write_scene(&fixtures::textured(), to, opts).unwrap();
                (to, self_contained, w.package.files.iter().map(|f| file(&f.path, fnv(&f.bytes))).collect::<Vec<_>>())
            },
        );
        let want = [
            (MeshFormat::Dae, false, vec![file("model.dae", 0x028472e5241e208d), file("textures/quadrants.png", 0x952b1e0ad94254c4)]),
            (MeshFormat::Dae, true, vec![file("model.dae", 0x64560ce190f60ca1)]),
            (MeshFormat::Fbx, false, vec![file("model.fbx", 0x80b74ffea2f1227f)]),
            (MeshFormat::Fbx, true, vec![file("model.fbx", 0x80b74ffea2f1227f)]),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn bake_is_todays_output() {
        // Guards the Collada files this writer produced before `dae_morphs`
        // existed: every fixture, written as one file and as a package, is
        // byte for byte what it was, and the default is `Bake`, which writes
        // the same bytes as asking for it. The hashes are FNV-1a 64 of each
        // file, recorded before the switch was added (`DefaultHasher`'s
        // algorithm can change between Rust releases). The default-against-
        // `Bake` check compares the two runs' bytes, so it does not depend on
        // the platform's libm. A change meant to alter these files updates the
        // pins and says why.
        //
        // The pins run on macOS only (Ruling DM2a): nearly every fixture's
        // Collada text goes through libm (colours through `srgb_encode`'s
        // `powf`, `sin_cos`, `exp` and `atan2`/`hypot` in the fixtures, slerp
        // in resampling), and glibc may differ from Apple's in the last bit.
        fn fnv(bytes: &[u8]) -> u64 {
            let mut h: u64 = 0xcbf29ce484222325;
            for b in bytes {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x100000001b3);
            }
            h
        }
        let mut got: Vec<(String, bool, String, String)> = Vec::new();
        for (name, scene) in fixtures::all() {
            for self_contained in [false, true] {
                let opts = WriteOptions { self_contained, ..Default::default() };
                let w = super::super::write_scene(&scene, MeshFormat::Dae, opts).unwrap();
                for f in &w.package.files {
                    got.push((name.to_string(), self_contained, f.path.clone(), format!("{:016x}", fnv(&f.bytes))));
                }
                let bake = WriteOptions { dae_morphs: DaeMorphs::Bake, ..opts };
                let explicit = super::super::write_scene(&scene, MeshFormat::Dae, bake).unwrap();
                let files = |w: &super::super::SceneWrite| w.package.files.iter().map(|f| (f.path.clone(), f.bytes.clone())).collect::<Vec<_>>();
                assert_eq!(files(&w), files(&explicit), "{name}, self_contained {self_contained}: the default is Bake");
            }
        }
        #[cfg(target_os = "macos")]
        let got: Vec<(&str, bool, &str, &str)> = got.iter().map(|(n, s, p, h)| (n.as_str(), *s, p.as_str(), h.as_str())).collect();
        #[cfg(target_os = "macos")]
        let want: Vec<(&str, bool, &str, &str)> = vec![
            ("textured", false, "model.dae", "028472e5241e208d"),
            ("textured", false, "textures/quadrants.png", "952b1e0ad94254c4"),
            ("textured", true, "model.dae", "64560ce190f60ca1"),
            ("skinned", false, "model.dae", "8b539e4beab20336"),
            ("skinned", false, "textures/bands.png", "7bf71dc9bf715a1b"),
            ("skinned", true, "model.dae", "b56399037e53d39b"),
            ("morph", false, "model.dae", "7f5193d1cc15ae14"),
            ("morph", true, "model.dae", "7f5193d1cc15ae14"),
            ("cube", false, "model.dae", "31de961d6498d72e"),
            ("cube", true, "model.dae", "31de961d6498d72e"),
            ("metal", false, "model.dae", "758732efbf90f908"),
            ("metal", false, "textures/quadrants.png", "952b1e0ad94254c4"),
            ("metal", true, "model.dae", "d95d38b317cc6e17"),
            ("multiuv", false, "model.dae", "1507272ccfda9865"),
            ("multiuv", false, "textures/quadrants.png", "952b1e0ad94254c4"),
            ("multiuv", true, "model.dae", "1771cc126b0fc1b2"),
            ("multiuv_shared", false, "model.dae", "b99ea91f86cf6a35"),
            ("multiuv_shared", false, "textures/quadrants.png", "952b1e0ad94254c4"),
            ("multiuv_shared", true, "model.dae", "071c93b1eff09c87"),
            ("layered", false, "model.dae", "aff078fc20000826"),
            ("layered", true, "model.dae", "aff078fc20000826"),
            ("staged", false, "model.dae", "6065a872e7fbcfa1"),
            ("staged", true, "model.dae", "6065a872e7fbcfa1"),
            ("wire", false, "model.dae", "3cf75a328301ce6a"),
            ("wire", true, "model.dae", "3cf75a328301ce6a"),
            ("clips", false, "model.dae", "29e0f0af599cea4a"),
            ("clips", false, "textures/bands.png", "7bf71dc9bf715a1b"),
            ("clips", true, "model.dae", "4e0132c37fbe82c3"),
            ("unlit", false, "model.dae", "2ebde68c1c58e8b8"),
            ("unlit", false, "textures/quadrants.png", "952b1e0ad94254c4"),
            ("unlit", true, "model.dae", "095f2418129ed058"),
        ];
        #[cfg(target_os = "macos")]
        assert_eq!(got, want);
    }

    #[test]
    fn dae_morphs_decides_whether_the_targets_and_their_animation_are_kept() {
        // `Keep` reports morph targets as supported and owes no note about
        // their weight animation. `Bake` supports none, and the weight
        // channels are what it drops and names.
        let s = fixtures::morph();
        let channels = s.animations.iter().flat_map(|a| &a.channels).filter(|c| c.path == AnimPath::Weights).count();
        assert!(channels > 0, "the morph fixture animates its weights");
        let weights_lost = |opts: WriteOptions| {
            extra_losses(&s, opts).into_iter().filter(|l| l.kind == LossKind::MorphTargets).map(|l| l.what).collect::<Vec<_>>()
        };
        let bake = WriteOptions { dae_morphs: DaeMorphs::Bake, ..Default::default() };
        let keep = WriteOptions { dae_morphs: DaeMorphs::Keep, ..Default::default() };
        assert!(!capabilities(bake).morph_targets, "Bake writes no morph targets");
        assert!(capabilities(keep).morph_targets, "Keep writes them");
        assert_eq!(weights_lost(bake), [format!("{channels} morph-weight animation channel(s)")], "Bake drops the weight animation, named");
        assert_eq!(weights_lost(keep), Vec::<String>::new(), "Keep drops nothing");
    }

    #[test]
    fn dae_morphs_are_spelled_as_a_caller_spells_them() {
        assert_eq!(DaeMorphs::Bake.as_str(), "bake");
        assert_eq!(DaeMorphs::Keep.as_str(), "keep");
        assert_eq!(DaeMorphs::default(), DaeMorphs::Bake);
        assert_eq!(WriteOptions::default().dae_morphs, DaeMorphs::Bake);
    }

    // ── kept morphs (`DaeMorphs::Keep`) ─────────────────────────────────────

    fn keep_opts() -> WriteOptions {
        WriteOptions { self_contained: false, dae_morphs: DaeMorphs::Keep, ..Default::default() }
    }

    /// `scene` written with `Keep` and read back.
    fn kept_back(scene: &Scene) -> Scene {
        read_package(&package(scene, keep_opts())).scene
    }

    /// `scene` written with `Keep`, as parsed XML.
    fn kept_xml(scene: &Scene) -> Element {
        super::super::xml::parse(&package(scene, keep_opts()).files[0].bytes).unwrap()
    }

    /// The element with this `id`, whatever it is.
    fn by_id<'a>(root: &'a Element, id: &str) -> &'a Element {
        let mut stack = vec![root];
        while let Some(e) = stack.pop() {
            if e.attr("id") == Some(id) {
                return e;
            }
            stack.extend(e.children.iter());
        }
        panic!("no element with id {id}");
    }

    fn ref_id(url: &str) -> &str {
        url.strip_prefix('#').unwrap_or(url)
    }

    fn assert_unique_ids(root: &Element) {
        let mut ids = HashSet::new();
        let mut stack = vec![root];
        while let Some(e) = stack.pop() {
            if let Some(id) = e.attr("id") {
                assert!(ids.insert(id.to_string()), "duplicate id {id}");
                assert!(id.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_'), "{id}");
                assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)), "{id}");
            }
            stack.extend(e.children.iter());
        }
    }

    /// (base geometry id, target geometry ids) of every `<morph>`, in document order.
    fn morph_controllers(root: &Element) -> Vec<(String, Vec<String>)> {
        root.descendants_named("morph")
            .into_iter()
            .map(|m| {
                let base = ref_id(m.attr("source").unwrap_or("")).to_string();
                let ids = m
                    .descendants_named("IDREF_array")
                    .first()
                    .map(|a| a.text.split_ascii_whitespace().map(str::to_string).collect())
                    .unwrap_or_default();
                (base, ids)
            })
            .collect()
    }

    /// The numbers of the `<source>` a geometry's `<vertices>` input names
    /// for `semantic`, or `None` when `<vertices>` has no such input.
    fn vertices_floats(root: &Element, geometry: &str, semantic: &str) -> Option<Vec<f32>> {
        let vertices = by_id(root, geometry).child("mesh")?.child("vertices")?;
        let input = vertices.children_named("input").find(|i| i.attr("semantic") == Some(semantic))?;
        let source = by_id(root, ref_id(input.attr("source")?));
        super::super::xml::floats(&source.child("float_array")?.text).ok()
    }

    fn weights_of(s: &Scene, anim: usize, node: usize, t: f32) -> Vec<f32> {
        pose::sample_pose(s, anim, t).weights.get(node).cloned().unwrap_or_default()
    }

    type Posed = Vec<(Vec<[f32; 3]>, Option<Vec<[f32; 3]>>)>;

    /// What a scene draws in world space at `t` into its first clip (`None`: at rest).
    fn posed(s: &Scene, t: Option<f32>) -> Posed {
        let pose = match t {
            Some(t) if !s.animations.is_empty() => pose::sample_pose(s, 0, t),
            _ => pose::default_pose(s),
        };
        pose::world_primitives(s, &pose).unwrap().into_iter().map(|w| (w.positions, w.normals)).collect()
    }

    /// `got` draws what `want` draws, in the same order, at rest and at every
    /// sample time of `want`'s first clip, to within `eps`.
    fn assert_posed_alike(want: &Scene, got: &Scene, eps: f32, ctx: &str) {
        let mut times = vec![None];
        times.extend(fixtures::sample_times(want).into_iter().map(Some));
        for t in times {
            let (w, g) = (posed(want, t), posed(got, t));
            assert_eq!(g.len(), w.len(), "{ctx} @ {t:?}: drawn primitives");
            for (i, ((wp, wn), (gp, gn))) in w.iter().zip(&g).enumerate() {
                assert_eq!(gp.len(), wp.len(), "{ctx} @ {t:?}: primitive {i} vertices");
                for (k, (a, b)) in wp.iter().zip(gp).enumerate() {
                    assert!(close3(*a, *b, eps), "{ctx} @ {t:?}: primitive {i} vertex {k}: {a:?} vs {b:?}");
                }
                assert_eq!(gn.is_some(), wn.is_some(), "{ctx} @ {t:?}: primitive {i} normals");
                if let (Some(wn), Some(gn)) = (wn, gn) {
                    for (k, (a, b)) in wn.iter().zip(gn).enumerate() {
                        assert!(close3(*a, *b, eps), "{ctx} @ {t:?}: primitive {i} normal {k}: {a:?} vs {b:?}");
                    }
                }
            }
        }
    }

    /// The morph fixture's grid three times over, as one mesh of three
    /// primitives: each copy sits at its own height and bulges by its own
    /// amount, so a primitive read as another is seen.
    fn morph_three_primitives() -> Scene {
        let mut s = fixtures::morph();
        let base = s.meshes[0].primitives[0].clone();
        s.meshes[0].primitives = (0..3)
            .map(|k| {
                let mut p = base.clone();
                for v in p.positions.iter_mut() {
                    v[2] += 3.0 * k as f32;
                }
                for t in p.targets.iter_mut() {
                    for o in t.positions.iter_mut() {
                        o[2] *= 1.0 + 0.5 * k as f32;
                    }
                }
                p
            })
            .collect();
        s
    }

    /// The morph fixture, drawn by three more nodes: two with the same weights
    /// of their own, one with a third set.
    fn morph_instances() -> Scene {
        let mut s = fixtures::morph();
        for (name, x, w) in [("copy_a", 3.0, [1.0, 0.5]), ("copy_b", -3.0, [1.0, 0.5]), ("copy_c", 6.0, [0.0, 1.0])] {
            let transform = Transform::Trs { translation: [x, 0.0, 0.0], rotation: math::QUAT_IDENTITY, scale: [1.0; 3] };
            s.add_root(Node { name: Some(name.into()), transform, mesh: Some(0), weights: Some(w.to_vec()), ..Default::default() });
        }
        s
    }

    /// The skinned fixture with one morph target on its tube (a swell, with
    /// normals that lean with it) and a weight channel beside its joint clip.
    fn skinned_and_morphed() -> Scene {
        let mut s = fixtures::skinned();
        let p = &mut s.meshes[0].primitives[0];
        let swell: Vec<[f32; 3]> = p.positions.iter().map(|v| [v[0] * 0.8, 0.0, v[2] * 0.8]).collect();
        let lean: Vec<[f32; 3]> = p.normals.as_ref().unwrap().iter().map(|n| [n[0] * 0.2, 0.1, n[2] * 0.2]).collect();
        p.targets.push(MorphTarget { positions: swell, normals: Some(lean) });
        s.meshes[0].weights = vec![0.5];
        s.meshes[0].target_names = vec!["swell".into()];
        // The hip's cubic scale is the one channel Collada's 30 fps matrix
        // keys do not follow to 1e-5 (1.6e-5 off at 0.34 s). It has nothing to
        // do with morphs, and the matrix tests cover it; the test below judges
        // the morph under the skin at 1e-5, so the cubic channel stays out.
        s.animations[0].channels.retain(|c| c.interpolation != Interpolation::CubicSpline);
        s.animations[0].channels.push(Channel {
            node: 3,
            path: AnimPath::Weights,
            interpolation: Interpolation::Linear,
            times: vec![0.0, 1.0, 2.0],
            values: vec![0.0, 1.0, 0.25],
        });
        s
    }

    #[test]
    fn kept_morphs_round_trip_through_our_reader() {
        let fixture = fixtures::morph();
        let w = super::super::write_scene(&fixture, MeshFormat::Dae, keep_opts()).unwrap();
        let t = text(&w.package);
        assert!(t.contains("<morph ") && t.contains("method=\"NORMALIZED\""), "Keep writes a NORMALIZED morph controller");
        assert!(w.losses.iter().all(|l| l.kind != LossKind::MorphTargets), "no morph_targets loss under Keep: {:?}", w.losses);
        assert_eq!(w.package, package(&fixture, keep_opts()), "writing is deterministic");

        let back = read_package(&w.package).scene;
        assert_eq!(back.meshes.len(), 1, "one mesh");
        let (fm, bm) = (&fixture.meshes[0], &back.meshes[0]);
        assert_eq!(bm.target_names, fm.target_names, "target names");
        assert_eq!(bm.weights, fm.weights, "default weights");
        assert_eq!(bm.primitives.len(), 1);
        let (fp, bp) = (&fm.primitives[0], &bm.primitives[0]);
        assert_eq!(bp.targets.len(), fp.targets.len(), "target count");
        for (k, (b, f)) in bp.targets.iter().zip(&fp.targets).enumerate() {
            assert_eq!(b.positions.len(), f.positions.len(), "target {k} vertices");
            for (x, y) in b.positions.iter().zip(&f.positions) {
                assert!(close3(*x, *y, 1e-6), "target {k} position offset {x:?} vs {y:?}");
            }
            let (bn, fnn) = (b.normals.as_ref().expect("normal offsets read back"), f.normals.as_ref().unwrap());
            for (x, y) in bn.iter().zip(fnn) {
                assert!(close3(*x, *y, 1e-6), "target {k} normal offset {x:?} vs {y:?}");
            }
        }

        // The weight animation: the same weights at every sample time, and
        // the fixture's own keys among the written ones.
        assert_eq!(back.animations.len(), 1, "the clip");
        assert_eq!(back.animations[0].name, fixture.animations[0].name);
        let node = back.nodes.iter().position(|n| n.mesh.is_some()).expect("a node draws the mesh");
        let ch = back.animations[0].channels.iter().find(|c| c.path == AnimPath::Weights && c.node == node).expect("a weight channel");
        for k in &fixture.animations[0].channels[0].times {
            assert!(ch.times.iter().any(|t| (t - k).abs() < 1e-6), "key {k} among {:?}", ch.times);
        }
        for t in fixtures::sample_times(&fixture) {
            let (want, got) = (weights_of(&fixture, 0, 0, t), weights_of(&back, 0, node, t));
            assert_eq!(got.len(), want.len(), "weights at {t}: {got:?} vs {want:?}");
            assert!(want.iter().zip(&got).all(|(a, b)| (a - b).abs() < 1e-6), "weights at {t}: {got:?} vs {want:?}");
        }
        assert_posed_alike(&fixture, &back, 1e-5, "morph");
    }

    #[test]
    fn kept_morphs_under_a_skin_round_trip() {
        // SceneKit skins over a morph (measured, Task 0), so a skinned and
        // morphed mesh is written as <skin source="#…-morph">, not baked.
        let fixture = skinned_and_morphed();
        let pkg = package(&fixture, keep_opts());
        let root = super::super::xml::parse(&pkg.files[0].bytes).unwrap();
        let skins = root.descendants_named("skin");
        assert_eq!(skins.len(), 1);
        let source = ref_id(skins[0].attr("source").unwrap_or(""));
        assert!(source.ends_with("-morph"), "the skin's source is the morph controller: {source}");
        assert!(by_id(&root, source).child("morph").is_some(), "{source} is a <morph> controller");

        let back = read_package(&pkg).scene;
        assert_eq!(back.skins.len(), 1);
        assert_eq!(back.meshes.len(), 1);
        assert_eq!(back.meshes[0].target_count(), 1, "the target survives under the skin");
        assert_eq!(back.meshes[0].target_names, ["swell"]);
        assert_eq!(back.meshes[0].weights, [0.5]);
        assert!(back.animations[0].channels.iter().any(|c| c.path == AnimPath::Weights), "the weight animation survives");
        assert_posed_alike(&fixture, &back, 1e-5, "skinned and morphed");
    }

    #[test]
    fn kept_morph_normals_sit_in_vertices() {
        // The reader takes normal offsets only from a NORMAL inside
        // <vertices>, in the base and in every target; SceneKit morphs them.
        // An unmorphed mesh beside it keeps today's layout.
        let mut fixture = fixtures::morph();
        let mut flat = fixture.meshes[0].clone();
        flat.name = Some("flat".into());
        flat.primitives[0].targets.clear();
        (flat.weights, flat.target_names) = (Vec::new(), Vec::new());
        fixture.meshes.push(flat);
        fixture.add_root(Node { name: Some("flat_node".into()), mesh: Some(1), ..Default::default() });

        let root = kept_xml(&fixture);
        let morphs = morph_controllers(&root);
        assert_eq!(morphs.len(), 1, "only the morphed mesh has a controller");
        let (base, targets) = &morphs[0];
        assert_eq!(targets.len(), 2);
        for id in std::iter::once(base).chain(targets) {
            let mesh = by_id(&root, id).child("mesh").unwrap();
            let in_vertices = |e: &Element| e.children_named("input").any(|i| i.attr("semantic") == Some("NORMAL"));
            assert!(in_vertices(mesh.child("vertices").unwrap()), "{id}: NORMAL inside <vertices>");
            assert!(!in_vertices(mesh.child("triangles").unwrap()), "{id}: no NORMAL on <triangles>");
        }
        let plain = by_id(&root, "flat-mesh").child("mesh").unwrap();
        let normal_on = |e: &Element| e.children_named("input").any(|i| i.attr("semantic") == Some("NORMAL"));
        assert!(!normal_on(plain.child("vertices").unwrap()), "an unmorphed geometry keeps NORMAL on <triangles>");
        assert!(normal_on(plain.child("triangles").unwrap()));

        let back = kept_back(&fixture);
        let m = &back.meshes[back.nodes[0].mesh.unwrap()];
        for (k, (b, f)) in m.primitives[0].targets.iter().zip(&fixture.meshes[0].primitives[0].targets).enumerate() {
            let (bn, fnn) = (b.normals.as_ref().expect("normal offsets"), f.normals.as_ref().unwrap());
            assert_eq!(bn.len(), fnn.len());
            for (x, y) in bn.iter().zip(fnn) {
                assert!(close3(*x, *y, 1e-6), "target {k} normal offset {x:?} vs {y:?}");
            }
        }
    }

    #[test]
    fn a_multi_primitive_morph_comes_back_as_meshes() {
        // One controller a primitive; SceneKit keeps one <instance_controller>
        // a node, so the later ones sit in identity child nodes, and the
        // reader gives each controller its mesh and node (the spec's accepted
        // asymmetry). Names, default weights, targets and the drawing agree.
        let fixture = morph_three_primitives();
        let pkg = package(&fixture, keep_opts());
        let t = text(&pkg);
        assert_eq!(t.matches("<instance_controller").count(), 3, "{t}");
        let node = t.find("<node id=\"sheet_node\"").unwrap();
        let (part1, part2) = (t.find("<node id=\"sheet_node-morph1\"").expect("a node for the second"), t.find("<node id=\"sheet_node-morph2\"").expect("and the third"));
        assert!(node < part1 && part1 < part2, "the parts sit inside the node, after what it draws itself");
        assert_eq!(t[node..part1].matches("<instance_controller").count(), 1, "one on the node itself");
        assert_unique_ids(&super::super::xml::parse(&pkg.files[0].bytes).unwrap());

        let back = read_package(&pkg).scene;
        assert_eq!(back.meshes.len(), 3, "three meshes");
        assert_eq!(back.nodes.iter().filter(|n| n.mesh.is_some()).count(), 3, "three drawing nodes");
        for (mi, m) in back.meshes.iter().enumerate() {
            assert_eq!(m.primitives.len(), 1, "mesh {mi}");
            assert_eq!(m.target_names, fixture.meshes[0].target_names, "mesh {mi} target names");
            assert_eq!(m.weights, fixture.meshes[0].weights, "mesh {mi} default weights");
            assert_eq!(m.target_count(), 2, "mesh {mi} targets");
        }
        assert_posed_alike(&fixture, &back, 1e-5, "three primitives");
    }

    #[test]
    fn instances_with_their_own_weights_get_their_own_controllers() {
        // <instance_controller> has no weight override, so a node whose
        // weights differ needs a controller of its own; nodes whose weights
        // agree (and that no clip animates) share one. The geometries are
        // shared by all.
        let fixture = morph_instances();
        let pkg = package(&fixture, keep_opts());
        let root = super::super::xml::parse(&pkg.files[0].bytes).unwrap();
        assert_eq!(root.descendants_named("morph").len(), 3, "the animated node, the copies' shared set, copy_c's");
        assert_eq!(root.descendants_named("geometry").len(), 3, "the base and its two targets, shared by every controller");
        assert_eq!(root.descendants_named("instance_controller").len(), 4, "a node each");
        assert_unique_ids(&root);

        let back = read_package(&pkg).scene;
        let drawn: Vec<usize> = (0..back.nodes.len()).filter(|&i| back.nodes[i].mesh.is_some()).collect();
        assert_eq!(drawn.len(), 4);
        let weights: Vec<Vec<f32>> = drawn.iter().map(|&i| back.meshes[back.nodes[i].mesh.unwrap()].weights.clone()).collect();
        assert_eq!(weights, [vec![0.25, 0.0], vec![1.0, 0.5], vec![1.0, 0.5], vec![0.0, 1.0]]);
        assert_eq!(back.nodes[drawn[1]].mesh, back.nodes[drawn[2]].mesh, "the copies with one set of weights share a mesh");
        let animated: Vec<usize> =
            back.animations[0].channels.iter().filter(|c| c.path == AnimPath::Weights).map(|c| c.node).collect();
        assert_eq!(animated, [drawn[0]], "only the first node's weights are animated");
        assert_posed_alike(&fixture, &back, 1e-5, "instances");
    }

    #[test]
    fn targets_without_normal_offsets_still_carry_normals() {
        // A target with no normal offsets beside one with them: its geometry
        // still carries normals (the base's), so a reader that morphs them
        // blends toward the base, not toward nothing.
        let mut fixture = fixtures::morph();
        let n = fixture.meshes[0].primitives[0].positions.len();
        fixture.meshes[0].primitives[0].targets[0].normals = None;
        fixture.meshes[0].primitives[0].targets[1].normals = Some(vec![[0.0, 0.05, 0.0]; n]);
        let root = kept_xml(&fixture);
        let morphs = morph_controllers(&root);
        assert_eq!(morphs.len(), 1, "a morph controller");
        let (base, targets) = morphs[0].clone();
        let base_normals = vertices_floats(&root, &base, "NORMAL").expect("the base's normals sit in <vertices>");
        let bare = vertices_floats(&root, &targets[0], "NORMAL").expect("the target with no offsets carries normals");
        assert_eq!(bare, base_normals, "the base's own, for a target with no offsets");
        let tilted = vertices_floats(&root, &targets[1], "NORMAL").expect("the other target's normals");
        assert_ne!(tilted, base_normals);

        let back = kept_back(&fixture);
        let p = &back.meshes[back.nodes[0].mesh.unwrap()].primitives[0];
        let zero = p.targets[0].normals.as_ref().expect("zero offsets read back");
        assert!(zero.iter().all(|o| close3(*o, [0.0; 3], 1e-6)), "no offsets stay no offsets");
        let tilt = p.targets[1].normals.as_ref().unwrap();
        assert!(tilt.iter().all(|o| close3(*o, [0.0, 0.05, 0.0], 1e-6)), "{:?}", tilt[0]);
        // `prepare` models it: the reader's zero offsets are in its scene.
        let prepared = prepare(&fixture, keep_opts()).unwrap();
        assert_eq!(prepared.meshes[0].primitives[0].targets[0].normals, Some(vec![[0.0; 3]; n]));
        assert_posed_alike(&fixture, &back, 1e-5, "one target without normal offsets");

        // And with none of them having any.
        for t in fixture.meshes[0].primitives[0].targets.iter_mut() {
            t.normals = None;
        }
        let root = kept_xml(&fixture);
        let morphs = morph_controllers(&root);
        assert_eq!(morphs.len(), 1, "a morph controller");
        let (base, targets) = morphs[0].clone();
        let base_normals = vertices_floats(&root, &base, "NORMAL").expect("base normals");
        for id in &targets {
            assert_eq!(vertices_floats(&root, id, "NORMAL").as_ref(), Some(&base_normals), "{id}");
        }
        assert_posed_alike(&fixture, &kept_back(&fixture), 1e-5, "no target with normal offsets");
    }

    /// The morph fixture with its one weight channel replaced.
    fn morph_with_weight_keys(interpolation: Interpolation, times: &[f32], values: &[f32]) -> Scene {
        let mut s = fixtures::morph();
        s.animations[0].channels[0] =
            Channel { node: 0, path: AnimPath::Weights, interpolation, times: times.to_vec(), values: values.to_vec() };
        s
    }

    #[test]
    fn step_and_cubic_weight_animation_is_resampled() {
        // Weights follow the matrix channels' rules: sampled at every source
        // key and at 30 fps, LINEAR, a key just before each STEP key, and a
        // key at 0 holding the first value, which overrides the defaults.
        let step = morph_with_weight_keys(Interpolation::Step, &[0.5, 1.0, 1.5], &[0.9, 0.1, 0.2, 0.8, 0.6, 0.4]);
        let back = kept_back(&step);
        let node = back.nodes.iter().position(|n| n.mesh.is_some()).unwrap();
        let weights_channel = |s: &Scene| {
            s.animations.first().and_then(|a| a.channels.iter().find(|c| c.path == AnimPath::Weights)).cloned().expect("a weight channel")
        };
        assert_eq!(weights_channel(&back).times[0], 0.0, "a key at 0");
        for t in [0.0, 0.25, 0.5, 0.75, 0.9998, 1.0, 1.25, 1.4998, 1.5, 1.9] {
            let (want, got) = (weights_of(&step, 0, 0, t), weights_of(&back, 0, node, t));
            assert_eq!(got.len(), want.len(), "step at {t}: {got:?} vs {want:?}");
            assert!(want.iter().zip(&got).all(|(a, b)| (a - b).abs() < 1e-6), "step at {t}: {got:?} vs {want:?}");
        }
        assert_ne!(weights_of(&back, 0, node, 0.0), back.meshes[0].weights, "the clip's first value overrides the defaults from 0");

        // Zero tangents: a smoothstep between the keys, far from the straight
        // line a reader would draw if only the keys were written.
        #[rustfmt::skip]
        let keys = [
            0.0, 0.0,  0.0, 1.0,  0.0, 0.0, // in-tangent, value, out-tangent
            0.0, 0.0,  1.0, 0.0,  0.0, 0.0,
            0.0, 0.0,  0.0, 1.0,  0.0, 0.0,
        ];
        let cubic = morph_with_weight_keys(Interpolation::CubicSpline, &[0.0, 1.0, 2.0], &keys);
        let back = kept_back(&cubic);
        let node = back.nodes.iter().position(|n| n.mesh.is_some()).unwrap();
        assert!(weights_channel(&back).times.len() >= 60, "sampled finely, not just at its keys");
        for t in [0.25, 0.5, 0.75, 1.25, 1.75] {
            let (want, got) = (weights_of(&cubic, 0, 0, t), weights_of(&back, 0, node, t));
            assert_eq!(got.len(), want.len(), "cubic at {t}: {got:?} vs {want:?}");
            assert!(want.iter().zip(&got).all(|(a, b)| (a - b).abs() < 2e-3), "cubic at {t}: {got:?} vs {want:?}");
        }
        let straight = 0.25;
        assert!((weights_of(&cubic, 0, 0, 0.25)[0] - straight).abs() > 0.05, "the cubic is not the straight line");
    }

    #[test]
    fn a_morphed_mesh_under_an_animated_parent_keeps_both() {
        let mut fixture = fixtures::morph();
        fixture.nodes.push(Node { name: Some("rig".into()), children: vec![0], ..Default::default() });
        fixture.roots = vec![1];
        fixture.animations[0].channels.push(Channel {
            node: 1,
            path: AnimPath::Translation,
            interpolation: Interpolation::Linear,
            times: vec![0.0, 1.0, 2.0],
            values: vec![0.0, 0.0, 0.0, 2.0, 0.5, 0.0, 2.0, 1.0, 0.0],
        });
        let pkg = package(&fixture, keep_opts());
        let t = text(&pkg);
        let node = t.find("<node id=\"sheet_node\"").unwrap();
        let close = t[node..].find("</node>").unwrap() + node;
        assert!(t[node..close].contains("<instance_controller"), "the controller is drawn from the node under the parent");
        assert!(t.contains("target=\"rig/transform\""), "the parent's transform animation");
        assert!(t.contains("-weights(0)\"") && t.contains("-weights(1)\""), "and the weight animation");

        let back = read_package(&pkg).scene;
        let channels = &back.animations[0].channels;
        assert!(channels.iter().any(|c| c.path == AnimPath::Translation), "the parent still moves");
        assert!(channels.iter().any(|c| c.path == AnimPath::Weights), "and the weights still play");
        assert_posed_alike(&fixture, &back, 1e-5, "morph under an animated parent");
    }

    #[test]
    fn weight_channels_target_the_morph_weight_source() {
        // SceneKit plays a weight channel only when it names the MORPH_WEIGHT
        // <source>; one naming the <float_array> inside it plays nothing (Task
        // 0). Our reader accepts both, so only this reading of the XML sees it.
        for (name, fixture) in [("morph", fixtures::morph()), ("three primitives", morph_three_primitives())] {
            let root = kept_xml(&fixture);
            let mut weight_channels = 0;
            for channel in root.descendants_named("channel") {
                let target = channel.attr("target").unwrap_or("");
                let Some((id, index)) = target.split_once('(') else { continue };
                assert!(index.ends_with(')'), "{name}: {target}");
                weight_channels += 1;
                let element = by_id(&root, id);
                assert_eq!(element.name, "source", "{name}: {target} names a <{}>, not a <source>", element.name);
                let morph_weight = root.descendants_named("input").into_iter().any(|i| {
                    i.attr("semantic") == Some("MORPH_WEIGHT") && i.attr("source").map(ref_id) == Some(id)
                });
                assert!(morph_weight, "{name}: {target}: no <targets> input MORPH_WEIGHT names it");
            }
            let controllers = root.descendants_named("morph").len();
            assert!(controllers > 0, "{name}: Keep writes morph controllers");
            assert_eq!(weight_channels, 2 * controllers, "{name}: a channel for each weight of each controller");
        }
    }

    #[test]
    fn kept_morph_weights_in_two_clips_round_trip() {
        // No harness judge sees this: SceneKit binds keys by target, so two
        // clips animating one node replace each other's keys there, and
        // three.js refuses the file. Under Keep both clips animate one node's
        // weights with keys of their own; each must read back with its own
        // keys, and draw what the source draws at every sample time of the
        // clip (Ruling DM21).
        let mut fixture = fixtures::morph();
        fixture.animations.push(Animation {
            name: Some("Pulse".into()),
            channels: vec![Channel {
                node: 0,
                path: AnimPath::Weights,
                interpolation: Interpolation::Linear,
                // Other keys than "Breathe"'s (0, 0.8, 2.0), and another length.
                times: vec![0.25, 0.5, 1.5, 2.5],
                values: vec![1.0, 0.0, 0.2, 0.9, 0.7, 0.1, 0.0, 0.4],
            }],
        });
        let back = kept_back(&fixture);
        assert_eq!(back.animations.len(), 2, "both clips come back");
        for clip in 0..2 {
            assert_eq!(back.animations[clip].name, fixture.animations[clip].name, "clip {clip}'s name");
            let (want, got) = (&fixture.animations[clip].channels, &back.animations[clip].channels);
            assert_eq!(got.len(), want.len(), "clip {clip}'s channels");
            // Each clip is judged on its own: put it first, which is the clip
            // `fixtures::sample_times` and `assert_posed_alike` follow.
            let (mut first_want, mut first_got) = (fixture.clone(), back.clone());
            first_want.animations.swap(0, clip);
            first_got.animations.swap(0, clip);
            let times = fixtures::sample_times(&first_want);
            assert_eq!(times.len(), 6, "clip {clip} has sample times");
            for &t in &times {
                let (w, g) = (weights_of(&fixture, clip, 0, t), weights_of(&back, clip, 0, t));
                assert_eq!(w.len(), g.len(), "clip {clip} @ {t}: weights");
                for (i, (a, b)) in w.iter().zip(&g).enumerate() {
                    assert!((a - b).abs() <= 1e-5, "clip {clip} @ {t}: weight {i}: {a} vs {b}");
                }
            }
            assert_posed_alike(&first_want, &first_got, 1e-5, &format!("clip {clip}"));
        }
    }

    #[test]
    fn prepare_under_keep_keeps_the_morph_fixtures_targets_and_weight_channel() {
        let s = fixtures::morph();
        let kept = prepare(&s, keep_opts()).unwrap();
        assert_eq!(kept.meshes[0].target_count(), 2);
        assert_eq!(kept.meshes[0].target_names, s.meshes[0].target_names);
        assert_eq!(kept.meshes[0].weights, s.meshes[0].weights);
        let weights = |s: &Scene| -> Vec<Channel> {
            s.animations.iter().flat_map(|a| a.channels.iter().filter(|c| c.path == AnimPath::Weights).cloned()).collect()
        };
        assert_eq!(weights(&kept), weights(&s), "the weight channel is kept whole");
        // Bake applies the targets and drops the channel.
        let baked = prepare(&s, WriteOptions::default()).unwrap();
        assert_eq!(baked.meshes[0].target_count(), 0);
        assert!(weights(&baked).is_empty());
    }

    #[test]
    fn keep_does_not_charge_morphed_instances_as_copied_geometry() {
        // Bake gives every morphed instance its own baked copy of the mesh,
        // so a hundred instances of a 40,000-vertex mesh pass the scene's
        // 4,000,000-vertex limit. Keep draws them all from the shared
        // geometries through controllers, and copies nothing.
        const VERTICES: usize = 40_000;
        let mesh = SceneMesh {
            name: Some("big".into()),
            primitives: vec![Primitive {
                positions: (0..VERTICES).map(|i| [i as f32 * 1e-3, 0.0, 0.0]).collect(),
                indices: vec![0, 1, 2],
                targets: vec![MorphTarget { positions: vec![[0.0, 1.0, 0.0]; VERTICES], normals: None }],
                ..Default::default()
            }],
            weights: vec![0.5],
            target_names: Vec::new(),
        };
        let mut s = Scene { meshes: vec![mesh], ..Default::default() };
        for i in 0..MAX_SCENE_VERTICES / VERTICES + 1 {
            s.add_root(Node { name: Some(format!("copy{i}")), mesh: Some(0), ..Default::default() });
        }
        let bake = super::super::write_scene(&s, MeshFormat::Dae, WriteOptions::default());
        assert!(matches!(&bake, Err(MeshError::Unsupported(m)) if m.contains("vertices")), "Bake copies per instance: {:?}", bake.err());
        let keep = super::super::write_scene(&s, MeshFormat::Dae, WriteOptions { dae_morphs: DaeMorphs::Keep, ..Default::default() })
            .unwrap_or_else(|e| panic!("Keep shares the geometry: {e}"));
        assert_eq!(text(&keep.package).matches("<controller ").count(), 1, "every instance shares one controller");
    }

    // ── final fix: Keep's controllers are charged (Ruling DM17) ─────────────

    /// `nodes` roots drawing one mesh of `primitives` one-triangle primitives
    /// with `targets` morph targets each, every node at weights of its own:
    /// under `Keep`, a controller set (and so a controller per primitive) for
    /// each node.
    fn weighted_nodes(nodes: usize, primitives: usize, targets: usize) -> Scene {
        let primitive = Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            targets: vec![MorphTarget { positions: vec![[0.0, 0.0, 1.0]; 3], normals: None }; targets],
            ..Default::default()
        };
        let mesh = SceneMesh {
            name: Some("shared".into()),
            primitives: vec![primitive; primitives],
            weights: vec![0.0; targets],
            target_names: Vec::new(),
        };
        let mut s = Scene { meshes: vec![mesh], ..Default::default() };
        for i in 0..nodes {
            s.add_root(Node {
                name: Some(format!("n{i}")),
                mesh: Some(0),
                weights: Some(vec![(i + 1) as f32 / 10_000.0; targets]),
                ..Default::default()
            });
        }
        s
    }

    #[test]
    fn keep_charges_its_controllers_against_the_vertex_ceiling() {
        // Each (set, primitive) controller lists every target of its mesh in
        // an IDREF_array and a float_array. Nodes at weights of their own are
        // a set each, so 1,001 nodes over 4 primitives of 1,000 targets write
        // 4,004,000 target references, which Keep copies no geometry for but
        // which are the largest thing it writes. Counted as vertices they pass
        // MAX_SCENE_VERTICES; counted by node and target alone (1,001,000),
        // as the primitives are forgotten, they would not.
        let (nodes, primitives, targets) = (1_001, 4, 1_000);
        assert!(nodes * targets < MAX_SCENE_VERTICES, "counted without the primitives, the scene passes");
        assert!(nodes * primitives * targets > MAX_SCENE_VERTICES, "counted as Keep writes it, it does not");
        let s = weighted_nodes(nodes, primitives, targets);
        let keep = super::super::write_scene(&s, MeshFormat::Dae, keep_opts());
        let message = match &keep {
            Err(MeshError::Unsupported(m)) => m.clone(),
            other => panic!("Keep refuses it before writing anything: {:?}", other.as_ref().map(|w| w.package.files.len())),
        };
        assert!(message.contains("controller"), "the refusal names what it counted: {message}");
        for count in [format!("{nodes} nodes"), format!("{nodes} sets"), format!("{} target references", nodes * primitives * targets)] {
            assert!(message.contains(&count), "the refusal names {count}: {message}");
        }
        // Bake writes no controller, and charges its copies per instance
        // (1,001 nodes of 12 vertices): this check does not refuse it.
        let bake = super::super::write_scene(&s, MeshFormat::Dae, WriteOptions::default());
        assert!(bake.is_ok(), "Bake is not refused by the controller check: {:?}", bake.err());
    }

    #[test]
    fn keep_charges_a_skin_controller_per_set() {
        // A skin over a morph is a controller of its own for each (mesh, skin,
        // morph set), with the primitive's joint and weight arrays per vertex
        // (Ruling DM22). Nodes at distinct weights drawing one skinned and
        // morphed mesh are a set each: with one target the morph controllers
        // are cheap, and the skin controllers are the whole bill.
        let fixture = skinned_and_morphed();
        let vertices = fixture.meshes[0].primitives[0].positions.len();
        let copy = |mut s: Scene, nodes: usize| {
            for i in 0..nodes {
                let weights = Some(vec![(i + 1) as f32 / 100_000.0]);
                s.add_root(Node { name: Some(format!("copy{i}")), mesh: Some(0), skin: Some(0), weights, ..Default::default() });
            }
            s
        };
        let many = copy(fixture.clone(), MAX_SCENE_VERTICES / vertices + 1);
        let keep = super::super::write_scene(&many, MeshFormat::Dae, keep_opts());
        let message = match &keep {
            Err(MeshError::Unsupported(m)) => m.clone(),
            other => panic!("Keep refuses it before writing anything: {:?}", other.as_ref().map(|w| w.package.files.len())),
        };
        let sets = MAX_SCENE_VERTICES / vertices + 2;
        assert!(message.contains(&format!("{sets} skin controllers")), "the refusal names the skin controllers: {message}");
        assert!(message.contains(&format!("{sets} sets")), "and the sets: {message}");
        // The target references alone (one a set) are nowhere near the ceiling.
        assert!(sets < MAX_SCENE_VERTICES, "the morph controllers do not account for the refusal");
        // Bake copies the mesh to every morphed node, and refuses it as ever.
        let bake = super::super::write_scene(&many, MeshFormat::Dae, WriteOptions::default());
        assert!(matches!(&bake, Err(MeshError::Unsupported(m)) if m.contains("the meshes pass")), "Bake: {:?}", bake.err());
        // A few sets are not refused, and write a skin controller for each.
        let few = copy(fixture, 3);
        let pkg = super::super::write_scene(&few, MeshFormat::Dae, keep_opts()).unwrap_or_else(|e| panic!("a few sets are fine: {e}")).package;
        let root = super::super::xml::parse(&pkg.files[0].bytes).unwrap();
        assert_eq!(root.descendants_named("skin").len(), 4, "a skin controller for each of the four sets");
    }

    #[test]
    fn keep_charges_each_skin_controllers_joints() {
        // A skin controller also writes the skin's joint names and one 4x4 bind
        // pose a joint, once for each (mesh, skin, morph set) (Ruling DM23). A
        // small mesh whose skin has many joints is cheap in vertices and in
        // targets, and the joint arrays are the whole bill.
        const JOINTS: usize = 1000;
        let mut fixture = skinned_and_morphed();
        let vertices = fixture.meshes[0].primitives[0].positions.len();
        while fixture.skins[0].joints.len() < JOINTS {
            let name = format!("extra{}", fixture.skins[0].joints.len());
            let node = fixture.add_root(Node { name: Some(name), ..Default::default() });
            fixture.skins[0].joints.push(node);
            fixture.skins[0].inverse_bind_matrices.push(math::IDENTITY);
        }
        let copy = |mut s: Scene, nodes: usize| {
            for i in 0..nodes {
                let weights = Some(vec![(i + 1) as f32 / 100_000.0]);
                s.add_root(Node { name: Some(format!("copy{i}")), mesh: Some(0), skin: Some(0), weights, ..Default::default() });
            }
            s
        };
        // One more set than the 17 numbers a joint puts over the ceiling.
        let sets = MAX_SCENE_VERTICES / (17 * JOINTS) + 2;
        // The vertex and target terms alone (the skin's vertices, the mesh's
        // vertices and one target reference a set) are nowhere near it.
        assert!(sets * (vertices + 1) + vertices < MAX_SCENE_VERTICES, "the vertex terms do not account for the refusal");
        let many = copy(fixture.clone(), sets - 1);
        let keep = super::super::write_scene(&many, MeshFormat::Dae, keep_opts());
        let message = match &keep {
            Err(MeshError::Unsupported(m)) => m.clone(),
            other => panic!("Keep refuses it before writing anything: {:?}", other.as_ref().map(|w| w.package.files.len())),
        };
        assert!(message.contains(&format!("{sets} skin controllers")), "the refusal names the skin controllers: {message}");
        assert!(message.contains(&format!("{} joints", sets * JOINTS)), "and their joints: {message}");
        // A few sets of the same skin are not refused, and each writes the
        // joint names and bind poses: a skin of its own for each of the four.
        let few = copy(fixture, 3);
        let pkg = super::super::write_scene(&few, MeshFormat::Dae, keep_opts()).unwrap_or_else(|e| panic!("a few sets are fine: {e}")).package;
        let root = super::super::xml::parse(&pkg.files[0].bytes).unwrap();
        assert_eq!(root.descendants_named("skin").len(), 4, "a skin controller for each of the four sets");
        // (The weight animation's interpolation names are Name_arrays too.)
        let names: Vec<_> = root.descendants_named("Name_array").into_iter().filter(|a| a.attr("id").is_some_and(|id| id.contains("joints"))).collect();
        assert_eq!(names.len(), 4, "and a joint name array for each");
        assert!(names.iter().all(|a| a.attr("count") == Some("1000")), "of every joint of the skin");
    }

    #[test]
    fn keep_does_not_refuse_a_few_controller_sets_of_many_targets() {
        // Five sets of the widest mesh the scene allows are 5,120 references.
        let s = weighted_nodes(5, 1, MAX_MORPH_TARGETS);
        let pkg = super::super::write_scene(&s, MeshFormat::Dae, keep_opts()).unwrap_or_else(|e| panic!("a few sets are fine: {e}")).package;
        assert_eq!(text(&pkg).matches("<controller ").count(), 5, "a controller for each set");
    }

    // ── fix round 1: the resample budget, target names, skinned morphs ──────

    /// One mesh of `primitives` one-triangle primitives with `targets` morph
    /// targets each, drawn by one node whose weights a clip of two LINEAR keys,
    /// `seconds` apart, animates.
    fn many_primitive_morph(primitives: usize, targets: usize, seconds: f32) -> Scene {
        let primitive = Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            targets: vec![MorphTarget { positions: vec![[0.0, 0.0, 1.0]; 3], normals: None }; targets],
            ..Default::default()
        };
        let mesh = SceneMesh {
            name: Some("wide".into()),
            primitives: vec![primitive; primitives],
            weights: vec![0.0; targets],
            target_names: Vec::new(),
        };
        let mut s = Scene { meshes: vec![mesh], ..Default::default() };
        s.add_root(Node { name: Some("wide_node".into()), mesh: Some(0), ..Default::default() });
        s.animations.push(Animation {
            name: None,
            channels: vec![Channel {
                node: 0,
                path: AnimPath::Weights,
                interpolation: Interpolation::Linear,
                times: vec![0.0, seconds],
                values: vec![0.0; 2 * targets],
            }],
        });
        s
    }

    #[test]
    fn keep_writes_three_values_per_primitive_target_and_time() {
        // What `write_scene`'s resample budget charges a weight channel under
        // Keep (DM13): one <animation> for each primitive and target, holding
        // a TIME, a WEIGHT and an INTERPOLATION array as long as the time grid.
        let s = many_primitive_morph(6, 5, 2.0);
        let times = pose::resample_times(&s, 0, 30.0).len();
        let root = kept_xml(&s);
        let mut written = 0;
        for a in root.descendants_named("animation") {
            for array in a.descendants_named("float_array").into_iter().chain(a.descendants_named("Name_array")) {
                written += array.attr("count").and_then(|c| c.parse::<usize>().ok()).expect("an array count");
            }
        }
        assert_eq!(root.descendants_named("animation").len(), 6 * 5, "an animation for each primitive and target");
        assert_eq!(written, times * 6 * 5 * 3, "three arrays of the time grid's length each");
    }

    #[test]
    fn keep_refuses_a_weight_animation_only_its_primitives_push_over_the_resample_budget() {
        // 60 primitives of 60 targets over a 100 s clip, 3,001 times. Counted
        // by target alone that is 180,060 values; written, 60 x 60 x 3 a time,
        // it is over MAX_RESAMPLED_VALUES.
        let s = many_primitive_morph(60, 60, 100.0);
        let times = pose::resample_times(&s, 0, 30.0).len();
        assert!(times * 60 <= pose::MAX_RESAMPLED_VALUES, "counted by target alone, the clip passes");
        assert!(times * 60 * 60 * 3 > pose::MAX_RESAMPLED_VALUES, "counted as Keep writes it, it does not");
        let keep = super::super::write_scene(&s, MeshFormat::Dae, keep_opts());
        assert!(
            matches!(&keep, Err(MeshError::Unsupported(m)) if m.contains("keyed values")),
            "Keep refuses it before sampling anything: {:?}",
            keep.err()
        );
        // The refusal names the weights the clip has and the values a key
        // writes for them, so the number that crossed the cap is in it (DM13).
        let Err(MeshError::Unsupported(message)) = &keep else { unreachable!("matched above") };
        assert!(
            message.contains("60 morph weights, written as 10800 values a key"),
            "the refusal names both counts: {message}"
        );
        // Bake writes no weight animation, so the same scene passes.
        let bake = super::super::write_scene(&s, MeshFormat::Dae, WriteOptions::default());
        assert!(bake.is_ok(), "Bake passes the budget: {:?}", bake.err());
    }

    #[test]
    fn the_resample_refusal_keeps_its_text_where_values_equal_weights() {
        // FBX, and Collada baking, write one value a target and time, so the
        // weights are the values and the text says only so: twelve clips'
        // worth of channels over 1,024 targets and 3,001 times.
        let mut s = many_primitive_morph(1, 1024, 100.0);
        let channel = s.animations[0].channels[0].clone();
        s.animations[0].channels.extend(std::iter::repeat_n(channel, 11));
        let err = pose::check_resample_budget(&s, 30.0, |m| m.target_count()).expect_err("12 x 1,024 x 3,001 passes 32M");
        let MeshError::Unsupported(message) = err else { panic!("unsupported: {err:?}") };
        assert_eq!(
            message,
            format!(
                "the animation would resample to more than {} keyed values (clip 0: 0 nodes, {} morph weights)",
                pose::MAX_RESAMPLED_VALUES,
                12 * 1024
            )
        );
    }

    #[test]
    fn prepare_under_keep_names_every_target_as_it_is_written() {
        // The reader names a target by its geometry's `name`, else by its id,
        // and the writer's names are NCNames (DM14). `prepare` says what the
        // reader will give back: an unnamed target `target{i}`, a name with a
        // space the underscored one.
        let mut fixture = fixtures::morph();
        let cases: [(Vec<String>, [&str; 2]); 2] = [
            (vec![String::new(), "has space".into()], ["target0", "has_space"]),
            (Vec::new(), ["target0", "target1"]),
        ];
        for (names, want) in cases {
            fixture.meshes[0].target_names = names;
            let prepared = prepare(&fixture, keep_opts()).unwrap();
            assert_eq!(prepared.meshes[0].target_names, want, "the names prepare gives");
            assert_eq!(
                kept_back(&fixture).meshes[0].target_names,
                prepared.meshes[0].target_names,
                "the reader gives back the names prepare says"
            );
            let root = kept_xml(&fixture);
            let (_, targets) = morph_controllers(&root).remove(0);
            let written: Vec<&str> = targets.iter().map(|id| by_id(&root, id).attr("name").unwrap_or("")).collect();
            assert_eq!(written, want, "each target geometry is written with its name");
        }
    }

    #[test]
    fn a_skinned_morph_on_two_nodes_at_different_weights_gets_a_skin_over_each_morph() {
        // The skin controller's key is (mesh, skin, morph set): a second node
        // that draws the mesh at other weights needs a morph controller of its
        // own, and a skin over it.
        let mut fixture = skinned_and_morphed();
        let transform = Transform::Trs { translation: [2.0, 0.0, 0.0], rotation: math::QUAT_IDENTITY, scale: [1.0; 3] };
        fixture.add_root(Node {
            name: Some("tube_copy".into()),
            transform,
            mesh: Some(0),
            skin: Some(0),
            weights: Some(vec![1.0]),
            ..Default::default()
        });
        let pkg = package(&fixture, keep_opts());
        let root = super::super::xml::parse(&pkg.files[0].bytes).unwrap();
        assert_unique_ids(&root);
        assert_eq!(root.descendants_named("morph").len(), 2, "a morph controller for each set of weights");
        let skins = root.descendants_named("skin");
        let sources: HashSet<&str> = skins.iter().map(|s| ref_id(s.attr("source").unwrap_or(""))).collect();
        assert_eq!((skins.len(), sources.len()), (2, 2), "a skin over each morph: {sources:?}");
        for source in &sources {
            assert!(by_id(&root, source).child("morph").is_some(), "{source} is a <morph> controller");
        }
        assert_posed_alike(&fixture, &read_package(&pkg).scene, 1e-5, "skinned morph on two nodes");
    }

    #[test]
    fn a_skinned_multi_primitive_morph_keeps_a_skin_over_each_primitives_morph() {
        // `skin_part_ids` with a morph: the second primitive's skin controller
        // is drawn from an identity child node, over its own morph controller.
        let mut fixture = skinned_and_morphed();
        let mut second = fixture.meshes[0].primitives[0].clone();
        for v in second.positions.iter_mut() {
            v[0] += 2.0;
        }
        fixture.meshes[0].primitives.push(second);
        let pkg = package(&fixture, keep_opts());
        let t = text(&pkg);
        let root = super::super::xml::parse(&pkg.files[0].bytes).unwrap();
        assert_unique_ids(&root);
        assert_eq!(root.descendants_named("morph").len(), 2, "a morph controller a primitive");
        assert_eq!(root.descendants_named("skin").len(), 2, "and a skin over each");
        assert_eq!(t.matches("<instance_controller").count(), 2);
        let node = t.find("<node id=\"tube_node\"").expect("the node");
        let part = t.find("<node id=\"tube_node-skin1\"").expect("the second primitive's skin sits in a child node");
        assert!(node < part);
        assert_posed_alike(&fixture, &read_package(&pkg).scene, 1e-5, "skinned multi-primitive morph");
    }

    /// The `set` of every TEXCOORD input of a written document, in order.
    fn texcoord_sets(doc: &str) -> Vec<&str> {
        doc.split(r#"<input semantic="TEXCOORD""#).skip(1).filter_map(|rest| rest.split(r#" set=""#).nth(1)?.split('"').next()).collect()
    }

    #[test]
    fn dae_writes_and_reads_every_uv_set() {
        // Collada holds eight sets, each a TEXCOORD `<source>` in f64, so
        // multiuv's four come back bit for bit. A package puts the base
        // colour's set first, then the rest by index: the fixture's 3, 0, 1
        // and 2. Collada keeps no occlusion map, so set 1 is not sampled here
        // and keeps its place among the rest. The binding names the new index.
        // A single file has no textures and keeps scene order.
        let fixture = fixtures::multiuv();
        let source = &fixture.meshes[0].primitives[0];
        for (single, order, base) in [(false, [3, 0, 1, 2], Some(0)), (true, [0, 1, 2, 3], None)] {
            let ctx = if single { "one file" } else { "package" };
            let opts = WriteOptions { self_contained: single, ..Default::default() };
            let w = super::super::write_scene(&fixture, MeshFormat::Dae, opts).unwrap();
            assert!(!w.losses.iter().any(|l| l.kind == LossKind::Uvs), "{ctx}: {:?}", w.losses);
            let doc = text(&w.package);
            assert_eq!(texcoord_sets(&doc), ["0", "1", "2", "3"], "{ctx}: a TEXCOORD input per set");
            let back = read_package(&w.package).scene;
            let p = &back.meshes[0].primitives[0];
            let want = Primitive { uvs: order.map(|i| source.uvs[i].clone()).to_vec(), ..source.clone() };
            assert_eq!(corner_uvs(p), corner_uvs(&want), "{ctx}: the fixture's sets {order:?}, bit for bit");
            let m = p.material.map(|m| &back.materials[m]).expect("a material");
            assert_eq!(m.base_color_texture.as_ref().map(|t| t.uv_set), base, "{ctx}: the base colour's set");
            assert!(m.occlusion_texture.is_none(), "{ctx}: no occlusion map");
            if !single {
                assert!(doc.contains(r#"texcoord="UVSET0""#), "{ctx}: the base colour samples UVSET0");
                let bound = r#"<bind_vertex_input semantic="UVSET0" input_semantic="TEXCOORD" input_set="0"/>"#;
                assert!(doc.contains(bound), "{ctx}: UVSET0 is set 0");
            }
        }
    }

    // ── reader: geometry ────────────────────────────────────────────────────

    #[test]
    fn reads_polylists_ear_clipping_concave_polygons() {
        // A quad and a dart whose reflex corner is (1,1): fanned from its
        // first corner it would cover the notch and flip a triangle.
        let pos = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0, 3.0, 0.0, 0.0, 5.0, 1.0, 0.0, 3.0, 2.0, 0.0, 4.0, 1.0, 0.0];
        let body = scene_doc(
            &geom("g", &pos, r##"<polylist count="2"><input semantic="VERTEX" source="#g-v" offset="0"/><vcount>4 4</vcount><p>0 1 2 3 4 5 6 7</p></polylist>"##),
            r##"<node id="n"><instance_geometry url="#g"/></node>"##,
        );
        let s = read_doc(&body).unwrap().scene;
        let areas = signed_area_z(&s);
        assert_eq!(areas.len(), 4);
        assert!(areas.iter().all(|a| *a > 0.0), "every triangle keeps the polygon's winding: {areas:?}");
        let total: f32 = areas.iter().sum();
        assert!((total - 2.0).abs() < 1e-5, "quad 1 + dart 1, no overlap: {total}");
    }

    #[test]
    fn reads_polygons_fans_and_strips_with_their_winding() {
        let pos = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0];
        for (kind, p) in [("polygons", "<p>0 1 2 3</p>"), ("trifans", "<p>0 1 2 3</p>"), ("tristrips", "<p>0 1 3 2</p>")] {
            let body = scene_doc(
                &geom("g", &pos, &format!(r##"<{kind} count="1"><input semantic="VERTEX" source="#g-v" offset="0"/>{p}</{kind}>"##)),
                r##"<node id="n"><instance_geometry url="#g"/></node>"##,
            );
            let s = read_doc(&body).unwrap_or_else(|e| panic!("{kind}: {e}")).scene;
            let areas = signed_area_z(&s);
            assert_eq!(areas.len(), 2, "{kind}");
            assert!(areas.iter().all(|a| (*a - 0.5).abs() < 1e-6), "{kind}: {areas:?}");
        }
    }

    /// Was `skips_lines_with_a_warning_and_refuses_holes` (mesh-gaps item 10,
    /// Task 4): `<lines>` were skipped with a warning, and are now read as
    /// line segments beside the triangles, with nothing said. Holes are still
    /// refused.
    #[test]
    fn reads_lines_beside_triangles_and_refuses_holes() {
        let body = scene_doc(
            &geom("g", &TRI, r##"<triangles count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><p>0 1 2</p></triangles><lines count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><p>0 1</p></lines>"##),
            r##"<node id="n"><instance_geometry url="#g"/></node>"##,
        );
        let r = read_doc(&body).unwrap();
        assert_eq!(r.scene.triangle_count(), 1);
        assert_eq!(r.scene.drawable_count(), 2, "the triangle and the segment");
        let topologies: Vec<Topology> = r.scene.meshes[0].primitives.iter().map(|p| p.topology).collect();
        assert_eq!(topologies, [Topology::Triangles, Topology::Lines]);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let holes = scene_doc(
            &geom("g", &TRI, r##"<polygons count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><ph><p>0 1 2</p><h>0 1 2</h></ph></polygons>"##),
            r##"<node id="n"><instance_geometry url="#g"/></node>"##,
        );
        assert!(matches!(read_doc(&holes), Err(MeshError::Unsupported(_))));
    }

    // ── points and lines (mesh-gaps item 10, Task 4) ────────────────────────

    /// Five points on a zigzag, for the line tests.
    const ZIG: [f32; 15] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0, 2.0, 0.0, 0.0];

    fn zig(i: usize) -> [f32; 3] {
        [ZIG[3 * i], ZIG[3 * i + 1], ZIG[3 * i + 2]]
    }

    const VERT_IN: &str = r##"<input semantic="VERTEX" source="#g-v" offset="0"/>"##;

    /// Two normals, `nrm`, for a primitive to take at another offset.
    const NRM_SRC: &str = r##"<source id="nrm"><float_array id="na" count="6">0 0 1 0 1 0</float_array><technique_common><accessor source="#na" count="2" stride="3"><param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/></accessor></technique_common></source>"##;

    /// A node drawing geometry `g` of [`ZIG`]'s points, [`NRM_SRC`] and these
    /// primitives.
    fn lines_doc(prims: &str) -> String {
        let g = geom("g", &ZIG, prims).replacen("<vertices", &format!("{NRM_SRC}<vertices"), 1);
        scene_doc(&g, r##"<node id="n"><instance_geometry url="#g"/></node>"##)
    }

    /// Each segment of a lines primitive as its two end positions.
    fn segment_ends(p: &Primitive) -> Vec<[[f32; 3]; 2]> {
        assert_eq!(p.topology, Topology::Lines);
        p.indices.chunks_exact(2).map(|c| [p.positions[c[0] as usize], p.positions[c[1] as usize]]).collect()
    }

    /// `<lines>` is a pair of corners a segment, and `<linestrips>` a `<p>` a
    /// strip, of one fewer segments than its corners (COLLADA 1.4.1 spec,
    /// pp. 5-72, 5-74, 5-80).
    #[test]
    fn dae_lines_and_linestrips_read() {
        let prims = format!(
            r#"<lines count="2">{VERT_IN}<p>0 1 2 3</p></lines><linestrips count="2">{VERT_IN}<p>0 1 2 3</p><p>3 4</p></linestrips>"#
        );
        let r = read_doc(&lines_doc(&prims)).unwrap();
        let ps = &r.scene.meshes[0].primitives;
        assert_eq!(ps.len(), 2, "one primitive for each element");
        assert!(ps.iter().all(|p| p.topology == Topology::Lines), "{:?}", ps.iter().map(|p| p.topology).collect::<Vec<_>>());
        assert_eq!(segment_ends(&ps[0]), [[zig(0), zig(1)], [zig(2), zig(3)]], "<lines>: two corners a segment");
        assert_eq!(ps[0].positions.len(), 4, "the vertices its segments use: point 4 is not among them");
        assert_eq!(
            segment_ends(&ps[1]),
            [[zig(0), zig(1)], [zig(1), zig(2)], [zig(2), zig(3)], [zig(3), zig(4)]],
            "<linestrips>: a strip of c corners has c - 1 segments, and none between strips"
        );
        assert_eq!(ps[1].indices, [0, 1, 1, 2, 2, 3, 3, 4], "the corners a segment shares are one vertex");
        assert_eq!((ps[1].positions.len(), r.scene.triangle_count(), r.scene.drawable_count()), (5, 0, 2 + 4));
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    /// A line's other inputs are a triangle's: a normal at its own offset
    /// splits a vertex where two segments meet with different normals.
    #[test]
    fn lines_take_the_inputs_triangles_take() {
        let prims = format!(
            r##"<lines count="2">{VERT_IN}<input semantic="NORMAL" source="#nrm" offset="1"/><p>0 0 1 0 1 1 2 1</p></lines>"##
        );
        let s = read_doc(&lines_doc(&prims)).unwrap().scene;
        let p = &s.meshes[0].primitives[0];
        assert_eq!(p.positions, [zig(0), zig(1), zig(1), zig(2)], "point 1 once for each normal it is used with");
        assert_eq!(p.normals, Some(vec![[0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0]]));
        assert_eq!(p.indices, [0, 1, 2, 3]);
    }

    /// The material symbol of a line binds as a triangle's does, and one that
    /// binds nothing is said without calling the primitive triangles.
    #[test]
    fn a_line_primitive_binds_its_material_symbol() {
        let body = material_doc(r#"<phong><diffuse><color>1 0 0 1</color></diffuse></phong>"#, "", "", "")
            .replace(r#"<triangles count="1" material="sym">"#, r#"<lines count="1" material="sym">"#)
            .replace("<p>0 1 2</p></triangles>", "<p>0 1</p></lines>");
        assert!(body.contains("<lines count=\"1\" material=\"sym\">") && body.contains("</lines>"), "{body}");
        let s = read_doc(&body).unwrap().scene;
        let p = &s.meshes[0].primitives[0];
        assert_eq!((p.topology, p.material), (Topology::Lines, Some(0)));
        let base = s.materials[0].base_color;
        assert!(base.iter().zip([1.0, 0.0, 0.0, 1.0]).all(|(a, b)| (a - b).abs() < 1e-6), "{base:?}");

        let r = read_doc(&lines_doc(&format!(r#"<lines count="1" material="nope">{VERT_IN}<p>0 1</p></lines>"#))).unwrap();
        assert_eq!(r.scene.meshes[0].primitives[0].material, None);
        let said: Vec<&String> = r.warnings.iter().filter(|w| w.contains("nope")).collect();
        assert_eq!(said.len(), 1, "{:?}", r.warnings);
        assert!(said[0].contains("no material") && !said[0].contains("triangles"), "{}", said[0]);
    }

    /// A `<lines>` whose count or corners do not agree is refused as a
    /// `<triangles>`' is; a strip of fewer than 2 corners is skipped, counted
    /// and said once for the element; a document left drawing nothing is empty.
    #[test]
    fn bad_line_elements_are_refused_or_skipped() {
        let with = |element: &str| lines_doc(element);
        for (what, element) in [
            ("a count the corners do not make", format!(r#"<lines count="3">{VERT_IN}<p>0 1 2 3</p></lines>"#)),
            ("an odd number of corners", format!(r#"<lines count="1">{VERT_IN}<p>0 1 2</p></lines>"#)),
            ("a corner past the points", format!(r#"<lines count="1">{VERT_IN}<p>0 9</p></lines>"#)),
            ("a negative index", format!(r#"<lines count="1">{VERT_IN}<p>0 -1</p></lines>"#)),
            ("a tuple cut short", format!(r##"<lines count="1">{VERT_IN}<input semantic="NORMAL" source="#nrm" offset="1"/><p>0 0 1</p></lines>"##)),
            ("strips fewer than it says", format!(r#"<linestrips count="2">{VERT_IN}<p>0 1 2</p></linestrips>"#)),
            ("a strip past the points", format!(r#"<linestrips count="1">{VERT_IN}<p>0 1 9</p></linestrips>"#)),
            ("a count of billions", format!(r#"<lines count="4000000000">{VERT_IN}<p>0 1</p></lines>"#)),
            ("billions of strips", format!(r#"<linestrips count="4000000000">{VERT_IN}<p>0 1</p></linestrips>"#)),
        ] {
            let r = read_doc(&with(&element));
            assert!(matches!(r, Err(MeshError::Malformed(_))), "{what}: {r:?}");
        }

        let strips = format!(r#"<linestrips count="3">{VERT_IN}<p>0 1 2</p><p>3</p><p></p></linestrips>"#);
        let r = read_doc(&with(&strips)).unwrap();
        assert_eq!(segment_ends(&r.scene.meshes[0].primitives[0]), [[zig(0), zig(1)], [zig(1), zig(2)]]);
        let said: Vec<&String> = r.warnings.iter().filter(|w| w.contains("linestrips")).collect();
        assert_eq!(said.len(), 1, "{:?}", r.warnings);
        assert!(said[0].contains("2 line strip(s) of fewer than 2 corners"), "{}", said[0]);

        let nothing = format!(r#"<linestrips count="1">{VERT_IN}<p>0</p></linestrips><lines count="0">{VERT_IN}</lines>"#);
        assert_eq!(read_doc(&with(&nothing)).unwrap_err(), MeshError::Empty, "no segment is nothing to draw");
    }

    /// A strip whose segments pass the scene's index limit is refused from its
    /// corner count, before any of them is made: 6,000,101 corners are
    /// 6,000,100 segments and 12,000,200 indices. The list ends in a token
    /// that is not a number, so it is refused as `Unsupported` only by the
    /// check that counts tokens (`line_corners`); parsing the list would say
    /// `Malformed`.
    #[test]
    fn a_linestrip_past_the_index_limit_is_refused_before_it_is_expanded() {
        let started = std::time::Instant::now();
        let corners = format!("{}x", "0 1 ".repeat(3_000_050));
        let r = read_doc(&lines_doc(&format!(r#"<linestrips count="1">{VERT_IN}<p>{corners}</p></linestrips>"#)));
        assert!(matches!(&r, Err(MeshError::Unsupported(m)) if m.contains("line segments")), "{:?}", r.map(|_| ()));
        eprintln!("linestrip past the index limit: {:?}", started.elapsed());
    }

    /// The scene `wire` has of its loop alone: a node drawing the animated,
    /// coloured four-segment loop and nothing else.
    fn lines_alone() -> Scene {
        let wire = fixtures::wire();
        let mut alone = Scene { name: Some("lines".into()), ..Default::default() };
        alone.meshes.push(wire.meshes[1].clone());
        alone.nodes.push(Node { name: Some("lines_node".into()), mesh: Some(0), ..Default::default() });
        alone.roots = vec![0];
        assert_eq!(alone.meshes[0].primitives[0].topology, Topology::Lines);
        alone
    }

    /// `wire` through `write_scene` (prepare, then write): the cube, and the
    /// loop as `<lines>`, element for element what the reader gives back; the
    /// grid of points is not written, and is named once.
    #[test]
    fn wire_round_trips_through_collada_with_its_points_reported() {
        let wire = fixtures::wire();
        for self_contained in [false, true] {
            let opts = WriteOptions { self_contained, ..Default::default() };
            let ctx = if self_contained { "single file" } else { "package" };
            let w = super::super::write_scene(&wire, MeshFormat::Dae, opts).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let named: Vec<&Loss> = w.losses.iter().filter(|l| matches!(l.kind, LossKind::Points | LossKind::Lines)).collect();
            assert_eq!(named.len(), 1, "{ctx}: Collada holds lines and has no points: {:?}", w.losses);
            assert!(named[0].kind == LossKind::Points && named[0].what == "1 point primitive(s)", "{ctx}: {:?}", named[0]);

            let file = text(&w.package);
            assert_eq!(file.matches("<lines ").count(), 1, "{ctx}: the loop is one <lines>");
            assert!(file.contains(r#"<lines count="4""#), "{ctx}: four segments");
            assert!(!file.contains("<linestrips") && !file.contains("<points"), "{ctx}: <lines> is what every reader draws");
            assert_eq!(file.matches("<triangles ").count(), 1, "{ctx}: the cube");

            assert_round_trip("wire", &wire, opts);
            let back = read_package(&w.package).scene;
            let of = |s: &Scene, t: Topology| -> Vec<Primitive> { s.meshes.iter().flat_map(|m| m.primitives.clone()).filter(|p| p.topology == t).collect() };
            let (got, want) = (of(&back, Topology::Lines), of(&wire, Topology::Lines));
            assert_eq!((got.len(), want.len()), (1, 1), "{ctx}");
            assert_eq!(got[0].indices, want[0].indices, "{ctx}: the loop's segments");
            assert_eq!(got[0].positions, want[0].positions, "{ctx}");
            assert_eq!(got[0].colors, want[0].colors, "{ctx}: and their colours");
            assert!(of(&back, Topology::Points).is_empty(), "{ctx}");
            assert_eq!(back.triangle_count(), wire.triangle_count(), "{ctx}");
        }
    }

    /// Collada's own gates are of what is drawn: lines alone write and read
    /// back, and points alone are refused by name (`write_scene` says what
    /// Collada holds and what the scene has instead).
    #[test]
    fn a_scene_of_lines_alone_writes_and_reads_back_as_collada() {
        let alone = lines_alone();
        for self_contained in [false, true] {
            let opts = WriteOptions { self_contained, ..Default::default() };
            let ctx = if self_contained { "single file" } else { "package" };
            let w = super::super::write_scene(&alone, MeshFormat::Dae, opts);
            assert!(w.is_ok(), "{ctx}: {:?}", w.as_ref().err());
            let w = w.unwrap();
            assert!(w.losses.is_empty(), "{ctx}: {:?}", w.losses);
            let back = super::super::read_scene(&w.package.files[0].bytes, MeshFormat::Dae, &w.package.resolver());
            assert!(back.is_ok(), "{ctx}: {:?}", back.as_ref().err());
            let back = back.unwrap().scene;
            assert_eq!((back.triangle_count(), back.drawable_count()), (0, 4), "{ctx}");
        }
        let doc_of_lines = super::super::read_scene(&doc(&lines_doc(&format!(r#"<lines count="1">{VERT_IN}<p>0 1</p></lines>"#))), MeshFormat::Dae, &NoSidecars);
        assert!(doc_of_lines.is_ok(), "{:?}", doc_of_lines.map(|_| ()));

        let wire = fixtures::wire();
        let mut dots = Scene { name: Some("points".into()), ..Default::default() };
        dots.meshes.push(wire.meshes[2].clone());
        dots.nodes.push(Node { name: Some("points_node".into()), mesh: Some(0), ..Default::default() });
        dots.roots = vec![0];
        let r = super::super::write_scene(&dots, MeshFormat::Dae, WriteOptions::default());
        assert_eq!(
            r.map(|w| w.losses),
            Err(MeshError::Unsupported("dae holds triangles and lines; this scene has 9 points and no triangles or lines".into()))
        );
    }

    /// Lines over vertices their segments share, each with a normal, a uv and
    /// a colour. With `loose`, a fifth vertex that no segment names.
    fn tangled_lines(loose: bool) -> Scene {
        let n = if loose { 5 } else { 4 };
        let v = |i: usize| i as f32;
        let line = Primitive {
            positions: (0..n).map(|i| [v(i), (i % 2) as f32, 0.0]).collect(),
            normals: Some((0..n).map(|i| [0.0, 0.0, 1.0 - v(i) / 8.0]).collect()),
            uvs: vec![(0..n).map(|i| [v(i) / 4.0, 0.25]).collect()],
            colors: Some((0..n).map(|i| [v(i) / 4.0, 0.5, 1.0 - v(i) / 4.0, 1.0]).collect()),
            indices: vec![0, 1, 1, 2, 2, 3, 3, 0],
            topology: Topology::Lines,
            ..Default::default()
        };
        let mut s = Scene { name: Some("tangled".into()), ..Default::default() };
        s.meshes = vec![SceneMesh { name: Some("tangled_lines".into()), primitives: vec![line], ..Default::default() }];
        s.nodes = vec![Node { name: Some("lines_node".into()), mesh: Some(0), ..Default::default() }];
        s.roots = vec![0];
        s
    }

    /// `prepare` is what the reader gives back (the harness draws
    /// `to.prepare(from.prepare(x))`): lines over shared vertices keep them
    /// shared, with their normals, uvs and colours, in a package and in a
    /// single file. Collada's `prepare` does not compact vertices, as for
    /// triangles: a vertex no segment names is in the prepared scene and not
    /// in what reads back.
    #[test]
    fn dae_prepare_is_what_lines_over_shared_vertices_read_back_as() {
        for self_contained in [false, true] {
            let opts = WriteOptions { self_contained, ..Default::default() };
            let ctx = if self_contained { "single file" } else { "package" };
            let scene = tangled_lines(false);
            let want = MeshFormat::Dae.prepare(&scene, opts).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let w = super::super::write_scene(&scene, MeshFormat::Dae, opts).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let back = super::super::read_scene(&w.package.files[0].bytes, MeshFormat::Dae, &w.package.resolver()).unwrap_or_else(|e| panic!("{ctx}: {e}")).scene;
            let (a, b) = (&want.meshes[0].primitives[0], &back.meshes[0].primitives[0]);
            assert_eq!((a.topology, b.topology), (Topology::Lines, Topology::Lines), "{ctx}");
            assert_eq!(a.positions.len(), 4, "{ctx}: the four shared vertices are not split");
            assert_eq!(b.positions, a.positions, "{ctx}: positions");
            assert_eq!(b.indices, a.indices, "{ctx}: indices");
            assert_eq!(b.normals, a.normals, "{ctx}: normals");
            assert_eq!(b.uvs, a.uvs, "{ctx}: uvs");
            assert_eq!(b.colors, a.colors, "{ctx}: colours");
            assert!(a.normals.is_some() && a.colors.is_some() && a.uvs.len() == 1, "{ctx}: there is something to compare");

            let loose = tangled_lines(true);
            let kept = MeshFormat::Dae.prepare(&loose, opts).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let w = super::super::write_scene(&loose, MeshFormat::Dae, opts).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let back = super::super::read_scene(&w.package.files[0].bytes, MeshFormat::Dae, &w.package.resolver()).unwrap_or_else(|e| panic!("{ctx}: {e}")).scene;
            let (a, b) = (&kept.meshes[0].primitives[0], &back.meshes[0].primitives[0]);
            assert_eq!((a.positions.len(), b.positions.len()), (5, 4), "{ctx}: the vertex nothing names is not read");
            assert_eq!(segment_ends(b), segment_ends(a), "{ctx}: the same segments");
        }
    }

    /// `prepare` strips what Collada cannot hold, so a writer that is handed
    /// a points primitive was not prepared. A debug build says so.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "dae was given a points primitive; prepare strips what it cannot hold")]
    fn the_writer_is_never_given_a_points_primitive() {
        let _ = write_scene(&fixtures::wire(), WriteOptions::default());
    }

    /// The same two element kinds through the reader and the writer's own
    /// output, each cut at every length, with each bit of its first 4 KiB and
    /// of the span from its first `<lines` flipped in turn, and 2-8 bits
    /// flipped at random in 10,000 seeded cases (`layer_sweep`). Every reading
    /// returns.
    #[test]
    fn dae_lines_survive_truncation_and_bit_flips() {
        let prims = format!(
            r#"<lines count="2">{VERT_IN}<p>0 1 2 3</p></lines><linestrips count="3">{VERT_IN}<p>0 1 2 3</p><p>3 4</p><p>1</p></linestrips>"#
        );
        let hand = doc(&lines_doc(&prims));
        let name = "lean <lines> and <linestrips> as Collada";
        let whole = super::super::read_scene(&hand, MeshFormat::Dae, &NoSidecars).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((whole.scene.drawable_count(), whole.scene.triangle_count()), (2 + 4, 0), "{name} itself carries them");
        layer_sweep(name, &hand, span(name, &hand, "<lines ", "</mesh>"), |bytes| {
            let _ = super::super::read_scene(bytes, MeshFormat::Dae, &NoSidecars);
        });

        let written = package(&lines_alone(), WriteOptions { self_contained: true, ..Default::default() });
        let name = "the writer's loop as Collada";
        let file = &written.files[0].bytes;
        let whole = super::super::read_scene(file, MeshFormat::Dae, &NoSidecars).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(whole.scene.drawable_count(), 4, "{name} itself carries it");
        layer_sweep(name, file, span(name, file, "<lines ", "</mesh>"), |bytes| {
            let _ = super::super::read_scene(bytes, MeshFormat::Dae, &NoSidecars);
        });
    }

    #[test]
    fn reads_separate_index_streams_and_welds_identical_corners() {
        // Two triangles sharing an edge, with per-face normals at their own
        // offset: the shared corners split, identical tuples weld.
        let pos = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0];
        let prims = r##"<triangles count="2"><input semantic="VERTEX" source="#g-v" offset="0"/><input semantic="NORMAL" source="#n" offset="1"/><p>0 0 1 0 2 0 0 1 2 1 3 1</p></triangles>"##;
        let normals = r##"<source id="n"><float_array id="na" count="6">0 0 1 0 0.6 0.8</float_array><technique_common><accessor source="#na" count="2" stride="3"><param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/></accessor></technique_common></source>"##;
        let g = geom("g", &pos, prims).replacen("<vertices", &format!("{normals}<vertices"), 1);
        let s = read_doc(&scene_doc(&g, r##"<node id="n0"><instance_geometry url="#g"/></node>"##)).unwrap().scene;
        let p = &s.meshes[0].primitives[0];
        assert_eq!(p.positions.len(), 6, "vertices 0 and 2 each appear with two normals");
        assert_eq!(p.indices.len(), 6);
        let n = p.normals.as_ref().unwrap();
        assert_eq!(n[p.indices[3] as usize], [0.0, 0.6, 0.8]);
    }

    #[test]
    fn honours_accessor_offset_stride_and_unnamed_params() {
        // Element i at 1 + 4i: X, an unnamed (skipped) value, then Y and Z.
        let src = r##"<source id="g-p"><float_array id="g-pa" count="13">99 0 7 0 0 1 7 0 0 0 7 1 0</float_array><technique_common><accessor source="#g-pa" count="3" stride="4" offset="1"><param name="X" type="float"/><param type="float"/><param name="Y" type="float"/><param name="Z" type="float"/></accessor></technique_common></source>"##;
        let g = format!(
            r##"<geometry id="g"><mesh>{src}<vertices id="g-v"><input semantic="POSITION" source="#g-p"/></vertices><triangles count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><p>0 1 2</p></triangles></mesh></geometry>"##
        );
        let s = read_doc(&scene_doc(&g, r##"<node id="n"><instance_geometry url="#g"/></node>"##)).unwrap().scene;
        assert_eq!(s.meshes[0].primitives[0].positions, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    }

    #[test]
    fn texture_coordinates_flip_to_v_down_and_follow_their_set_numbers() {
        // One set, numbered 1 (SceneKit's spelling), plus a second numbered 3.
        let uv = |id: &str, v: &str| format!(r##"<source id="{id}"><float_array id="{id}a" count="6">{v}</float_array><technique_common><accessor source="#{id}a" count="3" stride="2"><param name="S" type="float"/><param name="T" type="float"/></accessor></technique_common></source>"##);
        let prims = r##"<triangles count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><input semantic="TEXCOORD" source="#t3" offset="0" set="3"/><input semantic="TEXCOORD" source="#t1" offset="0" set="1"/><p>0 1 2</p></triangles>"##;
        let g = geom("g", &TRI, prims).replacen("<vertices", &format!("{}{}<vertices", uv("t1", "0 0 1 0 0 1"), uv("t3", "0.25 0.25 0.5 0.5 0.75 0.75")), 1);
        let s = read_doc(&scene_doc(&g, r##"<node id="n"><instance_geometry url="#g"/></node>"##)).unwrap().scene;
        let p = &s.meshes[0].primitives[0];
        assert_eq!(p.uvs.len(), 2);
        assert_eq!(p.uvs[0], vec![[0.0, 1.0], [1.0, 1.0], [0.0, 0.0]], "set 1 first, V flipped");
        assert_eq!(p.uvs[1][1], [0.5, 0.5]);
    }

    // ── reader: scene graph ─────────────────────────────────────────────────

    #[test]
    fn converts_units_and_up_axes_at_the_root() {
        let cases = [
            ("Z_UP", "0.01", [0.01f32, 0.03, -0.02]),
            ("X_UP", "1", [-2.0, 1.0, 3.0]),
            ("Y_UP", "2", [2.0, 4.0, 6.0]),
        ];
        for (up, meter, want) in cases {
            let body = format!(
                r#"<asset><unit meter="{meter}"/><up_axis>{up}</up_axis></asset>{}"#,
                scene_doc(&geom("g", &[1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0], r##"<triangles count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><p>0 1 2</p></triangles>"##), r##"<node id="n"><instance_geometry url="#g"/></node>"##)
            );
            // A second <asset> would be invalid; build the document by hand.
            let bytes = format!(r#"<?xml version="1.0"?><COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">{body}</COLLADA>"#);
            let s = read_scene(bytes.as_bytes(), &NoSidecars).unwrap().scene;
            let p = world_positions(&s)[0];
            assert!(close3(p, want, 1e-6), "{up} {meter}: {p:?}");
        }
    }

    #[test]
    fn composes_transform_elements_in_document_order() {
        // (1,0,0) scaled by 2, turned 90° about Z, then moved by +x.
        let s = read_doc(&tri_doc(r#"<translate>1 0 0</translate><rotate>0 0 1 90</rotate><scale>2 2 2</scale>"#)).unwrap().scene;
        assert!(close3(world_positions(&s)[1], [1.0, 2.0, 0.0], 1e-6), "{:?}", world_positions(&s));
        // A row-major <matrix> followed by a translate.
        let s = read_doc(&tri_doc(r#"<matrix>1 0 0 5 0 1 0 0 0 0 1 0 0 0 0 1</matrix><translate>0 1 0</translate>"#)).unwrap().scene;
        assert!(close3(world_positions(&s)[0], [5.0, 1.0, 0.0], 1e-6));
        // <lookat>: from (0,0,5) toward the origin, up +Y: identity rotation.
        let s = read_doc(&tri_doc(r#"<lookat>0 0 5 0 0 0 0 1 0</lookat>"#)).unwrap().scene;
        assert!(close3(world_positions(&s)[1], [1.0, 0.0, 5.0], 1e-6));
        assert!(matches!(read_doc(&tri_doc(r#"<skew>45 1 0 0 0 1 0</skew>"#)), Err(MeshError::Unsupported(_))));
        assert!(matches!(read_doc(&tri_doc(r#"<rotate>0 0 0 30</rotate>"#)), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn instance_node_copies_library_nodes_and_keeps_meshes_shared() {
        let g = geom("g", &TRI, r##"<triangles count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><p>0 1 2</p></triangles>"##);
        let body = format!(
            r##"<library_geometries>{g}</library_geometries><library_nodes><node id="part"><instance_geometry url="#g"/></node></library_nodes><library_visual_scenes><visual_scene id="vs"><node id="a"><translate>5 0 0</translate><instance_node url="#part"/></node><node id="b"><instance_node url="#part"/></node></visual_scene></library_visual_scenes><scene><instance_visual_scene url="#vs"/></scene>"##
        );
        let s = read_doc(&body).unwrap().scene;
        assert_eq!(s.nodes.len(), 4);
        assert_eq!(s.meshes.len(), 1, "one geometry, instanced twice");
        let p = world_positions(&s);
        assert!(close3(p[1], [6.0, 0.0, 0.0], 1e-6) && close3(p[4], [1.0, 0.0, 0.0], 1e-6), "{p:?}");
    }

    #[test]
    fn an_instance_node_cycle_is_refused() {
        let body = r##"<library_nodes><node id="a"><instance_node url="#b"/></node><node id="b"><instance_node url="#a"/></node></library_nodes><library_visual_scenes><visual_scene id="vs"><node id="root"><instance_node url="#a"/></node></visual_scene></library_visual_scenes><scene><instance_visual_scene url="#vs"/></scene>"##;
        assert!(matches!(read_doc(body), Err(MeshError::Malformed(_))));
        let selfref = r##"<library_visual_scenes><visual_scene id="vs"><node id="a"><instance_node url="#a"/></node></visual_scene></library_visual_scenes><scene><instance_visual_scene url="#vs"/></scene>"##;
        assert!(matches!(read_doc(selfref), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn an_instance_node_bomb_is_refused_quickly() {
        // Each level instances the next twice: 2^40 nodes from a 3 KB file.
        let mut lib = String::new();
        for i in 0..40 {
            lib.push_str(&format!(r##"<node id="n{i}"><instance_node url="#n{}"/><instance_node url="#n{}"/></node>"##, i + 1, i + 1));
        }
        lib.push_str(r#"<node id="n40"/>"#);
        let body = format!(r##"<library_nodes>{lib}</library_nodes><library_visual_scenes><visual_scene id="vs"><node id="r"><instance_node url="#n0"/></node></visual_scene></library_visual_scenes><scene><instance_visual_scene url="#vs"/></scene>"##);
        let started = std::time::Instant::now();
        assert!(matches!(read_doc(&body), Err(MeshError::Unsupported(_))));
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let open: String = (0..600).map(|i| format!(r#"<node id="d{i}">"#)).collect();
        let close = "</node>".repeat(600);
        let body = format!(r##"<library_visual_scenes><visual_scene id="vs">{open}{close}</visual_scene></library_visual_scenes><scene><instance_visual_scene url="#vs"/></scene>"##);
        assert!(read_doc(&body).is_err());
    }

    // ── reader: materials and images ────────────────────────────────────────

    /// 2026-10-12 review, finding 26.4: an id is the first step of a channel's
    /// target address, where `.` selects a member (COLLADA 1.4.1, "Address
    /// Syntax": `node/sid.X`), so an id holding one misreads — assimp drops the
    /// channel. Ids map `.` to `_`; names keep it.
    #[test]
    fn ids_never_hold_the_address_member_separator() {
        let mut s = fixtures::skinned();
        for (i, n) in s.nodes.iter_mut().enumerate() {
            n.name = Some(format!("upper_arm.L.{i:03}"));
        }
        let pkg = package(&s, WriteOptions::default());
        let root = super::super::xml::parse(&pkg.files[0].bytes).unwrap();
        let channels = root.descendants_named("channel");
        assert!(!channels.is_empty());
        for c in channels {
            let target = c.attr("target").unwrap();
            assert!(!target.split('/').next().unwrap().contains('.'), "{target}");
        }
        let back = read_scene(&pkg.files[0].bytes, &NoSidecars).unwrap().scene;
        assert!(back.nodes.iter().any(|n| n.name.as_deref() == Some("upper_arm.L.001")), "names keep their dots");
    }

    /// 2026-10-12 review, finding 26.5: `<newparam>` may sit on the effect, its
    /// profile or the profile's technique (the 1.4.1 schema's
    /// profile_COMMON/technique), and the innermost declaration of a sid wins.
    #[test]
    fn technique_newparams_are_found_and_shadow_outer_ones() {
        let effect = r#"<newparam sid="tint"><float4>0 1 0 1</float4></newparam><lambert><diffuse><param ref="tint"/></diffuse></lambert>"#;
        let r = read_doc(&material_doc(effect, "", "", "")).unwrap();
        assert_eq!(r.scene.materials[0].base_color, [0.0, 1.0, 0.0, 1.0], "{:?}", r.warnings);
        let outer = r#"<newparam sid="tint"><float4>1 0 0 1</float4></newparam>"#;
        let r = read_doc(&material_doc(effect, outer, "", "")).unwrap();
        assert_eq!(r.scene.materials[0].base_color, [0.0, 1.0, 0.0, 1.0], "the technique's own declaration wins");
    }

    /// 2026-10-12 review, finding 26.6: a `<constant>` material's colour is its
    /// emission (1.4.1 §8), a texture as much as a colour: it is the unlit
    /// material's base colour map.
    #[test]
    fn a_constant_material_s_emission_texture_is_its_colour() {
        let doc = material_doc(
            r#"<constant><emission><texture texture="samp" texcoord="CH0"/></emission></constant>"#,
            SAMPLER,
            r#"<bind_vertex_input semantic="CH0" input_semantic="TEXCOORD" input_set="0"/>"#,
            UV0,
        );
        let r = with_png(&doc).unwrap();
        let m = &r.scene.materials[0];
        assert!(m.unlit && m.base_color_texture.is_some(), "{m:?}");
        assert_eq!(m.base_color, [1.0; 4]);
        assert!(m.emissive_texture.is_none() && m.emissive == [0.0; 3], "{m:?}");
    }

    /// 2026-10-12 review, finding 26.7: Collada names lose U+FFFE/U+FFFF too.
    #[test]
    fn noncharacters_never_reach_a_collada_document() {
        let mut s = fixtures::cube();
        let root = s.roots[0];
        s.nodes[root].name = Some("box\u{FFFF}".into());
        s.materials[0].name = Some("red\u{FFFF}".into());
        let w = super::super::write_scene(&s, MeshFormat::Dae, WriteOptions::default()).unwrap();
        let text = String::from_utf8(w.package.files[0].bytes.clone()).unwrap();
        assert!(!text.contains('\u{FFFF}'), "{text}");
    }

    /// 2026-10-12 review, finding 26.1: a comment inside a colour no longer
    /// fuses the numbers beside it (`1 0 <!-- tint --> 0 1` read magenta).
    #[test]
    fn a_comment_inside_a_value_does_not_change_it() {
        let r = read_doc(&material_doc(r#"<lambert><diffuse><color>1 0 <!-- tint --> 0 1</color></diffuse></lambert>"#, "", "", "")).unwrap();
        assert_eq!(r.scene.materials[0].base_color, [1.0, 0.0, 0.0, 1.0]);
    }

    fn material_doc(effect: &str, newparams: &str, bind: &str, prim_inputs: &str) -> String {
        let g = geom(
            "g",
            &TRI,
            &format!(r##"<triangles count="1" material="sym"><input semantic="VERTEX" source="#g-v" offset="0"/>{prim_inputs}<p>0 1 2</p></triangles>"##),
        )
        .replacen(
            "<vertices",
            r##"<source id="uv"><float_array id="uva" count="6">0 0 1 0 0 1</float_array><technique_common><accessor source="#uva" count="3" stride="2"><param name="S" type="float"/><param name="T" type="float"/></accessor></technique_common></source><vertices"##,
            1,
        );
        format!(
            r##"<library_images><image id="img"><init_from>tex/a.png</init_from></image></library_images><library_effects><effect id="fx"><profile_COMMON>{newparams}<technique sid="common">{effect}</technique></profile_COMMON></effect></library_effects><library_materials><material id="m" name="mat"><instance_effect url="#fx"/></material></library_materials>{}"##,
            scene_doc(&g, &format!(r##"<node id="n"><instance_geometry url="#g"><bind_material><technique_common><instance_material symbol="sym" target="#m">{bind}</instance_material></technique_common></bind_material></instance_geometry></node>"##))
        )
    }

    const SAMPLER: &str = r#"<newparam sid="surf"><surface type="2D"><init_from>img</init_from></surface></newparam><newparam sid="samp"><sampler2D><source>surf</source><wrap_s>CLAMP</wrap_s><wrap_t>MIRROR</wrap_t></sampler2D></newparam>"#;
    const UV0: &str = r##"<input semantic="TEXCOORD" source="#uv" offset="0" set="0"/>"##;

    fn with_png(body: &str) -> Result<SceneRead, MeshError> {
        let files = vec![PackageFile { path: "tex/a.png".into(), bytes: fixtures::quadrant_png() }];
        read_scene(&doc(body), &MemorySidecars { files: &files })
    }

    #[test]
    fn resolves_textures_through_the_sampler_and_surface() {
        let body = material_doc(
            r#"<phong><diffuse><texture texture="samp" texcoord="CH0"/></diffuse><shininess><float>30</float></shininess></phong>"#,
            SAMPLER,
            r#"<bind_vertex_input semantic="CH0" input_semantic="TEXCOORD" input_set="0"/>"#,
            UV0,
        );
        let s = with_png(&body).unwrap().scene;
        let m = &s.materials[0];
        assert_eq!(m.name.as_deref(), Some("mat"));
        let r = m.base_color_texture.as_ref().unwrap();
        assert_eq!(s.images[s.textures[r.texture].image].data, fixtures::quadrant_png());
        assert_eq!(s.textures[r.texture].sampler.wrap_s, Wrap::ClampToEdge);
        assert_eq!(s.textures[r.texture].sampler.wrap_t, Wrap::MirroredRepeat);
        assert!((m.roughness - roughness_from_shininess(30.0)).abs() < 1e-6);
        // Without the file, the texture is dropped and that is said.
        let r = read_doc(&body).unwrap();
        assert!(r.scene.materials[0].base_color_texture.is_none());
        assert!(r.warnings.iter().any(|w| w.contains("not supplied")), "{:?}", r.warnings);
    }

    /// 2026-10-12 review, finding 7: `<init_from>` holding CJK text panicked
    /// in the five-byte `data:` test ("テクスチャ.png": byte 5 is inside "ク").
    #[test]
    fn an_image_named_in_cjk_resolves_and_never_panics() {
        let body = material_doc(r#"<lambert><diffuse><texture texture="samp" texcoord="x"/></diffuse></lambert>"#, SAMPLER, "", UV0)
            .replace("<init_from>tex/a.png</init_from>", "<init_from>テクスチャ.png</init_from>");
        let files = vec![PackageFile { path: "テクスチャ.png".into(), bytes: fixtures::quadrant_png() }];
        let r = read_scene(&doc(&body), &MemorySidecars { files: &files }).unwrap();
        assert!(r.scene.materials[0].base_color_texture.is_some(), "{:?}", r.warnings);
        let r = read_doc(&body).unwrap();
        assert!(r.scene.materials[0].base_color_texture.is_none());
        assert!(r.warnings.iter().any(|w| w.contains("not supplied")), "{:?}", r.warnings);
    }

    #[test]
    fn a_texture_naming_an_image_directly_is_used_with_a_warning() {
        let body = material_doc(r#"<lambert><diffuse><texture texture="img" texcoord="x"/></diffuse></lambert>"#, "", "", UV0);
        let r = with_png(&body).unwrap();
        assert!(r.scene.materials[0].base_color_texture.is_some());
        assert!(r.warnings.iter().any(|w| w.contains("directly")), "{:?}", r.warnings);
    }

    #[test]
    fn absolute_image_paths_resolve_by_file_name_and_hex_images_decode() {
        let body = material_doc(r#"<lambert><diffuse><texture texture="samp" texcoord="x"/></diffuse></lambert>"#, SAMPLER, "", UV0)
            .replace("<init_from>tex/a.png</init_from>", "<init_from>file:///C:/work/a.png</init_from>");
        let files = vec![PackageFile { path: "a.png".into(), bytes: fixtures::quadrant_png() }];
        let r = read_scene(&doc(&body), &MemorySidecars { files: &files }).unwrap();
        assert!(r.scene.materials[0].base_color_texture.is_some());
        assert!(r.warnings.iter().any(|w| w.contains("beside the document")), "{:?}", r.warnings);
        let hex: String = fixtures::quadrant_png().iter().map(|b| format!("{b:02X}")).collect();
        let body = material_doc(r#"<lambert><diffuse><texture texture="samp" texcoord="x"/></diffuse></lambert>"#, SAMPLER, "", UV0)
            .replace("<init_from>tex/a.png</init_from>", &format!("<data>{hex}</data>"));
        let s = read_doc(&body).unwrap().scene;
        assert_eq!(s.images[0].data, fixtures::quadrant_png());
    }

    #[test]
    fn reads_colours_as_srgb_and_every_transparency_mode() {
        let c = srgb_encode(0.25);
        let cases: Vec<(String, AlphaMode, f32)> = vec![
            (format!(r#"<lambert><diffuse><color>{c} {c} {c} 1</color></diffuse></lambert>"#), AlphaMode::Opaque, 1.0),
            (r#"<lambert><transparent opaque="A_ONE"><color>1 1 1 0.4</color></transparent><transparency><float>1</float></transparency></lambert>"#.into(), AlphaMode::Blend, 0.4),
            (r#"<lambert><transparent opaque="RGB_ZERO"><color>0.25 0.25 0.25 1</color></transparent><transparency><float>1</float></transparency></lambert>"#.into(), AlphaMode::Blend, 0.75),
            (r#"<lambert><transparency><float>0.5</float></transparency></lambert>"#.into(), AlphaMode::Blend, 0.5),
            (r#"<lambert><transparent opaque="A_ONE"><color>1 1 1 1</color></transparent><transparency><float>1</float></transparency></lambert>"#.into(), AlphaMode::Opaque, 1.0),
        ];
        for (effect, mode, alpha) in cases {
            let s = read_doc(&material_doc(&effect, "", "", "")).unwrap().scene;
            let m = &s.materials[0];
            assert_eq!(m.alpha_mode, mode, "{effect}");
            assert!((m.base_color[3] - alpha).abs() < 1e-6, "{effect}: {:?}", m.base_color);
        }
        let s = read_doc(&material_doc(&format!(r#"<lambert><diffuse><color>{c} {c} {c} 1</color></diffuse></lambert>"#), "", "", "")).unwrap().scene;
        assert!((s.materials[0].base_color[0] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn reads_constant_shading_param_refs_and_double_sided_extras() {
        let s = read_doc(&material_doc(r#"<constant><emission><color>1 0 0 1</color></emission></constant>"#, "", "", "")).unwrap().scene;
        assert!(s.materials[0].unlit && s.materials[0].base_color[..3] == [1.0, 0.0, 0.0]);
        let np = r#"<newparam sid="tint"><float4>0 1 0 1</float4></newparam>"#;
        let fx = r#"<lambert><diffuse><param ref="tint"/></diffuse></lambert><extra><technique profile="MAX3D"><double_sided>1</double_sided></technique></extra>"#;
        let s = read_doc(&material_doc(fx, np, "", "")).unwrap().scene;
        assert_eq!(s.materials[0].base_color[..3], [0.0, 1.0, 0.0]);
        assert!(s.materials[0].double_sided);
    }

    // ── material layers: index_of_refraction ────────────────────────────────

    /// One triangle per material, side by side, each its own root.
    fn one_triangle_each(materials: Vec<Material>) -> Scene {
        let mut s = Scene::default();
        for (i, m) in materials.into_iter().enumerate() {
            s.materials.push(m);
            let x = 2.0 * i as f32;
            s.meshes.push(SceneMesh {
                primitives: vec![Primitive {
                    positions: vec![[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x, 1.0, 0.0]],
                    indices: vec![0, 1, 2],
                    material: Some(i),
                    ..Default::default()
                }],
                ..Default::default()
            });
            s.add_root(Node { mesh: Some(i), ..Default::default() });
        }
        s
    }

    /// Each material's name and ior, sorted by name.
    fn iors(s: &Scene) -> Vec<(String, Option<f32>)> {
        let mut out: Vec<(String, Option<f32>)> = s.materials.iter().map(|m| (m.name.clone().unwrap_or_default(), m.ior)).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// `<index_of_refraction>` is glTF's ior (KHR_materials_ior), read back
    /// exactly, from a package and from one file alike. It is written last in
    /// its shader, where the 1.4.1 schema puts it in both a lambert and a
    /// phong, after `<transparency>`. KHR's special 0 is no refractive index,
    /// and is left out (`an_ior_of_0_is_left_out_of_collada_and_named`).
    #[test]
    fn index_of_refraction_round_trips() {
        let values = [Some(1.2), Some(1.33), Some(1.52), Some(2.42), None];
        let materials: Vec<Material> = values
            .iter()
            .enumerate()
            .map(|(i, &ior)| Material {
                name: Some(format!("m{i}")),
                metallic: 0.0,
                // Lambert and phong in turn; the second blends.
                roughness: if i % 2 == 0 { 1.0 } else { 0.4 },
                base_color: [0.5, 0.5, 0.5, if i == 1 { 0.5 } else { 1.0 }],
                alpha_mode: if i == 1 { AlphaMode::Blend } else { AlphaMode::Opaque },
                ior,
                ..Default::default()
            })
            .collect();
        let scene = one_triangle_each(materials);
        let want: Vec<(String, Option<f32>)> = values.iter().enumerate().map(|(i, &ior)| (format!("m{i}"), ior)).collect();
        for self_contained in [false, true] {
            let opts = WriteOptions { self_contained, ..Default::default() };
            let w = super::super::write_scene(&scene, MeshFormat::Dae, opts).unwrap();
            assert!(!w.losses.iter().any(|l| l.what.contains("ior")), "{self_contained}: {:?}", w.losses);
            let back = read_scene(&w.package.files[0].bytes, &w.package.resolver()).unwrap();
            assert_eq!(iors(&back.scene), want, "single file: {self_contained}");
            assert!(back.warnings.is_empty(), "{:?}", back.warnings);
            let t = text(&w.package);
            assert_eq!(t.matches("<index_of_refraction>").count(), 4, "{t}");
            for (at, _) in t.match_indices("</index_of_refraction>") {
                let next = t[at + "</index_of_refraction>".len()..].trim_start();
                assert!(next.starts_with("</lambert>") || next.starts_with("</phong>"), "not last in its shader:\n{t}");
            }
            assert!(t.contains("<transparency>"), "{t}");
        }
    }

    /// The `<index_of_refraction>` values exporters write on every material
    /// by default read as unset (Ruling A22a; the sources are in the module
    /// docs): SceneKit's 1, Blender 2.8-3.6's 1.45 and 4.x's 1.5. Written,
    /// an ior of 1 or 1.45 is left out and named, since this reader would
    /// drop it; 1.5, glTF's own default, is left out unnamed: it draws the
    /// same.
    #[test]
    fn exporter_default_iors_read_as_unset() {
        for (v, want) in [("1", None), ("1.000000", None), ("1.45", None), ("1.5", None), ("1.6", Some(1.6))] {
            let r = read_doc(&material_doc(&format!("<lambert><index_of_refraction><float>{v}</float></index_of_refraction></lambert>"), "", "", "")).unwrap();
            assert_eq!(r.scene.materials[0].ior, want, "{v}");
            assert!(r.warnings.is_empty(), "{v}: {:?}", r.warnings);
        }
        let r = testdata(include_bytes!("testdata/dae/scenekit/textured_scenekit.dae"));
        assert!(r.scene.materials.iter().all(|m| m.ior.is_none()), "SceneKit writes 1 on every material");
        let scene = one_triangle_each(
            [("scenekit", 1.0), ("blender3", 1.45), ("blender4", 1.5), ("other", 1.6)]
                .iter()
                .map(|&(name, ior)| Material { name: Some(name.into()), metallic: 0.0, ior: Some(ior), ..Default::default() })
                .collect(),
        );
        let want = [("blender3".to_string(), None), ("blender4".into(), None), ("other".into(), Some(1.6)), ("scenekit".into(), None)];
        for self_contained in [false, true] {
            let opts = WriteOptions { self_contained, ..Default::default() };
            assert_eq!(iors(&prepare(&scene, opts).unwrap()), want, "prepared, single file: {self_contained}");
            let w = super::super::write_scene(&scene, MeshFormat::Dae, opts).unwrap();
            let mut named: Vec<&str> = w.losses.iter().filter(|l| l.what.contains("ior")).map(|l| l.what.as_str()).collect();
            named.sort_unstable();
            assert_eq!(named, ["an ior of 1 on 1 material(s)", "an ior of 1.45 on 1 material(s)"], "single file: {self_contained}");
            assert!(w.losses.iter().filter(|l| l.what.contains("ior")).all(|l| l.kind == LossKind::Materials));
            let back = read_scene(&w.package.files[0].bytes, &w.package.resolver()).unwrap().scene;
            assert_eq!(iors(&back), want, "read back, single file: {self_contained}");
        }
    }

    /// KHR_materials_ior's 0, a perfect mirror, has no Collada spelling: an
    /// `<index_of_refraction>` of 0 reads as no ior
    /// (`a_bad_index_of_refraction_is_left_out_with_a_warning`). So the
    /// writer leaves it out and names it, "an ior of 0", as OBJ does
    /// (Rulings A42, A21c), and the prepared scene is what reads back.
    #[test]
    fn an_ior_of_0_is_left_out_of_collada_and_named() {
        let scene = one_triangle_each(vec![Material { name: Some("mirror".into()), metallic: 0.0, ior: Some(0.0), ..Default::default() }]);
        let unset = vec![("mirror".to_string(), None)];
        for self_contained in [false, true] {
            let opts = WriteOptions { self_contained, ..Default::default() };
            assert_eq!(iors(&prepare(&scene, opts).unwrap()), unset, "prepared, single file: {self_contained}");
            let w = super::super::write_scene(&scene, MeshFormat::Dae, opts).unwrap();
            let named: Vec<(LossKind, &str)> = w.losses.iter().filter(|l| l.what.contains("ior")).map(|l| (l.kind, l.what.as_str())).collect();
            assert_eq!(named, [(LossKind::Materials, "an ior of 0 on 1 material(s)")], "single file: {self_contained}");
            assert!(!text(&w.package).contains("<index_of_refraction>"), "single file: {self_contained}");
            let back = read_scene(&w.package.files[0].bytes, &w.package.resolver()).unwrap();
            assert_eq!((iors(&back.scene), back.warnings.len()), (unset.clone(), 0), "read back, single file: {self_contained}");
        }
    }

    /// An `<index_of_refraction>` glTF's ior can't hold is left out with a
    /// warning naming it, and the file still converts (Rulings A22a, A16i):
    /// one between 0 and 1 (KHR_materials_ior allows 0, or 1 and above), or
    /// one that is no number. One of 0 or below is no refractive index, which
    /// is what profile_COMMON's element holds, and is left out too (Ruling
    /// A42), worded as OBJ's `Ni 0`: KHR's 0 is a perfect mirror, which no
    /// Collada file means by it. A value behind a `<param>` reads as a
    /// written one does.
    #[test]
    fn a_bad_index_of_refraction_is_left_out_with_a_warning() {
        for (v, why) in [
            ("0.5", "which KHR_materials_ior does not allow"),
            ("0", "0 or below"),
            ("-0", "0 or below"),
            ("-1", "0 or below"),
            ("nan", "not a number"),
            ("inf", "not a number"),
            ("abc", "not a number"),
        ] {
            let r = read_doc(&material_doc(&format!("<lambert><index_of_refraction><float>{v}</float></index_of_refraction></lambert>"), "", "", "")).unwrap();
            assert_eq!(r.scene.materials[0].ior, None, "{v}");
            assert!(
                r.warnings.len() == 1 && r.warnings[0].contains("<index_of_refraction>") && r.warnings[0].contains(v) && r.warnings[0].contains(why),
                "{v}: {:?}",
                r.warnings
            );
        }
        let np = r#"<newparam sid="n"><float>1.33</float></newparam>"#;
        let r = read_doc(&material_doc(r#"<phong><index_of_refraction><param ref="n"/></index_of_refraction></phong>"#, np, "", "")).unwrap();
        assert_eq!((r.scene.materials[0].ior, r.warnings.len()), (Some(1.33), 0), "{:?}", r.warnings);
        // A <param> naming nothing, or no float, says so too.
        let not_float = r#"<newparam sid="n"><float2>1 2</float2></newparam>"#;
        for (newparams, at) in [("", "missing"), (not_float, "n")] {
            let fx = format!(r#"<phong><index_of_refraction><param ref="{at}"/></index_of_refraction></phong>"#);
            let r = read_doc(&material_doc(&fx, newparams, "", "")).unwrap();
            assert_eq!(r.scene.materials[0].ior, None, "{at}");
            assert!(r.warnings.len() == 1 && r.warnings[0].contains("<index_of_refraction>") && r.warnings[0].contains("<param>"), "{at}: {:?}", r.warnings);
        }
    }

    // ── reader: controllers ─────────────────────────────────────────────────

    /// A two-vertex-weighted triangle skinned to joints `j0`/`j1`.
    fn skin_doc(bind_shape: &str, joints: &str, v: &str, vcount: &str, weights: &str) -> String {
        let g = geom("g", &TRI, r##"<triangles count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><p>0 1 2</p></triangles>"##);
        format!(
            r##"<library_controllers><controller id="c"><skin source="#g">{bind_shape}<source id="cj">{joints}<technique_common><accessor source="#cja" count="2" stride="1"><param name="JOINT" type="name"/></accessor></technique_common></source><source id="cm"><float_array id="cma" count="32">1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1 1 0 0 0 0 1 0 -1 0 0 1 0 0 0 0 1</float_array><technique_common><accessor source="#cma" count="2" stride="16"><param name="TRANSFORM" type="float4x4"/></accessor></technique_common></source><source id="cw"><float_array id="cwa" count="{}">{weights}</float_array><technique_common><accessor source="#cwa" count="{}" stride="1"><param name="WEIGHT" type="float"/></accessor></technique_common></source><joints><input semantic="JOINT" source="#cj"/><input semantic="INV_BIND_MATRIX" source="#cm"/></joints><vertex_weights count="3"><input semantic="JOINT" source="#cj" offset="0"/><input semantic="WEIGHT" source="#cw" offset="1"/><vcount>{vcount}</vcount><v>{v}</v></vertex_weights></skin></controller></library_controllers>{}"##,
            weights.split_whitespace().count(),
            weights.split_whitespace().count(),
            scene_doc(&g, r##"<node id="j0" name="j0" sid="J0" type="JOINT"><node id="j1" name="j1" sid="J1" type="JOINT"><translate>0 1 0</translate></node></node><node id="m"><translate>9 9 9</translate><instance_controller url="#c"><skeleton>#j0</skeleton></instance_controller></node>"##)
        )
    }

    #[test]
    fn skins_resolve_joints_by_sid_and_fold_the_bind_shape_matrix() {
        let body = skin_doc(
            "<bind_shape_matrix>1 0 0 0 0 1 0 2 0 0 1 0 0 0 0 1</bind_shape_matrix>",
            r#"<Name_array id="cja" count="2">J0 J1</Name_array>"#,
            "0 0 1 0 1 0",
            "1 1 1",
            "1",
        );
        let s = read_doc(&body).unwrap().scene;
        assert_eq!(s.skins.len(), 1);
        let sk = &s.skins[0];
        assert_eq!(sk.joints.iter().map(|j| s.nodes[*j].name.clone().unwrap()).collect::<Vec<_>>(), ["j0", "j1"]);
        // IBM(j1) = T(0,-1,0), folded with the bind shape T(0,2,0): T(0,1,0).
        assert_eq!(sk.inverse_bind_matrices[1], math::translation([0.0, 1.0, 0.0]));
        // Vertex 0 rides j0 (at the origin) with the bind shape: (0,2,0).
        // Vertex 1 rides j1: JM·IBM·BSM·v = T(0,1,0)·T(0,-1,0)·T(0,2,0)·(1,0,0).
        // The instancing node's own (9,9,9) is ignored, as the scene requires.
        let p = world_positions(&s);
        assert!(close3(p[0], [0.0, 2.0, 0.0], 1e-6) && close3(p[1], [1.0, 2.0, 0.0], 1e-6), "{p:?}");
    }

    #[test]
    fn skins_take_idref_joints_and_the_bind_shape_index() {
        let body = skin_doc("", r#"<IDREF_array id="cja" count="2">j0 j1</IDREF_array>"#, "0 0 -1 0 1 0", "1 1 1", "1")
            .replace(r#"<param name="JOINT" type="name"/>"#, r#"<param name="JOINT" type="IDREF"/>"#);
        let s = read_doc(&body).unwrap().scene;
        // -1 binds vertex 1 to the bind shape: the instancing node, which
        // becomes a third joint.
        assert_eq!(s.skins[0].joints.len(), 3);
        assert!(close3(world_positions(&s)[1], [10.0, 9.0, 9.0], 1e-6), "{:?}", world_positions(&s));
    }

    #[test]
    fn a_skin_of_a_skin_is_refused_without_recursing() {
        // A skin naming itself, and a 100k-long chain of skins: each must be
        // refused at the first link, not resolved (which recursed forever, or
        // as deep as the chain).
        let ctrl = |id: &str, src: &str| format!(r##"<controller id="{id}"><skin source="#{src}"><joints/><vertex_weights count="0"/></skin></controller>"##);
        let scene = |c: &str| scene_doc(&geom("g", &TRI, ""), &format!(r##"<node id="n"><instance_controller url="#{c}"/></node>"##));
        let body = format!("<library_controllers>{}</library_controllers>{}", ctrl("s", "s"), scene("s"));
        assert!(matches!(read_doc(&body), Err(MeshError::Unsupported(_))));
        let chain: String = (0..100_000).map(|i| ctrl(&format!("s{i}"), &format!("s{}", i + 1))).collect();
        let body = format!("<library_controllers>{chain}</library_controllers>{}", scene("s0"));
        assert!(matches!(read_doc(&body), Err(MeshError::Unsupported(_))));
    }

    #[test]
    fn skin_weights_that_disagree_with_their_counts_are_refused() {
        for (v, vcount) in [("0 0 1 0", "1 1 1"), ("0 0 1 0 5 0", "1 1 1"), ("0 0 1 0 1 7", "1 1 1"), ("0 0 1 0 1 0", "1 1")] {
            let body = skin_doc("", r#"<Name_array id="cja" count="2">J0 J1</Name_array>"#, v, vcount, "1");
            assert!(matches!(read_doc(&body), Err(MeshError::Malformed(_))), "{v} / {vcount}");
        }
    }

    /// 2026-10-12 review, finding 14: a geometry listed as MORPH_TARGET up to
    /// 1,024 times was decoded and copied whole for each listing (~2.3 MB
    /// per listing of a 200k-vertex base), with nothing charging it.
    #[test]
    fn morph_targets_are_charged_against_the_morph_budget() {
        let n = 20_001;
        let pos: Vec<f32> = (0..n).flat_map(|i| [i as f32, (i % 7) as f32, 0.0]).collect();
        let p: String = (0..n).map(|i| i.to_string()).collect::<Vec<_>>().join(" ");
        let base = geom("base", &pos, &format!(r##"<triangles count="{}"><input semantic="VERTEX" source="#base-v" offset="0"/><p>{p}</p></triangles>"##, n / 3));
        let tgt = geom("tgt", &pos, "");
        let k = MAX_MORPH_TARGETS;
        let ids = vec!["tgt"; k].join(" ");
        let weights = vec!["0"; k].join(" ");
        let body = format!(
            r##"<library_controllers><controller id="mc"><morph source="#base" method="RELATIVE"><source id="mt"><IDREF_array id="mta" count="{k}">{ids}</IDREF_array><technique_common><accessor source="#mta" count="{k}" stride="1"><param name="IDREF" type="IDREF"/></accessor></technique_common></source><source id="mw"><float_array id="mwa" count="{k}">{weights}</float_array><technique_common><accessor source="#mwa" count="{k}" stride="1"><param name="MORPH_WEIGHT" type="float"/></accessor></technique_common></source><targets><input semantic="MORPH_TARGET" source="#mt"/><input semantic="MORPH_WEIGHT" source="#mw"/></targets></morph></controller></library_controllers>{}"##,
            scene_doc(&format!("{base}{tgt}"), r##"<node id="n"><instance_controller url="#mc"/></node>"##)
        );
        let e = read_doc(&body).map(|_| ()).unwrap_err();
        assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("morph")), "{e:?}");
    }

    /// Also finding 14: every primitive vertex rebuilt its skin vertex's
    /// whole influence list (and kept its capacity after truncating): 20k
    /// influences on one vertex, used by 3,000 corners, reached 751 MB.
    #[test]
    fn a_skin_vertex_s_influences_are_gathered_once_for_every_corner_using_it() {
        let corners = 3000;
        let nrm: Vec<f32> = (0..corners).flat_map(|i| [0.0, 0.0, 1.0 + i as f32]).collect();
        let nsrc = format!(r##"<source id="g-n"><float_array id="g-na" count="{}">{}</float_array><technique_common><accessor source="#g-na" count="{corners}" stride="3"><param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/></accessor></technique_common></source>"##, nrm.len(), floats(&nrm));
        let p: String = (0..corners).map(|i| format!("{} {i}", i % 3)).collect::<Vec<_>>().join(" ");
        let g = geom("g", &TRI, &format!(r##"<triangles count="{}"><input semantic="VERTEX" source="#g-v" offset="0"/><input semantic="NORMAL" source="#g-n" offset="1"/><p>{p}</p></triangles>"##, corners / 3)).replace("<vertices", &format!("{nsrc}<vertices"));
        let heavy = 20_000;
        let v: String = std::iter::repeat_n("0 0", 3 * heavy).collect::<Vec<_>>().join(" ");
        let body = skin_doc("", r#"<Name_array id="cja" count="2">J0 J1</Name_array>"#, &v, &format!("{heavy} {heavy} {heavy}"), "1")
            .replace(r##"<library_geometries><geometry id="g" name="g">"##, "<library_geometries><geometry id=\"unused\" name=\"unused\">");
        let body = body.replacen("<library_geometries>", &format!("<library_geometries>{g}"), 1);
        let started = std::time::Instant::now();
        let s = read_doc(&body).unwrap().scene;
        assert!(started.elapsed().as_millis() < 1500, "{:?}", started.elapsed());
        let sw = s.meshes[0].primitives[0].skin.as_ref().unwrap();
        assert!(sw.influences <= MAX_INFLUENCES);
    }

    fn morph_doc(method: &str, target: &[f32], weight: &str, anim: &str) -> String {
        let base = geom("base", &TRI, r##"<triangles count="1"><input semantic="VERTEX" source="#base-v" offset="0"/><p>0 1 2</p></triangles>"##);
        let tgt = geom("tgt", target, "");
        format!(
            r##"<library_controllers><controller id="mc"><morph source="#base" method="{method}"><source id="mt"><IDREF_array id="mta" count="1">tgt</IDREF_array><technique_common><accessor source="#mta" count="1" stride="1"><param name="IDREF" type="IDREF"/></accessor></technique_common></source><source id="mw"><float_array id="mwa" count="1">{weight}</float_array><technique_common><accessor source="#mwa" count="1" stride="1"><param name="MORPH_WEIGHT" type="float"/></accessor></technique_common></source><targets><input semantic="MORPH_TARGET" source="#mt"/><input semantic="MORPH_WEIGHT" source="#mw"/></targets></morph></controller></library_controllers>{anim}{}"##,
            scene_doc(&format!("{base}{tgt}"), r##"<node id="n"><instance_controller url="#mc"/></node>"##)
        )
    }

    #[test]
    fn morphs_read_normalized_and_relative_with_weight_animation() {
        let anim = r##"<library_animations><animation id="a"><source id="ai"><float_array id="aia" count="2">0 1</float_array><technique_common><accessor source="#aia" count="2" stride="1"><param name="TIME" type="float"/></accessor></technique_common></source><source id="ao"><float_array id="aoa" count="2">0 1</float_array><technique_common><accessor source="#aoa" count="2" stride="1"><param name="WEIGHT" type="float"/></accessor></technique_common></source><sampler id="as"><input semantic="INPUT" source="#ai"/><input semantic="OUTPUT" source="#ao"/></sampler><channel source="#as" target="mwa(0)"/></animation></library_animations>"##;
        let moved = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 3.0, 0.0];
        let s = read_doc(&morph_doc("NORMALIZED", &moved, "0.5", anim)).unwrap().scene;
        let m = &s.meshes[0];
        assert_eq!(m.weights, vec![0.5]);
        assert_eq!(m.primitives[0].targets[0].positions, vec![[0.0; 3], [0.0; 3], [0.0, 2.0, 0.0]], "absolute target minus base");
        assert!(close3(world_positions(&s)[2], [0.0, 2.0, 0.0], 1e-6));
        let w = s.animations[0].channels.iter().find(|c| c.path == AnimPath::Weights).expect("weight channel");
        assert_eq!(w.values, vec![0.0, 1.0]);
        let delta = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0];
        let s = read_doc(&morph_doc("RELATIVE", &delta, "1", "")).unwrap().scene;
        assert_eq!(s.meshes[0].primitives[0].targets[0].positions[2], [0.0, 2.0, 0.0], "relative targets are offsets");
        assert!(close3(world_positions(&s)[2], [0.0, 3.0, 0.0], 1e-6));
        let short = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        assert!(matches!(read_doc(&morph_doc("RELATIVE", &short, "1", "")), Err(MeshError::Malformed(_))));
    }

    // ── reader: animation ───────────────────────────────────────────────────

    fn anim(target: &str, times: &str, values: &str, stride: usize, interp: Option<&str>, tangents: &str) -> String {
        let n = times.split_whitespace().count();
        let params: String = (0..stride).map(|i| format!(r#"<param name="V{i}" type="float"/>"#)).collect();
        let interp_src = interp.map(|i| format!(r##"<source id="ii"><Name_array id="iia" count="{n}">{}</Name_array><technique_common><accessor source="#iia" count="{n}" stride="1"><param name="INTERPOLATION" type="name"/></accessor></technique_common></source>"##, vec![i; n].join(" ")));
        format!(
            r##"<library_animations><animation id="an"><source id="ti"><float_array id="tia" count="{n}">{times}</float_array><technique_common><accessor source="#tia" count="{n}" stride="1"><param name="TIME" type="float"/></accessor></technique_common></source><source id="vo"><float_array id="voa" count="{}">{values}</float_array><technique_common><accessor source="#voa" count="{n}" stride="{stride}">{params}</accessor></technique_common></source>{}{tangents}<sampler id="sm"><input semantic="INPUT" source="#ti"/><input semantic="OUTPUT" source="#vo"/>{}{}</sampler><channel source="#sm" target="{target}"/></animation></library_animations>"##,
            values.split_whitespace().count(),
            interp_src.clone().unwrap_or_default(),
            if interp_src.is_some() { r##"<input semantic="INTERPOLATION" source="#ii"/>"## } else { "" },
            if tangents.is_empty() { "" } else { r##"<input semantic="IN_TANGENT" source="#tin"/><input semantic="OUT_TANGENT" source="#tout"/>"## },
        )
    }

    fn animated(node_content: &str, animation: &str) -> Scene {
        read_doc(&format!("{animation}{}", tri_doc(node_content))).unwrap().scene
    }

    fn x_at(s: &Scene, t: f32) -> f64 {
        pose::sample_pose(s, 0, t).locals()[0][12]
    }

    #[test]
    fn animates_members_elements_and_whole_transforms() {
        let s = animated(r#"<translate sid="loc">0 0 0</translate>"#, &anim("n/loc.X", "0 2", "0 4", 1, Some("LINEAR"), ""));
        assert!((x_at(&s, 1.0) - 2.0).abs() < 1e-5, "{}", x_at(&s, 1.0));
        let s = animated(r#"<matrix sid="m">1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1</matrix>"#, &anim("n/m(0)(3)", "0 2", "0 4", 1, None, ""));
        assert!((x_at(&s, 0.5) - 1.0).abs() < 1e-5);
        let s = animated(r#"<translate sid="t">0 0 0</translate>"#, &anim("n/t", "0 1", "0 0 0 3 0 0", 3, Some("STEP"), ""));
        assert!((x_at(&s, 0.999) - 0.0).abs() < 1e-9 && (x_at(&s, 1.0) - 3.0).abs() < 1e-9);
        assert_eq!(s.animations[0].channels[0].interpolation, Interpolation::Step);
        // rotate ANGLE: 0 → 180° about Z resampled, so the half-way pose is a
        // quarter turn even though slerp between the end keys is ambiguous.
        let s = animated(r#"<rotate sid="rz">0 0 1 0</rotate>"#, &anim("n/rz.ANGLE", "0 1", "0 180", 1, Some("LINEAR"), ""));
        let m = pose::sample_pose(&s, 0, 0.5).locals()[0];
        assert!((m[0] - 0.0).abs() < 1e-3 && (m[1] - 1.0).abs() < 1e-3, "{m:?}");
    }

    #[test]
    fn a_node_scaled_in_from_nothing_reads_and_writes() {
        // Read: scale 0 → 2 on a node; the zero key has no rotation to
        // decompose, which must not make the file unreadable.
        let s = animated(r#"<rotate sid="r">0 0 1 30</rotate><scale sid="s">1 1 1</scale>"#, &anim("n/s", "0 1", "0 0 0 2 2 2", 3, Some("LINEAR"), ""));
        let half = pose::sample_pose(&s, 0, 0.5).trs[0].unwrap();
        assert!(half.2.iter().all(|v| (v - 1.0).abs() < 1e-5), "{half:?}");
        let r30 = math::quat_from_axis_angle([0.0, 0.0, 1.0], 30f64.to_radians());
        assert!(math::quat_dot(half.1, r30).abs() > 1.0 - 1e-6, "the zero key keeps the node's rotation: {half:?}");
        // Write: a zero-scale key goes out as a millionth, which three.js can
        // decompose, and comes back as (nearly) nothing.
        let mut fx = fixtures::skinned();
        let ch = &mut fx.animations[0].channels[3];
        ch.interpolation = Interpolation::Linear;
        ch.values = vec![0.0, 0.0, 0.0, 1.25, 1.25, 1.25, 1.0, 1.0, 1.0];
        let t = text(&package(&fx, WriteOptions::default()));
        let hip = t.find("<float_array id=\"hip-anim-output-array\"").unwrap();
        assert!(t[hip..hip + 120].contains(">0.000001 0 0 0 0 0.000001 0 0 0 0 0.000001 0"), "{}", &t[hip..hip + 120]);
        let back = read_package(&package(&fx, WriteOptions::default())).scene;
        let s0 = pose::sample_pose(&back, 0, 0.0).trs[0].unwrap().2;
        assert!(s0.iter().all(|v| v.abs() < 1e-5), "{s0:?}");
    }

    #[test]
    fn evaluates_bezier_and_hermite_keys() {
        // 2-D Bézier tangents with controls on the chord: a straight line.
        let tangents = r##"<source id="tin"><float_array id="tina" count="4">0 0 1.3333334 2.6666667</float_array><technique_common><accessor source="#tina" count="2" stride="2"><param name="X" type="float"/><param name="Y" type="float"/></accessor></technique_common></source><source id="tout"><float_array id="touta" count="4">0.6666667 1.3333334 2 4</float_array><technique_common><accessor source="#touta" count="2" stride="2"><param name="X" type="float"/><param name="Y" type="float"/></accessor></technique_common></source>"##;
        let s = animated(r#"<translate sid="t">0 0 0</translate>"#, &anim("n/t.X", "0 2", "0 4", 1, Some("BEZIER"), tangents));
        for t in [0.3, 1.0, 1.7] {
            assert!((x_at(&s, t) - 2.0 * t as f64).abs() < 1e-3, "t={t}: {}", x_at(&s, t));
        }
        // Ease in/out with flat 1-D tangents: the midpoint is half-way and the
        // quarter point is the smoothstep's 0.15625 of the way.
        let flat = r##"<source id="tin"><float_array id="tina" count="2">0 4</float_array><technique_common><accessor source="#tina" count="2" stride="1"><param name="X" type="float"/></accessor></technique_common></source><source id="tout"><float_array id="touta" count="2">0 4</float_array><technique_common><accessor source="#touta" count="2" stride="1"><param name="X" type="float"/></accessor></technique_common></source>"##;
        let s = animated(r#"<translate sid="t">0 0 0</translate>"#, &anim("n/t.X", "0 2", "0 4", 1, Some("BEZIER"), flat));
        assert!((x_at(&s, 1.0) - 2.0).abs() < 1e-3);
        assert!(x_at(&s, 0.5) < 1.0, "eases in: {}", x_at(&s, 0.5));
        // Hermite: zero tangents are the same smoothstep.
        let zero = flat.replace(">0 4</float_array>", ">0 0</float_array>");
        let s = animated(r#"<translate sid="t">0 0 0</translate>"#, &anim("n/t.X", "0 2", "0 4", 1, Some("HERMITE"), &zero));
        assert!((x_at(&s, 0.5) - 4.0 * 0.15625).abs() < 2e-2, "{}", x_at(&s, 0.5));
        // Unsupported curve types say so.
        let r = read_doc(&format!("{}{}", anim("n/t.X", "0 2", "0 4", 1, Some("BSPLINE"), ""), tri_doc(r#"<translate sid="t">0 0 0</translate>"#)));
        assert!(matches!(r, Err(MeshError::Unsupported(_))));
    }

    #[test]
    fn animation_clips_become_separate_animations() {
        let a = anim("n/t.X", "0 1", "0 1", 1, None, "");
        let b = a.replace(r#"id="an""#, r#"id="an2""#).replace("0 1</float_array>", "0 2</float_array>").replace(r#"id="sm""#, r#"id="sm2""#).replace("#sm\"", "#sm2\"");
        let clips = r##"<library_animation_clips><animation_clip id="walk" name="Walk" start="0" end="1"><instance_animation url="#an"/></animation_clip><animation_clip id="run" end="1"><instance_animation url="#an2"/></animation_clip></library_animation_clips>"##;
        let body = format!("{a}{}{clips}{}", b.replace("<library_animations>", "<library_animations>"), tri_doc(r#"<translate sid="t">0 0 0</translate>"#));
        let s = read_doc(&body).unwrap().scene;
        assert_eq!(s.animations.len(), 2);
        assert_eq!(s.animations[0].name.as_deref(), Some("Walk"));
        assert_eq!(s.animations[1].name.as_deref(), Some("run"));
    }

    /// 2026-10-12 review, finding 26.2: an `<animation_clip>`'s start and end
    /// (seconds; 1.4.1: start defaults to 0, end to the longest animation's
    /// end) cut its stretch out of the animations it instances, which then
    /// plays from 0. Two clips of one 4 s animation each played all of it.
    #[test]
    fn a_clip_plays_only_its_start_to_end() {
        let a = anim("n/t.X", "0 4", "0 4", 1, None, "");
        let clips = r##"<library_animation_clips><animation_clip id="A" start="0" end="2"><instance_animation url="#an"/></animation_clip><animation_clip id="B" start="2" end="4"><instance_animation url="#an"/></animation_clip><animation_clip id="C" start="1"><instance_animation url="#an"/></animation_clip></library_animation_clips>"##;
        let s = read_doc(&format!("{a}{clips}{}", tri_doc(r#"<translate sid="t">0 0 0</translate>"#))).unwrap().scene;
        let x = |clip: usize, t: f32| pose::sample_pose(&s, clip, t).trs[0].unwrap().0[0];
        assert_eq!(s.animations[0].time_range(), Some((0.0, 2.0)));
        assert_eq!(s.animations[1].time_range(), Some((0.0, 2.0)));
        assert_eq!(s.animations[2].time_range(), Some((0.0, 3.0)), "end defaults to the animation's end");
        for (clip, t, want) in [(0, 0.0, 0.0), (0, 1.0, 1.0), (0, 2.0, 2.0), (1, 0.0, 2.0), (1, 1.0, 3.0), (1, 2.0, 4.0), (2, 0.0, 1.0)] {
            assert!((x(clip, t) - want).abs() < 1e-5, "clip {clip} at {t}: {} vs {want}", x(clip, t));
        }
    }

    /// 2026-10-12 review, finding 26.3: with clips present, an animation no
    /// clip instances is in no clip; it is named in a warning rather than
    /// lost without one.
    #[test]
    fn an_animation_in_no_clip_is_reported() {
        let a = anim("n/t.X", "0 1", "0 1", 1, None, "");
        let b = a.replace(r#"id="an""#, r#"id="an2""#).replace("0 1</float_array>", "0 2</float_array>").replace(r#"id="sm""#, r#"id="sm2""#).replace("#sm\"", "#sm2\"");
        let clips = r##"<library_animation_clips><animation_clip id="walk"><instance_animation url="#an"/></animation_clip></library_animation_clips>"##;
        let r = read_doc(&format!("{a}{b}{clips}{}", tri_doc(r#"<translate sid="t">0 0 0</translate>"#))).unwrap();
        assert_eq!(r.scene.animations.len(), 1);
        assert!(r.warnings.iter().any(|w| w.contains("no <animation_clip>")), "{:?}", r.warnings);
    }

    #[test]
    fn a_clip_starts_at_zero_even_when_its_first_key_is_later() {
        // three.js plays a clip for `end - start` seconds: a clip spanning
        // its keys (CesiumMan's begin at 1/24 s) would stop short of the last.
        let mut s = fixtures::skinned();
        for ch in s.animations[0].channels.iter_mut() {
            for t in ch.times.iter_mut() {
                *t += 0.5;
            }
        }
        let (_, hi) = s.animations[0].time_range().unwrap();
        let t = text(&package(&s, WriteOptions::default()));
        let clip = t.split("<animation_clip ").nth(1).expect("a clip").split('>').next().unwrap();
        assert!(clip.contains(r#"start="0""#), "{clip}");
        assert!(clip.contains(&format!(r#"end="{hi}""#)), "{clip}");
    }

    #[test]
    fn channels_on_things_the_scene_does_not_model_are_reported() {
        let r = read_doc(&format!("{}{}", anim("cam/fov", "0 1", "40 50", 1, None, ""), tri_doc(""))).unwrap();
        assert!(r.scene.animations.is_empty());
        assert!(r.warnings.iter().any(|w| w.contains("skipped")), "{:?}", r.warnings);
    }

    // ── independent producers ───────────────────────────────────────────────

    fn testdata(bytes: &[u8]) -> SceneRead {
        let files = vec![
            PackageFile { path: "textures/quadrants.png".into(), bytes: fixtures::quadrant_png() },
            PackageFile { path: "textures/bands.png".into(), bytes: fixtures::bands_png() },
            PackageFile { path: "textured_assimp_texture_0001.png".into(), bytes: fixtures::quadrant_png() },
            PackageFile { path: "basecolor.png".into(), bytes: include_bytes!("testdata/dae/research/basecolor.png").to_vec() },
        ];
        read_scene(bytes, &MemorySidecars { files: &files }).unwrap()
    }

    /// The world-space bounds of a scene at time `t` of its first clip.
    fn bounds_at(s: &Scene, t: Option<f32>) -> ([f32; 3], [f32; 3]) {
        let pose = match t {
            Some(t) => pose::sample_pose(s, 0, t),
            None => pose::default_pose(s),
        };
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for w in pose::world_primitives(s, &pose).unwrap() {
            for p in w.positions {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
        }
        (lo, hi)
    }

    #[test]
    fn reads_scenekits_export_of_the_skinned_fixture() {
        let r = testdata(include_bytes!("testdata/dae/scenekit/skinned_scenekit.dae"));
        let (s, fx) = (&r.scene, fixtures::skinned());
        assert_eq!(s.triangle_count(), fx.triangle_count());
        assert_eq!(s.skins.len(), 1);
        let names: Vec<_> = s.skins[0].joints.iter().map(|j| s.nodes[*j].name.clone().unwrap()).collect();
        assert_eq!(names, ["hip", "knee", "ankle"], "sids SID_4.. resolve through <skeleton>");
        assert!(s.materials[0].base_color_texture.is_some(), "set=\"1\" bound through CHANNEL2: {:?}", r.warnings);
        assert_eq!(s.animations.len(), 1);
        // The bend, between keys, against the fixture itself.
        for t in fixtures::sample_times(&fx) {
            let (a, b) = (bounds_at(s, Some(t)), bounds_at(&fx, Some(t)));
            assert!(close3(a.0, b.0, 2e-3) && close3(a.1, b.1, 2e-3), "t={t}: {a:?} vs {b:?}");
        }
    }

    #[test]
    fn reads_scenekits_export_of_the_textured_fixture() {
        let r = testdata(include_bytes!("testdata/dae/scenekit/textured_scenekit.dae"));
        let s = &r.scene;
        let fx = fixtures::textured();
        assert_eq!(s.triangle_count(), fx.triangle_count());
        assert_eq!(bounds_at(s, None).0.map(|v| (v * 1e4).round()), bounds_at(&fx, None).0.map(|v| (v * 1e4).round()));
        assert!(s.meshes.iter().flat_map(|m| &m.primitives).any(|p| p.colors.is_some()), "the panel's vertex colours");
        let wedge = s.materials.iter().find(|m| m.name.as_deref() == Some("wedge_mat")).unwrap();
        assert!((0..3).all(|k| (wedge.base_color[k] - [0.9, 0.3, 0.1][k]).abs() < 2e-3), "{:?}", wedge.base_color);
        assert!(s.materials.iter().any(|m| m.base_color_texture.is_some()));
        let cube = testdata(include_bytes!("testdata/dae/scenekit/cube_scenekit.dae")).scene;
        assert_eq!(bounds_at(&cube, None), bounds_at(&fixtures::cube(), None));
    }

    #[test]
    fn reads_assimps_exports() {
        let s = testdata(include_bytes!("testdata/dae/assimp/textured_assimp.dae")).scene;
        let fx = fixtures::textured();
        assert_eq!(s.triangle_count(), fx.triangle_count());
        let (a, b) = (bounds_at(&s, None), bounds_at(&fx, None));
        assert!(close3(a.0, b.0, 1e-4) && close3(a.1, b.1, 1e-4), "{a:?} vs {b:?}");
        assert!(s.materials.iter().any(|m| m.base_color_texture.is_some()));
        let cube = testdata(include_bytes!("testdata/dae/assimp/cube_assimp.dae")).scene;
        assert_eq!(bounds_at(&cube, None), bounds_at(&fixtures::cube(), None));
    }

    #[test]
    fn reads_the_research_references() {
        let r = testdata(include_bytes!("testdata/dae/research/ref.dae"));
        let s = &r.scene;
        assert_eq!(s.triangle_count(), 5);
        assert_eq!(s.materials.len(), 2);
        assert_eq!(s.skins.len(), 1);
        assert!(s.materials.iter().any(|m| m.double_sided), "GOOGLEEARTH double_sided");
        let red = s.materials.iter().find(|m| m.name.as_deref() == Some("RedMat")).unwrap();
        assert_eq!(red.base_color[..3], [1.0, 0.0, 0.0]);
        assert!(s.animations[0].channels.iter().any(|c| c.interpolation == Interpolation::Step), "Bone0's STEP channel");
        let m = testdata(include_bytes!("testdata/dae/research/morph.dae")).scene;
        assert_eq!(m.meshes[0].target_count(), 1, "morph under a skin");
        assert!(m.animations[0].channels.iter().any(|c| c.path == AnimPath::Weights));
        let rel = testdata(include_bytes!("testdata/dae/research/morph_relative_full.dae")).scene;
        assert_eq!(rel.meshes[0].weights, vec![1.0]);
        let top = world_positions(&rel)[4];
        assert!(close3(top, [0.0, 2.0, 0.0], 1e-6), "-0.5 + 0.5 offset: {top:?}");
    }

    // ── hostile input ───────────────────────────────────────────────────────

    #[test]
    fn other_containers_are_not_this_format() {
        let glb = super::super::write(&pose::flatten(&fixtures::cube()).unwrap(), MeshFormat::Glb).unwrap();
        for bytes in [
            glb,
            b"ply\nformat ascii 1.0\n".to_vec(),
            b"solid x\nendsolid x\n".to_vec(),
            b"v 0 0 0\nf 1 1 1\n".to_vec(),
            b"{\"asset\":{\"version\":\"2.0\"}}".to_vec(),
            b"<?xml version=\"1.0\"?><model xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\"/>".to_vec(),
            b"<!DOCTYPE html><html><body>oops</body></html>".to_vec(),
            vec![],
            vec![0xFF, 0xFE, b'<', 0],
        ] {
            assert!(matches!(read(&bytes), Err(MeshError::NotThisFormat { .. })), "{:?}", String::from_utf8_lossy(&bytes[..bytes.len().min(30)]));
        }
    }

    #[test]
    fn never_panics_on_truncation_or_corruption() {
        let pkg = package(&fixtures::skinned(), WriteOptions::default());
        let full = &pkg.files[0].bytes;
        let step = (full.len() / 400).max(1);
        for cut in (0..full.len()).step_by(step) {
            let _ = read(&full[..cut]);
        }
        for i in (0..full.len()).step_by(step) {
            for flip in [0x01u8, 0x20, 0x80] {
                let mut v = full.clone();
                v[i] ^= flip;
                let _ = read(&v);
            }
        }
    }

    /// The Collada file more than two uv sets are read from, multiuv as this
    /// module writes it as a package, through the reader: cut at every
    /// length, with each bit of its first 4 KiB flipped in turn, and with 2–8
    /// bits flipped at random in 10,000 seeded cases. Every reading is `Ok`
    /// or `Err`, never a panic. usd.rs's test of the same name runs the USD
    /// inputs, and ply.rs's `big_endian_truncation_and_bit_flips_never_panic`
    /// is the pattern.
    #[test]
    fn uv_set_inputs_survive_truncation_and_bit_flips() {
        const FLIP_SEED: u64 = 0x9E37_79B9_7F4A_7C15;
        let pkg = package(&fixtures::multiuv(), WriteOptions { self_contained: false, ..Default::default() });
        let file = &pkg.files[0].bytes;
        let side = &MemorySidecars { files: &pkg.files };
        let whole = super::super::read_scene(file, MeshFormat::Dae, side).unwrap_or_else(|e| panic!("multiuv as Collada: {e}"));
        assert_eq!(whole.scene.meshes[0].primitives[0].uvs.len(), 4, "the document itself carries the four sets");
        let survives =
            move |bytes: &[u8]| std::panic::catch_unwind(move || super::super::read_scene(bytes, MeshFormat::Dae, side)).is_ok();
        for cut in 0..file.len() {
            assert!(survives(&file[..cut]), "cut to {cut} bytes");
        }
        let mut flipped = file.clone();
        for bit in 0..file.len().min(4096) * 8 {
            flipped[bit / 8] ^= 1 << (bit % 8);
            assert!(survives(&flipped), "bit {bit} flipped");
            flipped[bit / 8] ^= 1 << (bit % 8);
        }
        // xorshift64 (Marsaglia, 2003): std only, and the same cases every run.
        let mut state = FLIP_SEED;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let bits = file.len() as u64 * 8;
        for case in 0..10_000 {
            let flips = 2 + next() % 7;
            let mut chosen: Vec<u64> = Vec::new();
            while (chosen.len() as u64) < flips {
                let bit = next() % bits;
                if !chosen.contains(&bit) {
                    chosen.push(bit);
                }
            }
            let mut flipped = file.clone();
            for &bit in &chosen {
                flipped[(bit / 8) as usize] ^= 1 << (bit % 8);
            }
            assert!(survives(&flipped), "case {case}: bits {chosen:?} flipped");
        }
    }

    /// `file`, read whole through `side`, where it must carry the iors
    /// `want`, then swept by [`layer_sweep`] with every bit from its first
    /// `<index_of_refraction>` to its last among the single flips (in
    /// layered's document both lie in the first 4 KiB). Every reading is `Ok`
    /// or `Err`.
    fn dae_layer_sweep(name: &str, file: &[u8], side: &dyn Sidecars, want: &[f32]) {
        let whole = super::super::read_scene(file, MeshFormat::Dae, side).unwrap_or_else(|e| panic!("{name}: {e}"));
        let iors: Vec<f32> = whole.scene.materials.iter().filter_map(|m| m.ior).collect();
        assert_eq!(iors, want, "{name} itself carries its iors");
        let close = "</index_of_refraction>";
        let first = span(name, file, "<index_of_refraction>", close);
        let last = file.windows(close.len()).rposition(|w| w == close.as_bytes()).expect("span found one") + close.len();
        layer_sweep(name, file, first.start..last, |bytes| {
            let _ = super::super::read_scene(bytes, MeshFormat::Dae, side);
        });
    }

    /// `scene` as this module writes it as a package, through
    /// [`dae_layer_sweep`]: the glass's ior of 1.52 and the plastic's of 1.6.
    fn dae_layered_sweep(label: &str, scene: &Scene) {
        let pkg = package(scene, WriteOptions { self_contained: false, ..Default::default() });
        dae_layer_sweep(&format!("{label} as Collada"), &pkg.files[0].bytes, &MemorySidecars { files: &pkg.files }, &[1.52, 1.6]);
    }

    /// `<index_of_refraction>`, as layered carries it and in its `<param>`
    /// form, through the reader: lean layered's glass and plastic, the two
    /// of its materials whose iors Collada keeps ([`dae_layered_sweep`] over
    /// [`lean_layered`]; the lacquer's 1.45 is an exporter's default, which
    /// the writer leaves out), and the `<param>` naming a `<newparam>`'s
    /// float of `a_bad_index_of_refraction_is_left_out_with_a_warning`,
    /// which the writer never writes ([`dae_layer_sweep`]).
    #[test]
    fn dae_layer_inputs_survive_truncation_and_bit_flips() {
        dae_layered_sweep("lean layered", &lean_layered(&["glass", "plastic"]));
        let param = doc(&material_doc(
            r#"<phong><index_of_refraction><param ref="n"/></index_of_refraction></phong>"#,
            r#"<newparam sid="n"><float>1.33</float></newparam>"#,
            "",
            "",
        ));
        dae_layer_sweep("a <param> ior", &param, &NoSidecars, &[1.33]);
    }

    /// [`dae_layered_sweep`] over the full-size `fixtures::layered()`: two
    /// minutes in a debug build, seconds released. Run it with the other
    /// full-size sweeps:
    /// `cargo test --release --lib -- --ignored layer_inputs_survive`.
    #[test]
    #[ignore = "full-size layered: minutes in a debug build; run it released (see the doc comment)"]
    fn dae_layer_inputs_survive_truncation_and_bit_flips_full() {
        dae_layered_sweep("layered", &fixtures::layered());
    }

    #[test]
    fn huge_counts_are_refused_before_anything_is_sized_from_them() {
        let started = std::time::Instant::now();
        let base = tri_doc("");
        for (from, to) in [
            (r#"count="9">"#, r#"count="4000000000">"#),
            (r#"count="3" stride="3""#, r#"count="4000000000" stride="3""#),
            (r#"<triangles count="1">"#, r#"<triangles count="4000000000">"#),
            (r#"stride="3""#, r#"stride="4000000000""#),
            (r#"offset="0"/><p>"#, r#"offset="4000000000"/><p>"#),
            ("<p>0 1 2</p>", "<p>0 1 4000000000</p>"),
            ("<p>0 1 2</p>", "<p>0 1 -2</p>"),
            ("0 0 0 1 0 0 0 1 0", "0 0 0 1 0 0 0 1 nan"),
            ("0 0 0 1 0 0 0 1 0", "0 0 0 1 0 0 0 1 1e999"),
        ] {
            assert!(base.contains(from), "{from}");
            let r = read_doc(&base.replacen(from, to, 1));
            assert!(matches!(r, Err(MeshError::Malformed(_)) | Err(MeshError::Unsupported(_))), "{to}: {r:?}");
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[test]
    fn error_messages_quote_a_bounded_amount_of_input() {
        let id = "x".repeat(1 << 20);
        let body = tri_doc("").replace(r##"url="#g""##, &format!(r##"url="#{id}""##));
        let e = read_doc(&body).unwrap_err().to_string();
        assert!(e.len() < 1000, "{} bytes", e.len());
    }

    #[test]
    fn a_document_with_no_triangles_is_empty() {
        let body = scene_doc(&geom("g", &TRI, ""), r##"<node id="n"><instance_geometry url="#g"/></node>"##);
        assert_eq!(read_doc(&body).unwrap_err(), MeshError::Empty);
        assert!(matches!(read_doc(""), Err(MeshError::Empty) | Err(MeshError::Malformed(_))));
    }

    #[test]
    fn deeply_nested_animations_index_their_channels_in_linear_time() {
        // 500 nested <animation>s around 100k channels: indexing every channel
        // under every enclosing animation used to cost 50M entries here.
        let mut body = String::from("<library_animations>");
        for i in 0..500 {
            body.push_str(&format!(r#"<animation id="a{i}">"#));
        }
        for _ in 0..100_000 {
            body.push_str(r#"<channel source="" target=""/>"#);
        }
        for _ in 0..500 {
            body.push_str("</animation>");
        }
        body.push_str("</library_animations>");
        body.push_str(&tri_doc(""));
        let started = std::time::Instant::now();
        let _ = read_doc(&body);
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "took {:?}", started.elapsed());
    }

    #[test]
    fn a_node_with_an_absurd_transform_stack_is_refused() {
        let stacked = |n: usize| tri_doc(&"<translate>0 0 0</translate>".repeat(n));
        assert!(read_doc(&stacked(MAX_NODE_OPS)).is_ok(), "the ceiling itself is allowed");
        let e = read_doc(&stacked(MAX_NODE_OPS + 1)).unwrap_err();
        assert!(e.to_string().contains("transform elements"), "{e}");
    }

    // ── lights and cameras ──────────────────────────────────────────────────

    /// A document of one triangle on node `tri`, with `lights` and `cameras`
    /// as the contents of their libraries and `nodes` beside `tri`.
    fn lit_doc(lights: &str, cameras: &str, nodes: &str) -> String {
        format!(
            "<library_lights>{lights}</library_lights><library_cameras>{cameras}</library_cameras>{}",
            scene_doc(
                &geom("g", &TRI, r##"<triangles count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><p>0 1 2</p></triangles>"##),
                &format!(r##"<node id="tri" name="tri"><instance_geometry url="#g"/></node>{nodes}"##),
            )
        )
    }

    /// A `<light>` whose id and name are `name`, `shape` its
    /// `<technique_common>`'s content and `extra` after that, and a node
    /// `{name}_node` placing it.
    fn one_light(name: &str, shape: &str, extra: &str) -> (String, String) {
        (
            format!(r#"<light id="{name}" name="{name}"><technique_common>{shape}</technique_common>{extra}</light>"#),
            format!(r##"<node id="{name}_node" name="{name}_node"><instance_light url="#{name}"/></node>"##),
        )
    }

    /// [`one_light`] for a `<camera>`, `projection` its optics' common
    /// technique's content and `extra` after that, inside `<optics>`.
    fn one_camera(name: &str, projection: &str, extra: &str) -> (String, String) {
        (
            format!(r#"<camera id="{name}" name="{name}"><optics><technique_common>{projection}</technique_common>{extra}</optics></camera>"#),
            format!(r##"<node id="{name}_node" name="{name}_node"><instance_camera url="#{name}"/></node>"##),
        )
    }

    /// An `<extra>` holding a litegen technique of `content`.
    fn litegen_extra(content: &str) -> String {
        format!(r#"<extra><technique profile="litegen">{content}</technique></extra>"#)
    }

    /// [`read_doc`] for a document whose `<unit>` is `meter` metres.
    fn read_doc_in(meter: &str, body: &str) -> Result<SceneRead, MeshError> {
        let d = String::from_utf8(doc(body)).unwrap();
        let d = d.replacen("</modified></asset>", &format!("</modified><unit meter=\"{meter}\"/></asset>"), 1);
        read_scene(d.as_bytes(), &NoSidecars)
    }

    /// The light the node named `node` places, if it places one.
    fn light_on<'s>(s: &'s Scene, node: &str) -> Option<&'s Light> {
        let n = s.nodes.iter().find(|n| n.name.as_deref() == Some(node))?;
        n.light.map(|l| &s.lights[l])
    }

    /// The camera the node named `node` places, if it places one.
    fn camera_on<'s>(s: &'s Scene, node: &str) -> Option<&'s Camera> {
        let n = s.nodes.iter().find(|n| n.name.as_deref() == Some(node))?;
        n.camera.map(|c| &s.cameras[c])
    }

    /// The lights and cameras each node of `s` places, by node name: what a
    /// round trip must give back value for value.
    fn placed(s: &Scene) -> Vec<(String, Option<Light>, Option<Camera>)> {
        let mut v: Vec<(String, Option<Light>, Option<Camera>)> = s
            .nodes
            .iter()
            .filter(|n| n.light.is_some() || n.camera.is_some())
            .map(|n| (n.name.clone().unwrap_or_default(), n.light.map(|l| s.lights[l].clone()), n.camera.map(|c| s.cameras[c].clone())))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// `doc` without the `<extra>` elements that hold a litegen technique:
    /// what a reader of the common profile alone sees, or what is left after
    /// a tool that drops unknown extras.
    fn without_litegen_extras(doc: &str) -> String {
        let (mut out, mut rest) = (String::new(), doc);
        while let Some(start) = rest.find("<extra>") {
            let end = start + rest[start..].find("</extra>").expect("every <extra> closes") + "</extra>".len();
            out.push_str(&rest[..start]);
            if !rest[start..end].contains(r#"profile="litegen""#) {
                out.push_str(&rest[start..end]);
            }
            rest = &rest[end..];
        }
        out.push_str(rest);
        out
    }

    /// The number in the first `<tag>` of `doc`, which must have one.
    fn number_in(doc: &str, tag: &str) -> f64 {
        let open = format!("<{tag}>");
        let at = doc.find(&open).unwrap_or_else(|| panic!("no <{tag}>")) + open.len();
        let end = at + doc[at..].find('<').unwrap();
        doc[at..end].trim().parse().unwrap_or_else(|_| panic!("<{tag}> holds {:?}", &doc[at..end]))
    }

    fn near3(a: [f64; 3], b: [f64; 3], eps: f64) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() <= eps)
    }

    /// `staged()` through Collada comes back exactly, through the litegen
    /// `<extra>` each `<light>` and each camera's `<optics>` carries: every
    /// light and camera at rest and at every sample time (`lightcam`, 1e-5),
    /// and each value as the prepared scene has it, names included, in both
    /// write modes, with nothing about one warned on the way back. Only the
    /// bulb's range is reported lost (Ruling R27): Collada has no form for
    /// it, so other readers light past it, and only the extra keeps it.
    #[test]
    fn every_light_and_camera_round_trips_through_collada() {
        let fixture = fixtures::staged();
        for self_contained in [false, true] {
            let opts = WriteOptions { self_contained, ..Default::default() };
            let ctx = if self_contained { "single file" } else { "package" };
            let w = super::super::write_scene(&fixture, MeshFormat::Dae, opts).unwrap();
            let lost: Vec<&Loss> = w.losses.iter().filter(|l| matches!(l.kind, LossKind::Lights | LossKind::Cameras)).collect();
            let named: Vec<(LossKind, &str)> = lost.iter().map(|l| (l.kind, l.what.as_str())).collect();
            assert_eq!(named, [(LossKind::Lights, "the range of 1 light(s) [bulb]")], "{ctx}: {lost:?}");
            assert!(lost[0].why.starts_with("dae has no light range") && lost[0].why.contains("<extra>"), "{ctx}: {lost:?}");
            let t = String::from_utf8(w.package.files[0].bytes.clone()).unwrap();
            assert_eq!(t.matches(r#"<technique profile="litegen">"#).count(), 5, "{ctx}: an extra on each of the three lights and two cameras");
            let r = read_scene(&w.package.files[0].bytes, &w.package.resolver()).unwrap();
            let about: Vec<&String> = r.warnings.iter().filter(|w| w.contains("light") || w.contains("camera")).collect();
            assert!(about.is_empty(), "{ctx}: {about:?}");
            let want = prepare(&fixture, opts).unwrap();
            lightcam::assert_same_lights_and_cameras(&want, &r.scene, 1e-5, ctx);
            assert_eq!(placed(&r.scene), placed(&want), "{ctx}: every value exactly");
        }
    }

    /// The same file without its litegen extras, as a reader of the common
    /// profile alone sees it, reads by the documented mapping: `<color>`,
    /// linear (Ruling R17), split by moving its largest component into the
    /// intensity (staged's colours each peak at 1, so the split gives the
    /// fixture's intensities, here within 1e-3 relatively), the spot's inner
    /// cone from `<falloff_exponent>` ([`inner_cone`]), no range (Collada has
    /// none), and every camera value as written. The world frames are
    /// untouched (`lightcam`, 1e-5, against the fixture with no ranges).
    #[test]
    fn without_the_litegen_extra_lights_and_cameras_follow_the_documented_mapping() {
        let fixture = fixtures::staged();
        let pkg = package(&fixture, WriteOptions::default());
        let full = text(&pkg);
        assert_eq!(full.matches(r#"profile="litegen""#).count(), 5, "the written file carries the five extras");
        let bare = without_litegen_extras(&full);
        assert!(!bare.contains(r#"profile="litegen""#), "all stripped");
        let r = read_scene(bare.as_bytes(), &pkg.resolver()).unwrap();
        let mut want = prepare(&fixture, WriteOptions::default()).unwrap();
        for l in want.lights.iter_mut() {
            l.range = None;
        }
        lightcam::assert_same_lights_and_cameras(&want, &r.scene, 1e-5, "without the extras");
        for l in &fixture.lights {
            let node = format!("{}_node", l.name.as_deref().unwrap());
            let got = light_on(&r.scene, &node).unwrap();
            assert!(((got.intensity - l.intensity) / l.intensity).abs() <= 1e-3, "{node}: intensity {} against {}", got.intensity, l.intensity);
        }
    }

    /// COLLADA 1.4.1 (2nd edition, March 2008), `<spot>`, pp. 5-119 to
    /// 5-120, all it says of the angle (its Concepts, the row of its
    /// Child Elements table, and its Details):
    ///
    /// > A spot light source radiates light in one direction in a cone shape
    /// > from a known location in space. The intensity of the light is
    /// > attenuated as the radiation angle increases away from the direction
    /// > of the light source. […]
    /// >
    /// > `<falloff_angle sid="...">` — The sid is optional. — Default 180.0 —
    /// > Occurrences 0 or 1
    /// >
    /// > The `<falloff_angle>` and `<falloff_exponent>` are used to specify
    /// > the amount of attenuation based on the direction of the light.
    ///
    /// The entry does not say whether the angle is the cone's full width or
    /// its half. So it is the full cone, in degrees (Ruling R23, which
    /// revises R4's half-angle). SceneKit, the Collada oracle of litegen's
    /// spec, reads it as the full cone (measured by Task 6a's harness), and
    /// Blender writes and reads it so (`LightExporter.cpp` L70,
    /// `DocumentImporter.cpp` L1030, v4.5.0). three.js reads the half-angle
    /// and assimp its inner cone: cited deviations (module docs). Angles are
    /// in degrees ("angle — degrees (standard units)", p. 6-2).
    /// - Read: a hand-written `<falloff_angle>` of 30 is an outer cone 15°
    ///   from the axis, not 30°. The spec's default of 180, with no
    ///   `<falloff_angle>` at all, is a hemisphere, an outer cone of π/2
    ///   from the axis: still a spot, not a point light.
    /// - Written: staged's spot, whose outer cone is 0.6 rad from the axis,
    ///   has a `<falloff_angle>` of 68.75493814777446, twice the f32 0.6 in
    ///   degrees (worked by hand), not 34.37746907388723.
    #[test]
    fn collada_falloff_angle_follows_the_spec() {
        let read = |shape: &str| {
            let (spot, node) = one_light("spot", shape, "");
            light_on(&read_doc(&lit_doc(&spot, "", &node)).unwrap().scene, "spot_node").map(|l| l.kind)
        };
        let outer = |kind: Option<LightKind>| match kind {
            Some(LightKind::Spot { outer_cone, .. }) => Some(outer_cone),
            _ => None,
        };
        let thirty = read("<spot><color>1 1 1</color><falloff_angle>30</falloff_angle><falloff_exponent>8</falloff_exponent></spot>");
        assert!(outer(thirty).is_some_and(|o| (f64::from(o) - std::f64::consts::FRAC_PI_6 / 2.0).abs() < 1e-7), "a 30° cone is 15° from the axis: {thirty:?}");
        let default = read("<spot><color>1 1 1</color></spot>");
        assert_eq!(outer(default), Some(std::f32::consts::FRAC_PI_2), "the default 180° cone is a hemisphere: {default:?}");
        let t = text(&package(&fixtures::staged(), WriteOptions::default()));
        assert!(t.contains("<falloff_angle>"), "the spot's falloff angle is written");
        let written = number_in(&t, "falloff_angle");
        assert!((written - 68.75493814777446).abs() < 1e-9, "0.6 rad from the axis is a 68.75° cone: {written}");
    }

    /// The rule that carries a spot's inner cone in `<falloff_exponent>`,
    /// both ways: [`falloff_exponent`] writes it, [`inner_cone`] reads it.
    /// The spec's spot shines cosᵉ of the angle from its axis (OpenGL's spot
    /// exponent), up to its cone's edge, the outer cone (Ruling R23 makes
    /// `<falloff_angle>` the full cone around it, which leaves this rule as it
    /// was); KHR_lights_punctual's falls from 1 at the inner cone to
    /// 0 at the outer one as the square of `(cos θ − cos outer) / (cos inner
    /// − cos outer)`. The rule: both fall to half at the same angle.
    #[test]
    fn the_falloff_exponent_rule_holds_both_ways() {
        // Worked by hand (Python): staged's cones, 0.2 and 0.6, fall to half
        // where cos θ = cos 0.6 + (cos 0.2 − cos 0.6)/√2 = 0.93474…, and
        // 0.93474…ᵉ = 1/2 at e = 10.271973425033295.
        let e = falloff_exponent(0.2, 0.6);
        assert!((e - 10.271973425033295).abs() < 1e-9, "staged's spot: {e}");
        // The property itself, by bisection on glTF's falloff: where it is
        // 1/2, cosᵉ is 1/2 too.
        let (ci, co) = (f64::from(0.2f32).cos(), f64::from(0.6f32).cos());
        let gltf = |t: f64| ((t.cos() - co) / (ci - co)).clamp(0.0, 1.0).powi(2);
        let (mut lo, mut hi) = (f64::from(0.2f32), f64::from(0.6f32));
        for _ in 0..100 {
            let mid = (lo + hi) / 2.0;
            if gltf(mid) > 0.5 { lo = mid } else { hi = mid }
        }
        assert!((lo.cos().powf(e) - 0.5).abs() < 1e-9, "cosᵉ at glTF's half: {}", lo.cos().powf(e));
        // Each table entry worked by hand: glTF's cones → the exponent → the
        // cones, from a cone of a milliradian to a quarter turn, and from an
        // inner cone of 0 to a hair inside the outer one.
        for (inner, outer, want) in [(0.0f32, 0.6f32, 13.199533940814677), (0.3, 0.31, 14.870338512852566), (0.001, 0.002, 737908.6185745718)] {
            let e = falloff_exponent(inner, outer);
            assert!((e - want).abs() <= 1e-9 * want, "({inner}, {outer}): {e} against {want}");
        }
        for (inner, outer) in [(0.2f32, 0.6f32), (0.0, 0.6), (0.6 * (1.0 - 1e-4), 0.6), (0.0, std::f32::consts::FRAC_PI_2), (1.5, std::f32::consts::FRAC_PI_2), (0.001, 0.002), (0.3, 0.31)] {
            let e = falloff_exponent(inner, outer);
            assert!(e.is_finite() && e > 0.0, "({inner}, {outer}): {e}");
            let back = inner_cone(outer.into(), e);
            assert!((back - f64::from(inner)).abs() < 1e-6, "({inner}, {outer}): e {e} reads back as {back}");
        }
        // The exponent → the cones → the exponent, across the exponents a
        // cone of 0.6 maps to: from just past the hard edge's 3.61 (where cosᵉ
        // is 1/2 at 0.6 itself) to the zero inner cone's 13.2.
        for e in [3.7, 5.0, 10.271973425033295, 13.1] {
            let inner = inner_cone(0.6, e);
            assert!(inner > 0.0 && inner < 0.6, "{e}: {inner}");
            let back = falloff_exponent(inner as f32, 0.6);
            assert!((back - e).abs() <= 1e-5 * e, "{e}: inner {inner} writes back as {back}");
        }
        // Past those ends: the spec's default exponent, 0 (a uniform cone),
        // and any too weak to halve the beam inside the cutoff are a hard
        // edge, which the reader softens; one too strong for the cone is an
        // inner cone of 0.
        assert_eq!(inner_cone(0.6, 0.0), 0.6);
        assert_eq!(inner_cone(0.6, 3.6), 0.6);
        assert_eq!(inner_cone(0.6, 1e6), 0.0);
        assert_eq!(inner_cone(0.6, f64::INFINITY), 0.0);
    }

    /// Collada lights shine, and cameras look, along their node's −Z, with
    /// +Y up ("The light's default direction vector in local coordinates is
    /// [0,0,-1]", `<directional>` p. 5-30; "the lens looks towards the local
    /// -Z axis, and the top of the camera is aligned with the local +Y
    /// axis", `<camera>` p. 5-20), as glTF's do: the boundary turns nothing.
    /// Read: a camera on a node turned a quarter about +Y looks along −X with
    /// +Y up, and a sun turned −90° about X shines straight down. Written:
    /// each node goes out with the scene's own matrix, so staged's lights and
    /// cameras, composed from the written matrices alone, face as in the
    /// scene.
    #[test]
    fn collada_cameras_and_lights_point_along_minus_z_on_the_wire() {
        let (sun, sun_node) = one_light("sun", "<directional><color>1 1 1</color></directional>", "");
        let (eye, eye_node) = one_camera("eye", "<perspective><yfov>45</yfov><znear>0.1</znear><zfar>100</zfar></perspective>", "");
        let nodes = format!(
            "{}{}",
            eye_node.replacen("><instance_camera", "><rotate>0 1 0 90</rotate><instance_camera", 1),
            sun_node.replacen("><instance_light", "><rotate>1 0 0 -90</rotate><instance_light", 1)
        );
        let r = read_doc(&lit_doc(&sun, &eye, &nodes)).unwrap();
        let (lights, cameras) = lightcam::frames(&r.scene, None);
        assert_eq!((lights.len(), cameras.len()), (1, 1), "{:?}", r.warnings);
        assert!(near3(cameras[0].forward, [-1.0, 0.0, 0.0], 1e-12) && near3(cameras[0].up, [0.0, 1.0, 0.0], 1e-12), "{:?}", cameras[0]);
        assert!(near3(lights[0].direction, [0.0, -1.0, 0.0], 1e-12), "{:?}", lights[0]);

        let fixture = fixtures::staged();
        let pkg = package(&fixture, WriteOptions::default());
        let root = super::super::xml::parse(&pkg.files[0].bytes).unwrap();
        let nodes = root.descendants_named("node");
        let matrix = |name: &str| -> Mat4 {
            let n = nodes.iter().find(|n| n.attr("name") == Some(name)).unwrap_or_else(|| panic!("no node {name}"));
            let values = parse_f64_list(&kid(n, "matrix").unwrap().text, name).unwrap();
            matrix_from_row_major(&values).unwrap()
        };
        let (want_lights, want_cameras) = lightcam::frames(&fixture, None);
        let rig = matrix("rig");
        let facing = |name: &str| {
            let m = math::mat_mul(&rig, &matrix(name));
            let l = (m[8] * m[8] + m[9] * m[9] + m[10] * m[10]).sqrt();
            [-m[8] / l, -m[9] / l, -m[10] / l]
        };
        for f in &want_lights {
            assert!(near3(facing(&f.node), f.direction, 1e-12), "{}: {:?} against {:?}", f.node, facing(&f.node), f.direction);
        }
        for f in &want_cameras {
            assert!(near3(facing(&f.node), f.forward, 1e-12), "{}: {:?} against {:?}", f.node, facing(&f.node), f.forward);
        }
        let written: Vec<&str> = root.descendants_named("instance_light").iter().chain(root.descendants_named("instance_camera").iter()).filter_map(|e| e.attr("url")).collect();
        assert_eq!(written.len(), 5, "each light and camera node instances its own: {written:?}");
    }

    /// `body` read as Collada, then written and read back as glTF: the path
    /// of Review Focus 2, through glTF's `pose::bake_shear`.
    fn collada_then_gltf(body: &str) -> (Scene, Scene) {
        let from = read_doc(body).unwrap().scene;
        let w = super::super::write_scene(&from, MeshFormat::Glb, WriteOptions::default()).unwrap();
        let to = super::super::read_scene(&w.package.files[0].bytes, MeshFormat::Glb, &NoSidecars).unwrap().scene;
        (from, to)
    }

    /// A camera `eye` on node `eye_node`, at (0.5, 1, 4) turned 30° about
    /// +Y and −20° about X, under a node whose transform elements are
    /// `parent`.
    fn camera_under(parent: &str) -> String {
        let (eye, _) = one_camera("eye", "<perspective><yfov>40</yfov><znear>0.1</znear><zfar>100</zfar></perspective>", "");
        let nodes = format!(
            r##"<node id="parent" name="parent">{parent}<node id="eye_node" name="eye_node"><translate>0.5 1 4</translate><rotate>0 1 0 30</rotate><rotate>1 0 0 -20</rotate><instance_camera url="#eye"/></node></node>"##
        );
        lit_doc("", &eye, &nodes)
    }

    /// Review Focus 2: a camera under a non-uniformly scaled parent keeps
    /// its world eye and forward from Collada on into glTF, within 1e-5.
    /// glTF's camera ignores its node's scale (glTF 2.0, "Cameras"), so the
    /// frame is the eye and the normalised −Z, as `lightcam` takes it. The
    /// eye and forward the Collada file gives, worked by hand (Python): the
    /// parent T(1, 2, 3)·S(2, 1, 0.5) puts the eye at (2, 3, 5), and its
    /// forward is S·Ry(30°)·Rx(−20°)·(0, 0, −1), normalised.
    #[test]
    fn a_camera_under_a_non_uniform_scale_keeps_its_eye_and_forward() {
        let (from, to) = collada_then_gltf(&camera_under("<translate>1 2 3</translate><scale>2 1 0.5</scale>"));
        let ((_, a), (_, b)) = (lightcam::frames(&from, None), lightcam::frames(&to, None));
        assert_eq!((a.len(), b.len()), (1, 1), "the camera, read from Collada and from glTF");
        assert!(near3(a[0].eye, [2.0, 3.0, 5.0], 1e-12), "{:?}", a[0].eye);
        assert!(near3(a[0].forward, [-0.8703967426371141, -0.3167985063221698, -0.37689284524748345], 1e-12), "{:?}", a[0].forward);
        assert!(near3(b[0].eye, a[0].eye, 1e-5) && near3(b[0].forward, a[0].forward, 1e-5), "{:?} against {:?}", b[0], a[0]);
    }

    /// Review Focus 2 under shear, which a Collada `<matrix>` carries and
    /// glTF's TRS cannot: the camera keeps its world eye and forward through
    /// glTF's `pose::bake_shear`, within 1e-5. The parent's matrix leans its
    /// Z axis half a unit toward +X. `bake_shear` moves the camera onto a
    /// node of its own, framed as the sheared run leaves it (Ruling R19);
    /// before that ruling, the nearest TRS of the parent moved its eye to
    /// (1.5, 3, 7.47).
    #[test]
    fn a_camera_under_shear_keeps_its_eye_and_forward() {
        let (from, to) = collada_then_gltf(&camera_under("<matrix>1 0 0.5 1 0 1 0 2 0 0 1 3 0 0 0 1</matrix>"));
        let ((_, a), (_, b)) = (lightcam::frames(&from, None), lightcam::frames(&to, None));
        assert_eq!((a.len(), b.len()), (1, 1), "the camera, read from Collada and from glTF");
        // Worked by hand (Python): the eye at (0.5 + 0.5·4 + 1, 1 + 2, 4 + 3),
        // looking along the sheared Ry(30°)·Rx(−20°)·(0, 0, −1).
        assert!(near3(a[0].eye, [3.5, 3.0, 7.0], 1e-12), "{:?}", a[0].eye);
        assert!(near3(a[0].forward, [-0.7046901553552409, -0.2749011245072659, -0.6540956785440851], 1e-12), "{:?}", a[0].forward);
        assert!(near3(b[0].eye, a[0].eye, 1e-5), "eye: {:?} against {:?}", b[0].eye, a[0].eye);
        assert!(near3(b[0].forward, a[0].forward, 1e-5), "forward: {:?} against {:?}", b[0].forward, a[0].forward);
    }

    /// Review Focus 6 in Collada: a light or camera that breaks one of
    /// glTF's bounds is dropped with a warning naming it and the bound, the
    /// file still reads, and every node stays: a spot whose litegen extra
    /// puts its cones out of order, a light whose extra gives an infinite
    /// intensity, and a perspective camera with a `<znear>` of 0.
    #[test]
    fn a_bad_light_or_camera_is_dropped_not_refused() {
        let (lamp, lamp_node) = one_light(
            "lamp",
            "<spot><color>1 1 1</color></spot>",
            &litegen_extra("<gltf_color>1 1 1</gltf_color><gltf_intensity>5</gltf_intensity><gltf_inner_cone_angle>0.7</gltf_inner_cone_angle><gltf_outer_cone_angle>0.6</gltf_outer_cone_angle>"),
        );
        let (flare, flare_node) = one_light("flare", "<point><color>1 1 1</color></point>", &litegen_extra("<gltf_color>1 1 1</gltf_color><gltf_intensity>inf</gltf_intensity>"));
        let (shot, shot_node) = one_camera("shot", "<perspective><yfov>40</yfov><znear>0</znear><zfar>10</zfar></perspective>", "");
        let r = read_doc(&lit_doc(&format!("{lamp}{flare}"), &shot, &format!("{lamp_node}{flare_node}{shot_node}"))).unwrap();
        assert_eq!(
            r.warnings,
            [
                r#"light "lamp" is dropped: its inner cone 0.7 is not below its outer cone 0.6"#,
                r#"light "flare" is dropped: its intensity inf is not finite"#,
                r#"camera "shot" is dropped: its znear 0 is not positive"#,
            ]
        );
        assert!(r.scene.lights.is_empty() && r.scene.cameras.is_empty());
        for node in ["lamp_node", "flare_node", "shot_node"] {
            assert!(r.scene.nodes.iter().any(|n| n.name.as_deref() == Some(node) && n.light.is_none() && n.camera.is_none()), "{node} stays, empty");
        }
    }

    /// Ruling R15: an `<ambient>` light, which the scene does not carry (the
    /// spec's non-goals), is dropped with a warning naming it and its
    /// colour, and its node stays.
    #[test]
    fn an_ambient_light_is_dropped_and_named_with_its_colour() {
        let (amb, node) = one_light("amb", "<ambient><color>0.2 0.25 0.5</color></ambient>", "");
        let r = read_doc(&lit_doc(&amb, "", &node)).unwrap();
        assert_eq!(
            r.warnings,
            [r#"light "amb" is dropped: it is an ambient light, colour (0.2, 0.25, 0.5), which the scene does not carry: it holds directional, point and spot lights only"#]
        );
        assert!(r.scene.lights.is_empty());
        assert!(r.scene.nodes.iter().any(|n| n.name.as_deref() == Some("amb_node")), "its node stays");
    }

    /// Review Focus 4 in Collada: a hard-edged spot, here one with the spec's
    /// default `<falloff_exponent>` of 0 (a uniform cone), is softened and
    /// warned once (`Light::soften_hard_edge`), not dropped; written back as
    /// Collada and read again, with or without the litegen extra, it comes
    /// back unchanged and unwarned. Its attenuation is the inverse square,
    /// so that nothing else about it is warned.
    #[test]
    fn a_hard_edged_collada_spot_is_softened_once() {
        let falloff = "<constant_attenuation>0</constant_attenuation><linear_attenuation>0</linear_attenuation><quadratic_attenuation>1</quadratic_attenuation>";
        // A 60° cone, 30° (π/6) from the axis (Ruling R23).
        let (spot, node) = one_light("spot", &format!("<spot><color>1 1 1</color>{falloff}<falloff_angle>60</falloff_angle></spot>"), "");
        let r = read_doc(&lit_doc(&spot, "", &node)).unwrap();
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert!(r.warnings[0].starts_with(r#"light "spot" had a hard edge"#), "{:?}", r.warnings);
        let soft = light_on(&r.scene, "spot_node").cloned().expect("softened, not dropped");
        let LightKind::Spot { inner_cone, outer_cone } = soft.kind else { panic!("{soft:?}") };
        assert!(inner_cone < outer_cone && (f64::from(outer_cone) - std::f64::consts::FRAC_PI_6).abs() < 1e-7, "{soft:?}");
        let again = text(&package(&r.scene, WriteOptions::default()));
        for (label, t) in [("with the extra", again.clone()), ("without it", without_litegen_extras(&again))] {
            let back = read_scene(t.as_bytes(), &NoSidecars).unwrap();
            assert!(back.warnings.is_empty(), "{label}: {:?}", back.warnings);
            let kind = light_on(&back.scene, "spot_node").map(|l| l.kind);
            let Some(LightKind::Spot { inner_cone: i, outer_cone: o }) = kind else { panic!("{label}: {kind:?}") };
            assert!((i - inner_cone).abs() <= 1e-7 && o == outer_cone, "{label}: ({i}, {o}) against ({inner_cone}, {outer_cone})");
        }
    }

    /// Ruling R32 in Collada: a `<falloff_angle>` past 180°, the full cone
    /// (Ruling R23), is a spot wider than a hemisphere. It lights nearly
    /// every way, so it is read as a point light, with one warning naming
    /// it, not dropped. One of exactly 180°, the spec's default, is a
    /// hemisphere and stays a spot.
    #[test]
    fn a_collada_spot_wider_than_a_hemisphere_reads_as_a_point_light() {
        let falloff = "<constant_attenuation>0</constant_attenuation><linear_attenuation>0</linear_attenuation><quadratic_attenuation>1</quadratic_attenuation>";
        let spot = |angle: &str| format!("<spot><color>1 1 1</color>{falloff}<falloff_angle>{angle}</falloff_angle><falloff_exponent>2</falloff_exponent></spot>");
        let (flood, flood_node) = one_light("flood", &spot("200"), "");
        let (edge, edge_node) = one_light("edge", &spot("180"), "");
        let r = read_doc(&lit_doc(&format!("{flood}{edge}"), "", &format!("{flood_node}{edge_node}"))).unwrap();
        assert_eq!(light_on(&r.scene, "flood_node").map(|l| l.kind), Some(LightKind::Point), "{:?}", r.warnings);
        assert!(matches!(light_on(&r.scene, "edge_node").map(|l| l.kind), Some(LightKind::Spot { .. })), "180° stays a spot: {:?}", r.warnings);
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        let w = &r.warnings[0];
        assert!(w.starts_with(r#"light "flood" has a <falloff_angle> of 200°"#) && w.contains("point light"), "{w}");
    }

    /// The litegen extra keeps a light's and a camera's own names
    /// (`<gltf_name>`), which the `name` attribute, an NCName, cannot hold
    /// whole. So "Key Light" and "Main Camera" come back as they were
    /// through litegen, while a reader of the common profile alone sees the
    /// attributes' "Key_Light" and "Main_Camera".
    #[test]
    fn light_and_camera_names_round_trip_through_the_extra() {
        let mut s = fixtures::staged();
        s.lights[0].name = Some("Key Light".into());
        s.cameras[0].name = Some("Main Camera".into());
        let names = |s: &Scene| {
            let mut l: Vec<Option<String>> = s.lights.iter().map(|l| l.name.clone()).collect();
            let mut c: Vec<Option<String>> = s.cameras.iter().map(|c| c.name.clone()).collect();
            l.sort();
            c.sort();
            (l, c)
        };
        let t = text(&package(&s, WriteOptions::default()));
        let back = read_scene(t.as_bytes(), &NoSidecars).unwrap().scene;
        assert_eq!(names(&back), names(&s), "through litegen");
        let bare = read_scene(without_litegen_extras(&t).as_bytes(), &NoSidecars).unwrap().scene;
        let (lights, cameras) = names(&bare);
        assert!(lights.contains(&Some("Key_Light".into())) && cameras.contains(&Some("Main_Camera".into())), "{lights:?} {cameras:?}");
    }

    /// assimp 6.0.5 reads no optics for a `<camera>` that has no `name`
    /// (`ColladaParser.cpp` L1049-L1051: it goes on to the next camera when
    /// the attribute is missing), so an unnamed camera is written with its
    /// id as its name. litegen's extra, with no `<gltf_name>` for it, keeps
    /// it unnamed; a reader of the common profile alone names it by its id.
    #[test]
    fn an_unnamed_camera_is_named_by_its_id() {
        let mut s = fixtures::staged();
        s.cameras[1].name = None;
        let t = text(&package(&s, WriteOptions::default()));
        let library = &t[t.find("<library_cameras>").unwrap()..t.find("</library_cameras>").unwrap()];
        assert!(library.contains(r#"<camera id="camera1-camera" name="camera1-camera">"#), "{library}");
        let back = read_scene(t.as_bytes(), &NoSidecars).unwrap().scene;
        assert_eq!(camera_on(&back, "ortho_camera_node").map(|c| c.name.clone()), Some(None), "unnamed through litegen");
        let bare = read_scene(without_litegen_extras(&t).as_bytes(), &NoSidecars).unwrap().scene;
        assert_eq!(camera_on(&bare, "ortho_camera_node").map(|c| c.name.clone()), Some(Some("camera1-camera".into())));
    }

    /// An `<instance_light>` or `<instance_camera>` whose url finds no light
    /// or camera in the document is warned, and its node is kept: one
    /// naming nothing, one naming a geometry, and one in another document.
    #[test]
    fn an_instance_naming_no_light_or_camera_keeps_its_node() {
        let nodes = r##"<node id="a" name="a"><instance_light url="#nowhere"/></node><node id="b" name="b"><instance_camera url="#g"/></node><node id="c" name="c"><instance_light url="lights.dae#sun"/></node>"##;
        let r = read_doc(&lit_doc("", "", nodes)).unwrap();
        assert_eq!(r.warnings.len(), 3, "{:?}", r.warnings);
        for (node, url) in [("a", r##""#nowhere""##), ("b", r##""#g""##), ("c", r##""lights.dae#sun""##)] {
            let w = r.warnings.iter().find(|w| w.starts_with(&format!("node \"{node}\" "))).unwrap_or_else(|| panic!("{node}: {:?}", r.warnings));
            assert!(w.contains(url) && w.ends_with("the node is kept without it"), "{w}");
        }
        assert!(r.warnings.iter().any(|w| w.starts_with("node \"b\" ") && w.contains("<geometry>")), "{:?}", r.warnings);
        for n in ["a", "b", "c"] {
            assert!(r.scene.nodes.iter().any(|x| x.name.as_deref() == Some(n)), "{n} stays");
        }
    }

    /// A `<light>` or `<camera>` that no node of the scene instances is
    /// dropped with a warning naming it (Ruling R13, as glTF's reader does).
    #[test]
    fn a_light_or_camera_no_node_instances_is_dropped_and_named() {
        let (spare, _) = one_light("spare", "<point><color>1 1 1</color></point>", "");
        let (blind, _) = one_camera("blind", "<perspective><yfov>40</yfov><znear>0.1</znear><zfar>10</zfar></perspective>", "");
        let r = read_doc(&lit_doc(&spare, &blind, "")).unwrap();
        assert_eq!(
            r.warnings,
            [
                r#"light "spare" is instanced by no node of the scene, so it is dropped"#,
                r#"camera "blind" is instanced by no node of the scene, so it is dropped"#,
            ]
        );
        assert!(r.scene.lights.is_empty() && r.scene.cameras.is_empty());
    }

    /// Every field of view the 1.4.1 `<perspective>` entry allows ("A single
    /// <xfov> element", "A single <yfov> element", "Both an <xfov> and a
    /// <yfov> element", "The <aspect_ratio> element and either <xfov> or
    /// <yfov>", pp. 5-92 to 5-93), in degrees, and the same four for
    /// `<orthographic>` (p. 5-89); each expected value worked by hand
    /// (Python). The aspect ratio is the view's width over its height (p.
    /// 5-93), which for a perspective view is tan(xfov/2) / tan(yfov/2). A
    /// lone xfov, xmag or ymag leaves the other to the viewport, taken here
    /// at 16:9, with a warning. Clipping distances and magnifications are in
    /// the document's `<unit>` ("The distances to the clipping planes are
    /// specified in the current units as defined by <asset>/<unit>", p.
    /// 5-93).
    #[test]
    fn every_legal_perspective_and_orthographic_combination_reads() {
        let persp = |yfov: f64, aspect: Option<f64>| Projection::Perspective { yfov: yfov as f32, aspect: aspect.map(|a| a as f32), znear: 0.1, zfar: Some(100.0) };
        let ortho = |xmag: f64, ymag: f64| Projection::Orthographic { xmag: xmag as f32, ymag: ymag as f32, znear: 0.0, zfar: 20.0 };
        let p = |fov: &str| format!("<perspective>{fov}<znear>0.1</znear><zfar>100</zfar></perspective>");
        let o = |mag: &str| format!("<orthographic>{mag}<znear>0</znear><zfar>20</zfar></orthographic>");
        let cases = [
            ("y", p("<yfov>45</yfov>"), persp(std::f64::consts::FRAC_PI_4, None), false),
            ("ya", p("<yfov>45</yfov><aspect_ratio>1.5</aspect_ratio>"), persp(std::f64::consts::FRAC_PI_4, Some(1.5)), false),
            ("xa", p("<xfov>90</xfov><aspect_ratio>1.5</aspect_ratio>"), persp(1.176005207095135, Some(1.5)), false),
            ("xy", p("<xfov>90</xfov><yfov>60</yfov>"), persp(1.0471975511965976, Some(1.7320508075688776)), false),
            ("x", p("<xfov>90</xfov>"), persp(1.0247789206214755, None), true),
            ("oxy", o("<xmag>3</xmag><ymag>2</ymag>"), ortho(3.0, 2.0), false),
            ("oxa", o("<xmag>3</xmag><aspect_ratio>1.5</aspect_ratio>"), ortho(3.0, 2.0), false),
            ("oya", o("<ymag>2</ymag><aspect_ratio>1.5</aspect_ratio>"), ortho(3.0, 2.0), false),
            ("ox", o("<xmag>3</xmag>"), ortho(3.0, 1.6875), true),
            ("oy", o("<ymag>2</ymag>"), ortho(3.5555555555555554, 2.0), true),
        ];
        let (mut cameras, mut nodes) = (String::new(), String::new());
        for (name, projection, _, _) in &cases {
            let (c, n) = one_camera(name, projection, "");
            cameras.push_str(&c);
            nodes.push_str(&n);
        }
        let r = read_doc(&lit_doc("", &cameras, &nodes)).unwrap();
        assert_eq!(r.scene.cameras.len(), cases.len(), "{:?}", r.warnings);
        let near = |a: f32, b: f32| (a - b).abs() <= 1e-6;
        for (name, _, want, warned) in &cases {
            let got = camera_on(&r.scene, &format!("{name}_node")).map(|c| c.projection);
            let same = match (got, *want) {
                (
                    Some(Projection::Perspective { yfov, aspect, znear, zfar }),
                    Projection::Perspective { yfov: wy, aspect: wa, znear: wn, zfar: wf },
                ) => near(yfov, wy) && aspect.is_some() == wa.is_some() && near(aspect.unwrap_or(0.0), wa.unwrap_or(0.0)) && near(znear, wn) && zfar == wf,
                (Some(Projection::Orthographic { xmag, ymag, znear, zfar }), Projection::Orthographic { xmag: wx, ymag: wy, znear: wn, zfar: wf }) => {
                    near(xmag, wx) && near(ymag, wy) && near(znear, wn) && near(zfar, wf)
                }
                _ => false,
            };
            assert!(same, "{name}: {got:?} against {want:?}");
            let named = r.warnings.iter().filter(|w| w.starts_with(&format!("camera \"{name}\" "))).count();
            assert_eq!(named, usize::from(*warned), "{name}: {:?}", r.warnings);
        }
        let (cm_p, cm_pn) = one_camera("cm_p", "<perspective><yfov>45</yfov><znear>10</znear><zfar>5000</zfar></perspective>", "");
        let (cm_o, cm_on) = one_camera("cm_o", "<orthographic><xmag>300</xmag><ymag>200</ymag><znear>0</znear><zfar>2000</zfar></orthographic>", "");
        let r = read_doc_in("0.01", &lit_doc("", &format!("{cm_p}{cm_o}"), &format!("{cm_pn}{cm_on}"))).unwrap();
        assert_eq!(camera_on(&r.scene, "cm_p_node").map(|c| c.projection), Some(Projection::Perspective { yfov: std::f32::consts::FRAC_PI_4, aspect: None, znear: 0.1, zfar: Some(50.0) }));
        assert_eq!(camera_on(&r.scene, "cm_o_node").map(|c| c.projection), Some(Projection::Orthographic { xmag: 3.0, ymag: 2.0, znear: 0.0, zfar: 20.0 }));
    }

    /// What a camera has that Collada's common profile cannot say: `<zfar>`
    /// is required, so an infinite far plane is written as 1000 m
    /// (`fbx::unset_far`, Blender's default clip end, as FBX writes it) and
    /// reported, and the litegen extra brings `None` back. An unset aspect
    /// ratio is `<yfov>` alone, which the spec allows ("the application can
    /// calculate the camera aspect ratio based on the viewport aspect
    /// ratio", p. 5-92), so it is neither written nor reported.
    #[test]
    fn what_collada_cannot_hold_of_a_camera_is_written_and_reported() {
        let mut s = fixtures::staged();
        let open = Projection::Perspective { yfov: 0.8, aspect: None, znear: 0.125, zfar: None };
        s.cameras[0].projection = open;
        let w = super::super::write_scene(&s, MeshFormat::Dae, WriteOptions::default()).unwrap();
        let cameras: Vec<&Loss> = w.losses.iter().filter(|l| l.kind == LossKind::Cameras).collect();
        assert_eq!(cameras.len(), 1, "{:?}", w.losses);
        assert!(cameras[0].what.contains("the infinite far plane of 1 camera(s) [persp_camera]"), "{:?}", cameras[0]);
        let t = String::from_utf8(w.package.files[0].bytes.clone()).unwrap();
        let at = t.find(r#"name="persp_camera""#);
        assert!(at.is_some(), "persp_camera is written");
        let persp = &t[at.unwrap()..];
        let persp = &persp[..persp.find("</perspective>").unwrap()];
        assert!(persp.contains("<zfar>1000</zfar>") && !persp.contains("aspect_ratio>"), "{persp}");
        let back = read_scene(&w.package.files[0].bytes, &w.package.resolver()).unwrap().scene;
        assert_eq!(camera_on(&back, "persp_camera_node").map(|c| c.projection), Some(open));
        let bare = read_scene(without_litegen_extras(&t).as_bytes(), &NoSidecars).unwrap().scene;
        let held = Projection::Perspective { yfov: 0.8, aspect: None, znear: 0.125, zfar: Some(1000.0) };
        assert_eq!(camera_on(&bare, "persp_camera_node").map(|c| c.projection), Some(held));
    }

    /// Ruling R17: a light's `<color>` is its linear colour times its
    /// intensity, on the wire. That is a linear scale, and it runs far past
    /// 1, where the sRGB curve material colours take would explode. So
    /// staged's sun, (1, 0.875, 0.75) at 3 lux, is written as 3 2.625 2.25;
    /// its bulb, (1, 0.75, 0.5) at 20 cd, as 20 15 10; and its spot,
    /// (0.5, 0.75, 1) at 40 cd, as 20 30 40, as assimp writes glTF's lights
    /// (`glTF2Importer.cpp` L975-L978). three.js decodes them from sRGB, a
    /// listed deviation.
    #[test]
    fn a_lights_color_is_its_linear_colour_times_its_intensity() {
        let t = text(&package(&fixtures::staged(), WriteOptions::default()));
        for (light, want) in [("sun", "3 2.625 2.25"), ("bulb", "20 15 10"), ("spot", "20 30 40")] {
            let at = t.find(&format!("<light id=\"{light}-light\""));
            assert!(at.is_some(), "{light} is written");
            let rest = &t[at.unwrap()..];
            let colour = &rest[rest.find("<color>").unwrap() + "<color>".len()..];
            assert_eq!(&colour[..colour.find('<').unwrap()], want, "{light}");
        }
    }

    /// Without the litegen extra, a light's `<color>`, its linear colour times
    /// its intensity (Ruling R17), splits into the colour and the intensity
    /// by moving its largest component into the intensity (Ruling R8 keeps
    /// the colour in 0 to 1); and a point or spot light's attenuation, the
    /// spec's A = constant + Dist·linear + Dist²·quadratic (p. 5-119), folds
    /// into the intensity as it stands at 1 m, in the document's units. The
    /// scene's inverse-square falloff is the quadratic term alone, read
    /// unwarned; any other falloff is warned. At 1 m, a centimetre
    /// document's Dist is 100, so its quadratic 1 is 10,000.
    #[test]
    fn a_lights_colour_and_attenuation_fold_into_its_intensity() {
        let falloff = |q: &str| format!("<constant_attenuation>0</constant_attenuation><linear_attenuation>0</linear_attenuation><quadratic_attenuation>{q}</quadratic_attenuation>");
        let tint = [1.0, 0.5, 0.25];
        let cases = [
            ("sun", "<directional><color>2 1 0.5</color></directional>".to_string(), "1", 2.0, [1.0, 0.5, 0.25], false),
            ("quad", format!("<point><color>1 0.5 0.25</color>{}</point>", falloff("4")), "1", 0.25, tint, false),
            ("plain", "<point><color>1 0.5 0.25</color></point>".to_string(), "1", 1.0, tint, true),
            ("cm", format!("<point><color>1 0.5 0.25</color>{}</point>", falloff("1")), "0.01", 1e-4, tint, false),
        ];
        for (name, shape, meter, intensity, colour, warned) in cases {
            let (l, n) = one_light(name, &shape, "");
            let r = read_doc_in(meter, &lit_doc(&l, "", &n)).unwrap();
            let got = light_on(&r.scene, &format!("{name}_node"));
            assert!(got.is_some(), "{name}: {:?}", r.warnings);
            let got = got.unwrap();
            assert!(((f64::from(got.intensity) - intensity) / intensity).abs() < 1e-6, "{name}: intensity {} against {intensity}", got.intensity);
            assert!((0..3).all(|k| (f64::from(got.color[k]) - colour[k]).abs() < 1e-6), "{name}: colour {:?} against {colour:?}", got.color);
            assert_eq!(got.range, None, "{name}: Collada has no range");
            let about = r.warnings.iter().filter(|w| w.starts_with(&format!("light \"{name}\" ")) && w.contains("attenuation")).count();
            assert_eq!(about, usize::from(warned), "{name}: {:?}", r.warnings);
        }
    }

    /// A litegen extra that does not read (a colour of two numbers) is
    /// warned and passed over: the light reads from its common technique,
    /// by the documented mapping.
    #[test]
    fn a_litegen_extra_that_does_not_read_falls_back_to_the_common_technique() {
        let (l, n) = one_light("lamp", "<directional><color>2 1 0.5</color></directional>", &litegen_extra("<gltf_color>1 1</gltf_color><gltf_intensity>3</gltf_intensity>"));
        let r = read_doc(&lit_doc(&l, "", &n)).unwrap();
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert!(r.warnings[0].starts_with(r#"light "lamp" "#) && r.warnings[0].contains("litegen") && r.warnings[0].contains("gltf_color"), "{:?}", r.warnings);
        let got = light_on(&r.scene, "lamp_node").expect("read from the common technique");
        assert!((got.intensity - 2.0).abs() < 1e-6 && got.color == [1.0, 0.5, 0.25], "{got:?}");
    }

    /// `staged()`'s lights and cameras, lean, for the sweep: each on a root
    /// node at its place in staged, translated only, and one bare triangle
    /// of the ground, which the file needs to read at all. No rig, clip,
    /// cube, normals or materials: the paths that read those have sweeps of
    /// their own, and a sweep reads its file nine times per byte, plus
    /// 10,000 (Ruling A40a's lesson, and Task 4's ~47 s FBX sweep).
    fn lean_staged() -> Scene {
        let full = fixtures::staged();
        let mut ground = full.meshes[0].primitives[0].clone();
        ground.positions.truncate(3);
        ground.normals = None;
        ground.indices = vec![0, 1, 2];
        ground.material = None;
        let mut s = Scene { name: full.name.clone(), lights: full.lights.clone(), cameras: full.cameras.clone(), ..Default::default() };
        s.meshes.push(SceneMesh { primitives: vec![ground], ..Default::default() });
        s.nodes.push(Node { name: Some("ground_node".into()), mesh: Some(0), ..Default::default() });
        for n in &full.nodes[3..8] {
            let translation = n.transform.trs().expect("staged's nodes are TRS").0;
            s.nodes.push(Node { transform: Transform::Trs { translation, rotation: math::QUAT_IDENTITY, scale: [1.0; 3] }, children: Vec::new(), ..n.clone() });
        }
        s.roots = (0..6).collect();
        s
    }

    /// Lean staged as Collada (a single file, [`lean_staged`]), through the
    /// reader under `layer_sweep`: cut at every length, each bit of its
    /// first 4 KiB and of everything from `<library_cameras>` to the end of
    /// the visual scene flipped in turn, and 2–8 bits flipped at random in
    /// 10,000 seeded cases. Every reading returns.
    #[test]
    fn collada_light_and_camera_inputs_survive_truncation_and_bit_flips() {
        let file = package(&lean_staged(), WriteOptions { self_contained: true, ..Default::default() }).files[0].bytes.clone();
        let whole = read_scene(&file, &NoSidecars).unwrap_or_else(|e| panic!("lean staged as Collada: {e}"));
        assert_eq!((whole.scene.lights.len(), whole.scene.cameras.len()), (3, 2), "lean staged as Collada carries every light and camera");
        let layers = span("lean staged as Collada", &file, "<library_cameras>", "</library_visual_scenes>");
        layer_sweep("lean staged as Collada", &file, layers, |bytes| {
            let _ = read_scene(bytes, &NoSidecars);
        });
    }

    /// The same sweep over lean staged with its litegen extras stripped
    /// ([`without_litegen_extras`]). The reader then takes every light and
    /// camera from its common technique ([`read_light`], [`read_camera`]),
    /// which the sweep above never reaches: there, every extra that reads
    /// returns first. Each bit of the light and camera elements, from
    /// `<library_cameras>` to the end of `<library_lights>`, is flipped in
    /// turn among the single flips. Every reading returns.
    #[test]
    fn collada_lights_and_cameras_without_the_extra_survive_truncation_and_bit_flips() {
        let name = "lean staged as Collada, without its extras";
        let full = String::from_utf8(package(&lean_staged(), WriteOptions { self_contained: true, ..Default::default() }).files[0].bytes.clone()).unwrap();
        let file = without_litegen_extras(&full).into_bytes();
        assert!(!String::from_utf8_lossy(&file).contains(r#"profile="litegen""#), "{name}: every extra stripped");
        let whole = read_scene(&file, &NoSidecars).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((whole.scene.lights.len(), whole.scene.cameras.len()), (3, 2), "{name} carries every light and camera");
        let layers = span(name, &file, "<library_cameras>", "</library_lights>");
        layer_sweep(name, &file, layers, |bytes| {
            let _ = read_scene(bytes, &NoSidecars);
        });
    }

    /// assimp 6.0's Collada of litegen's GLB of a still `staged()`
    /// (`testdata/dae/assimp/staged_assimp.dae`; its README gives the
    /// commands), read under assimp's own conventions, each from assimp
    /// v6.0.5's source and each value worked by hand (Python):
    /// - `<color>` is the colour times the intensity, linear
    ///   (`glTF2Importer.cpp` L975-L978, written out unchanged by
    ///   `ColladaExporter.cpp` L513-L579), as this reader takes it (Ruling
    ///   R17). staged's colours each peak at 1, so the split gives back each
    ///   light's own colour and intensity.
    /// - The attenuation is 0, 0, 1, glTF's inverse square
    ///   (`glTF2Importer.cpp` L980-L996), read unwarned.
    /// - `<falloff_angle>` holds the inner cone, glTF's half-angle, in
    ///   degrees (`glTF2Importer.cpp` L999-L1000, `ColladaExporter.cpp`
    ///   L563-L567), and `<falloff_exponent>` 28.0020934 encodes the outer
    ///   cone by assimp's own guess (L568-L575; its importer says "I don't
    ///   know how to interpret it", `ColladaLoader.cpp` L365). This reader
    ///   takes `<falloff_angle>` as the full cone (Ruling R23), so the
    ///   spot's outer cone is 0.1, half the fixture's inner. cos(0.1)^28
    ///   stays above 1/2 inside that cone, so by [`inner_cone`] the spot is
    ///   a hard edge, softened and warned.
    /// - The orthographic camera is written as a perspective one with an
    ///   `<xfov>` of 0 (L430-L438: "assimp doesn't support the import of
    ///   orthographic cameras"). It reads as a yfov of 0, so it is dropped
    ///   and named, and its node is kept.
    /// - The perspective camera's `<xfov>` is 2·atan(aspect·tan(yfov/2)) in
    ///   degrees (`glTF2Importer.cpp` L925-L926, `ColladaExporter.cpp`
    ///   L434-L438), with `<aspect_ratio>` 1.5, the view's width over its
    ///   height. It reads back as the fixture's yfov of 0.8.
    /// - No light has a range: glTF's goes to node metadata
    ///   (`glTF2Importer.cpp` L1261-L1263).
    ///
    /// Every light and camera stands and faces as the fixture's do at rest,
    /// within the f32 that assimp writes each node's matrix in.
    #[test]
    fn reads_assimps_lights_and_cameras_under_its_own_conventions() {
        let r = testdata(include_bytes!("testdata/dae/assimp/staged_assimp.dae"));
        let s = &r.scene;
        assert_eq!((s.lights.len(), s.cameras.len()), (3, 1), "three lights, and the perspective camera alone: {:?}", r.warnings);
        let about: Vec<&String> = r.warnings.iter().filter(|w| w.contains("light") || w.contains("camera")).collect();
        assert_eq!(about.len(), 2, "{about:?}");
        assert!(about.iter().any(|w| w.as_str() == r#"camera "ortho_camera_node" is dropped: its yfov 0 is not between 0 and π"#), "{about:?}");
        assert!(about.iter().any(|w| w.starts_with(r#"light "spot_node" had a hard edge"#)), "{about:?}");
        assert!(s.nodes.iter().any(|n| n.name.as_deref() == Some("ortho_camera_node") && n.camera.is_none()), "its node stays");
        let near = |a: f32, b: f64, eps: f64| (f64::from(a) - b).abs() <= eps;
        for (node, intensity, colour) in [
            ("sun_node", 3.0, [1.0, 0.875, 0.75]),
            ("bulb_node", 20.0, [1.0, 0.75, 0.5]),
            ("spot_node", 40.0, [0.5, 0.75, 1.0]),
        ] {
            let l = light_on(s, node).unwrap_or_else(|| panic!("no light on {node}"));
            assert!(near(l.intensity, intensity, 1e-6 * intensity), "{node}: intensity {} against {intensity}", l.intensity);
            assert!((0..3).all(|k| near(l.color[k], colour[k], 1e-6)), "{node}: colour {:?} against {colour:?}", l.color);
            assert_eq!(l.range, None, "{node}");
        }
        assert_eq!(light_on(s, "sun_node").map(|l| l.kind), Some(LightKind::Directional));
        assert_eq!(light_on(s, "bulb_node").map(|l| l.kind), Some(LightKind::Point));
        let spot = light_on(s, "spot_node").map(|l| l.kind);
        let Some(LightKind::Spot { inner_cone, outer_cone }) = spot else { panic!("{spot:?}") };
        assert!(near(outer_cone, 0.1, 1e-7) && near(inner_cone, 0.1 * (1.0 - 1e-4), 1e-7), "({inner_cone}, {outer_cone})");
        let persp = camera_on(s, "persp_camera_node").map(|c| c.projection);
        let Some(Projection::Perspective { yfov, aspect, znear, zfar }) = persp else { panic!("{persp:?}") };
        assert!(near(yfov, 0.8, 1e-6) && aspect == Some(1.5) && znear == 0.125 && zfar == Some(64.0), "{persp:?}");
        let mut still = fixtures::staged();
        still.animations.clear();
        let ((want_lights, want_cameras), (got_lights, got_cameras)) = (lightcam::frames(&still, None), lightcam::frames(s, None));
        for w in &want_lights {
            let g = got_lights.iter().find(|g| g.node == w.node).unwrap_or_else(|| panic!("no light on {}", w.node));
            assert!(near3(g.position, w.position, 1e-5) && near3(g.direction, w.direction, 1e-5), "{}: {g:?} against {w:?}", w.node);
        }
        let w = want_cameras.iter().find(|c| c.node == "persp_camera_node").unwrap();
        assert!(near3(got_cameras[0].eye, w.eye, 1e-5) && near3(got_cameras[0].forward, w.forward, 1e-5) && near3(got_cameras[0].up, w.up, 1e-5), "{:?} against {w:?}", got_cameras[0]);
    }
}
