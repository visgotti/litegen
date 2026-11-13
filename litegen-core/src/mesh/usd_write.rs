//! The scene, written as one USDA layer.
//!
//! # Shape of the output, and why
//!
//! ```text
//! def SkelRoot "<scene>"          (Xform when nothing is skinned or morphed)
//!     def Scope "Materials"       UsdPreviewSurface networks
//!     def Xform / Mesh / Points / BasisCurves "<node>"
//!                                 one prim per node, transforms as xformOps
//!         def Mesh "<node>_<k>"   one Mesh (or Points, BasisCurves) per primitive
//!                                 of a multi-part mesh
//!         def Skeleton "<skin>"   at the deepest non-joint ancestor of its joints
//!             def SkelAnimation   joint and blend-shape animation (one a clip)
//!         def Mesh                skinned meshes beside their skeleton
//!     def DistantLight / SphereLight / Camera "<node>"   a node's light or camera
//!     def Skeleton "MorphTargets" joint-less, drives unskinned blend shapes
//!     variantSet "Animation"      which SkelAnimation each skeleton binds (clips)
//! ```
//!
//! - **A node's light or camera is its prim**, a `DistantLight`, a
//!   `SphereLight` (with `ShapingAPI` for a spot) or a `Camera`, which holds
//!   the node's transform and children as an Xform would. UsdLux lights and
//!   UsdGeomCamera cameras face −Z with +Y up, as the scene's do, so nothing
//!   turns. One that shares its node with a mesh, a skeleton joint (USD's
//!   joints are no prims) or the other gets a child node of its own first
//!   (`fbx::separate_lights_and_cameras`, which `usd::prepare` applies too).
//!   The units are the schemas': a light's intensity is photometric
//!   ([`SPHERE_INTENSITY_PER_CANDELA`], Ruling R6), unless
//!   `WriteOptions::usd_light_units` is `LightUnits::Blender`, which writes
//!   the intensity Blender's own USD exporter gives the light's `energy`
//!   there; and a camera's lens and
//!   film are in tenths of a metre ([`FILM_BACK_HEIGHT`],
//!   [`APERTURE_PER_MAG`]). A prim's `customData` keeps the scene's own
//!   values in a `litegen` dictionary (`usd_read::LITEGEN_KEY`), named
//!   after glTF's properties, for an exact round trip; other readers see
//!   the schema attributes, and `usd::losses` names what those cannot say.
//!   Its numbers are declared `double`/`double3`, whatever precision the
//!   scene holds: a parsed layer's dictionary value cannot say float from
//!   double, so a crate written from it holds doubles, and the text says
//!   the same, which keeps `usdcat` of either one identical (Ruling U15).
//!   Readers take them as f64.
//!
//! - **One Mesh per primitive**, not GeomSubsets: three.js's loader (the
//!   dashboard's viewer) drops every face no subset covers, and per-part Meshes
//!   need no partition bookkeeping. A node with one primitive and no children
//!   IS its Mesh prim, so a typical vendor model round-trips node for node.
//! - **Points and lines are `Points` and `BasisCurves` prims**, not Meshes
//!   with no faces (UsdGeomPoints and UsdGeomBasisCurves, `OpenUSD@v26.11.1
//!   pxr/usd/usdGeom/schema.usda` L1877 and L1594-1812). A points primitive is
//!   one `Points` prim with its `points`. A lines primitive is one `BasisCurves`
//!   of `type = "linear"` and `wrap = "nonperiodic"` with a 2-vertex curve for
//!   each segment (Ruling I10-R8, item 10's ledger): a USD curve has no
//!   vertex shared between segments, so `usd::prepare` and the writer make each
//!   index a vertex of its own ([`deindexed`]), and the reader gives that
//!   back vertex for vertex. A curve's normals would turn it into a ribbon
//!   and are not written (a point keeps its own). Both carry `widths`, one
//!   constant for the layer: 0.25 % of the diagonal of the scene's bounds at
//!   rest, a centimetre where there is no diagonal (Ruling I10-R9,
//!   [`point_width`]), and the prim's `extent` is its positions padded by
//!   half of it on every side. The primvars (`normals`, `displayColor`,
//!   `st`, joint weights) are `vertex`, one value per point or curve vertex
//!   (Ruling I10-R7 for what the reader takes of each interpolation).
//! - **One SkelRoot, the default prim.** UsdSkel only skins and morphs inside a
//!   SkelRoot. Unskinned meshes with morph targets still need a skeleton to
//!   carry their weights, so they bind to one joint-less Skeleton whose
//!   SkelAnimation holds every such weight; blend shapes on an unskinned mesh
//!   apply in its own space and keep its transform (verified with usdrecord).
//! - **Clips are Apple's variant set** (`variant_combiner.py`; `UsdClips::All`,
//!   the default of a USDA and a USDC, while a USDZ writes its first clip alone
//!   unless asked, [`super::USDZ_DEFAULT_CLIPS`]). Each clip is one
//!   SkelAnimation under every skeleton it animates, named for the clip, its own
//!   name and its end (`customData.litegen.clipEnd`, seconds) kept in
//!   `customData`; the SkelAnimations sit outside the variants. The default
//!   prim has the variant set `Animation`, a variant for each clip, the first
//!   selected, and each variant authors `skel:animationSource` on every
//!   skeleton through `over`s and nothing else: nothing a variant varies is
//!   authored outside the variant set, since a local opinion beats a variant's.
//!   A clip that moves none of a skeleton's joints lists none (the skeleton is at
//!   rest), and its blend-shape weights are written whenever the skeleton has
//!   blend shapes. A transform op has one value, so rigid motion is the first
//!   clip's alone, authored outside the variants as in a one-clip file, and
//!   `usd::losses` names the other clips' rigid motion. A scene with nothing
//!   but rigid motion has nothing to switch, and writes its first clip as ever.
//!   `endTimeCode` is the longest clip's end. At most [`MAX_CLIPS`], the
//!   reader's bound. One clip is written exactly as before there were clips.
//! - **Skinned meshes sit beside their skeleton**, both untransformed. USD
//!   ignores a skinned mesh's own transform; three.js does not (it parents the
//!   bones under the mesh), so keeping the two in the same space is what makes
//!   both readers agree.
//! - **Every prim and joint name is unique in the file.** three.js binds
//!   animation tracks by bare name, depth-first, so a joint and a prim sharing
//!   a name animate whichever it meets first.
//! - **Types are UsdSkel's exact ones** (`float3[]` translations, `quatf[]`
//!   rotations, `half3[]` scales): any other type is silently ignored by USD.
//! - **Colours are linear**, as USD's colour attributes are. Vertex colours
//!   reach the surface through a `UsdPrimvarReader_float3` on `displayColor`
//!   (a mesh's material is what USD draws, not its primvars), with the
//!   material's base colour multiplied in, since UsdPreviewSurface has no
//!   multiply node.
//! - **Unlit is a recipe and a marker.** UsdPreviewSurface has no unlit
//!   switch, so glTF's `KHR_materials_unlit` is written as every converter
//!   writes it, google/usd_from_gltf's emission recipe
//!   (`google/usd_from_gltf@6d288cc convert/materializer.cc` L91-93 and
//!   L458-560, `README.md` L99-100, as research Q2 cites them): `diffuseColor` black,
//!   `metallic` 1, `roughness` 1, and `emissiveColor` the base colour, the
//!   base-colour texture's `outputs:rgb` (sRGB) with `scale` the factor, or the
//!   vertex colour reader where the primitive has vertex colours; opacity is
//!   wired as for a lit material. A metal with a black base has a black F0, so
//!   no specular remains: Storm drew the pixel equal to the linear base colour
//!   at 0°, 70° and 85°, where `metallic` 0 left 1 to 2 levels of specular
//!   (`docs/superpowers/research/research-consumers.md`, Q2, measured with
//!   usdrecord). Nothing else a lit material has is written
//!   ([`unlit_surface`]), and the Material carries
//!   `customData.litegen.unlit` (`usd_read::UNLIT_KEY`), the one thing that
//!   reads it back as unlit, its base colour and texture from the emission:
//!   no standard marker exists, and the recipe alone (a black diffuse with an
//!   emission) is a real design, so it reads as the lit emissive material it
//!   is. There is no MaterialX output: RealityKit's
//!   `ND_realitykit_unlit_surfaceshader` is exact in RealityKit, but Storm
//!   draws it white and prefers `outputs:mtlx:surface` where both exist, and
//!   three.js ignores it. Known device risk, unmeasured: usd_from_gltf writes
//!   the emissive texture linear because Quick Look treated it so
//!   (`google/usd_from_gltf@6d288cc common/config.h` L207-210,
//!   `emissive_is_linear = true`, as research Q2 cites it); this one is
//!   sRGB, like the base colour it stands for, so an unlit textured USDZ may
//!   look brighter in Quick Look than in Storm until a device check settles it.
//! - **Time** is written at 30 time codes per second: three.js's USDA parser
//!   ignores `timeCodesPerSecond` and assumes 30, and every other reader honours
//!   the metadata, so 30 is the one rate everyone plays at the right speed.
//!
//! "Ruling R6" is mesh-gaps item 5's (lights and cameras), listed with its
//! cost in the "Rulings" section of
//! `docs/superpowers/plans/2026-11-04-mesh-lights-cameras.md`.

use std::collections::HashMap;
use std::fmt::Write as _;

use super::math::{self, Mat4};
use super::package;
use super::scene::{
    blender_energy, AlphaMode, AnimPath, Camera, Channel, Light, LightKind, Material, MorphTarget, Primitive, Projection, Scene, SkinWeights, TextureRef,
    Topology, Transform, Wrap, MAX_SCENE_VERTICES,
};
use super::usda::{fmt_f32, fmt_f64, quote};
use super::{LightUnits, MeshError, UsdClips};

/// Time codes per second of every layer this writer produces.
pub const TIME_CODES_PER_SECOND: f64 = 30.0;
/// Indentation stops growing past this depth: a hostile hierarchy thousands
/// of nodes deep would otherwise make the text quadratic in its depth.
const MAX_INDENT: usize = 48;
/// Bytes of joint paths a layer may name. UsdSkel names each joint by its
/// path from the skeleton root, so a chain's tokens grow with its depth
/// squared: an 8k-joint chain (1 MB as GLB) became a 329 MB USDA. A 200-joint
/// humanoid needs tens of KB.
const MAX_JOINT_PATH_BYTES: usize = 16 * 1024 * 1024;

fn malformed(detail: impl Into<String>) -> MeshError {
    MeshError::Malformed(format!("usd: {}", detail.into()))
}

/// A texture file the layer references, relative to it.
pub struct TextureFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// Prim and joint names, unique across the whole layer.
#[derive(Default)]
struct Names {
    used: std::collections::HashSet<String>,
    /// Where each base's numbering resumes, so repeats cost one probe each
    /// rather than a count up from `_1`.
    next: HashMap<String, usize>,
}

impl Names {
    fn take(&mut self, wanted: &str, fallback: &str) -> String {
        let base = identifier(wanted, fallback);
        let mut name = base.clone();
        let mut n = self.next.get(&base).copied().unwrap_or(1);
        while !self.used.insert(name.clone()) {
            name = format!("{base}_{n}");
            n += 1;
        }
        self.next.insert(base, n);
        name
    }
}

/// A valid USD prim name from anything: identifier characters kept, others
/// replaced, a leading digit prefixed. ASCII only, for older readers and
/// three.js's `\w`-based parser.
pub fn identifier(name: &str, fallback: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .take(96)
        .collect();
    if s.chars().all(|c| c == '_') {
        s = fallback.to_string();
    }
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

struct Out {
    text: String,
    depth: usize,
}

impl Out {
    fn line(&mut self, s: &str) {
        for _ in 0..self.depth.min(MAX_INDENT) {
            self.text.push_str("    ");
        }
        self.text.push_str(s);
        self.text.push('\n');
    }

    fn open(&mut self, header: &str, meta: &[String]) {
        if meta.is_empty() {
            self.line(header);
        } else {
            self.line(&format!("{header} ("));
            self.depth += 1;
            for m in meta {
                self.line(m);
            }
            self.depth -= 1;
            self.line(")");
        }
        self.line("{");
        self.depth += 1;
    }

    fn close(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        self.line("}");
    }
}

fn v3(v: [f32; 3]) -> String {
    format!("({}, {}, {})", fmt_f32(v[0]), fmt_f32(v[1]), fmt_f32(v[2]))
}

fn v2(v: [f32; 2]) -> String {
    format!("({}, {})", fmt_f32(v[0]), fmt_f32(v[1]))
}

/// A quaternion `[x, y, z, w]` in USDA's text order `(w, x, y, z)`.
fn quat(q: [f32; 4]) -> String {
    format!("({}, {}, {}, {})", fmt_f32(q[3]), fmt_f32(q[0]), fmt_f32(q[1]), fmt_f32(q[2]))
}

/// glTF's column-major 16 values are USD's four rows, in order.
fn matrix(m: &Mat4) -> String {
    let row = |r: usize| {
        format!(
            "({}, {}, {}, {})",
            fmt_f64(m[r * 4]),
            fmt_f64(m[r * 4 + 1]),
            fmt_f64(m[r * 4 + 2]),
            fmt_f64(m[r * 4 + 3])
        )
    };
    format!("( {}, {}, {}, {} )", row(0), row(1), row(2), row(3))
}

fn list<T>(items: &[T], f: impl Fn(&T) -> String) -> String {
    let mut s = String::with_capacity(items.len() * 8 + 2);
    s.push('[');
    for (i, it) in items.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&f(it));
    }
    s.push(']');
    s
}

/// The time code for `t` seconds: the shortest decimal that reads back to
/// exactly the same `f32` seconds.
pub fn time_code(t: f32) -> String {
    let code = t as f64 * TIME_CODES_PER_SECOND;
    for digits in 0..=9 {
        let s = format!("{code:.digits$}");
        if s.parse::<f64>().is_ok_and(|c| (c / TIME_CODES_PER_SECOND) as f32 == t) {
            return s;
        }
    }
    fmt_f64(code)
}

// ─── skeleton layout ────────────────────────────────────────────────────────

/// How one skin is laid out as a USD Skeleton.
struct SkelLayout {
    skin: usize,
    /// The non-joint node whose prim holds the Skeleton (`None`: the root).
    anchor: Option<usize>,
    /// Scene nodes of the USD joints, parents first.
    joints: Vec<usize>,
    /// USD joint index of each joint of the scene skin.
    skin_to_usd: Vec<usize>,
    /// Joint path tokens.
    paths: Vec<String>,
    rest: Vec<Mat4>,
    bind: Vec<Mat4>,
    /// Skinned mesh nodes bound to this skeleton.
    meshes: Vec<usize>,
}

/// Each node's distance from its root (roots are 0), parents first.
fn node_depths(scene: &Scene, parents: &[Option<usize>]) -> Vec<usize> {
    let mut depth = vec![0usize; scene.nodes.len()];
    for n in scene.topological_order() {
        if let Some(p) = parents[n] {
            depth[n] = depth[p] + 1;
        }
    }
    depth
}

/// The deepest node that is `a` or an ancestor of it and also `b` or an
/// ancestor of it; `None` when they hang from different roots.
fn common_ancestor(parents: &[Option<usize>], depth: &[usize], mut a: usize, mut b: usize) -> Option<usize> {
    while depth[a] > depth[b] {
        a = parents[a]?;
    }
    while depth[b] > depth[a] {
        b = parents[b]?;
    }
    while a != b {
        a = parents[a]?;
        b = parents[b]?;
    }
    Some(a)
}


/// Skins that some mesh node uses: the ones written as USD skeletons. A skin
/// nothing binds carries no geometry, and its joints are written as plain
/// nodes.
pub fn used_skins(scene: &Scene) -> Vec<bool> {
    let mut used = vec![false; scene.skins.len()];
    for n in &scene.nodes {
        if let (Some(s), Some(_)) = (n.skin, n.mesh) {
            if let Some(u) = used.get_mut(s) {
                *u = true;
            }
        }
    }
    used
}

/// Nodes that are joints of a written skeleton.
pub fn skeleton_joints(scene: &Scene) -> Vec<bool> {
    let used = used_skins(scene);
    let mut out = vec![false; scene.nodes.len()];
    for (si, skin) in scene.skins.iter().enumerate() {
        if used[si] {
            for &j in &skin.joints {
                if let Some(o) = out.get_mut(j) {
                    *o = true;
                }
            }
        }
    }
    out
}

/// Clips one file keeps, at most: the reader's own bound on the variants of
/// the `Animation` set ([`super::usd_stage::MAX_ANIMATION_VARIANTS`]), so the
/// writer never writes more clips than the reader reads back.
pub const MAX_CLIPS: usize = super::usd_stage::MAX_ANIMATION_VARIANTS;

/// Whether channel `c` of `scene` moves what a SkelAnimation animates: a
/// joint of a written skeleton (`joints` is [`skeleton_joints`]) or the blend
/// shape weights of a mesh with morph targets. USD switches these between
/// clips by switching the animation a skeleton binds; every other channel is
/// a transform op, and an op has one value.
pub fn is_skeletal(scene: &Scene, joints: &[bool], c: &Channel) -> bool {
    match c.path {
        AnimPath::Weights => scene.nodes.get(c.node).and_then(|n| n.mesh).and_then(|m| scene.meshes.get(m)).is_some_and(|m| m.target_count() > 0),
        _ => joints.get(c.node).copied().unwrap_or(false),
    }
}

/// Whether some clip of `scene` moves a skeleton or a blend shape, so that a
/// file of several clips has something to switch between them.
pub fn has_skeletal_clip(scene: &Scene) -> bool {
    let joints = skeleton_joints(scene);
    scene.animations.iter().any(|a| a.channels.iter().any(|c| is_skeletal(scene, &joints, c)))
}

/// For each written skin, in skin order: the anchor (the deepest common
/// ancestor of its joints that is not a joint, `None` for the root) and the
/// USD skeleton's joints — the skin's own plus every joint on the way from
/// them up to the anchor, so that no animated joint is ever folded into
/// another. Shared by the writer and by `usd::prepare`, which must agree on it.
pub fn skeleton_sets(scene: &Scene) -> Vec<(usize, Option<usize>, Vec<usize>)> {
    let used = used_skins(scene);
    let is_joint = skeleton_joints(scene);
    let parents = scene.parents();
    let depth = node_depths(scene, &parents);
    let mut out = Vec::new();
    for (si, skin) in scene.skins.iter().enumerate() {
        if !used[si] || skin.joints.is_empty() {
            continue;
        }
        // The anchor: the joints' deepest common ancestor-or-self, raised past
        // any joint. Found pairwise by depth, never holding a joint's whole
        // root path: one per joint was quadratic memory on a deep chain.
        let mut common = Some(skin.joints[0]);
        for &j in &skin.joints[1..] {
            common = common.and_then(|c| common_ancestor(&parents, &depth, c, j));
        }
        let mut anchor = common;
        while let Some(n) = anchor.filter(|n| is_joint[*n]) {
            anchor = parents[n];
        }
        let mut members = vec![false; scene.nodes.len()];
        let mut walked = vec![false; scene.nodes.len()];
        for &j in &skin.joints {
            members[j] = true;
            let mut cur = parents[j];
            while let Some(a) = cur {
                // Above here was walked from an earlier joint already.
                if Some(a) == anchor || std::mem::replace(&mut walked[a], true) {
                    break;
                }
                if is_joint[a] {
                    members[a] = true;
                }
                cur = parents[a];
            }
        }
        // Parents-first order: a pre-order walk from the anchor.
        let mut order = Vec::new();
        let starts: Vec<usize> = match anchor {
            Some(a) => scene.nodes[a].children.clone(),
            None => scene.roots.clone(),
        };
        let mut stack: Vec<usize> = starts.into_iter().rev().collect();
        let mut steps = 0usize;
        while let Some(n) = stack.pop() {
            steps += 1;
            if steps > scene.nodes.len() + 1 {
                break;
            }
            if members[n] {
                order.push(n);
            }
            stack.extend(scene.nodes[n].children.iter().rev().copied());
        }
        out.push((si, anchor, order));
    }
    out
}

fn skeleton_layouts(scene: &Scene, parents: &[Option<usize>], world: &[Mat4], names: &mut Names) -> Result<Vec<SkelLayout>, MeshError> {
    let mut out = Vec::new();
    let mut path_bytes = 0usize;
    for (si, anchor, order) in skeleton_sets(scene) {
        let skin = &scene.skins[si];
        let meshes: Vec<usize> = (0..scene.nodes.len()).filter(|&n| scene.nodes[n].skin == Some(si) && scene.nodes[n].mesh.is_some()).collect();
        let members: Vec<bool> = {
            let mut m = vec![false; scene.nodes.len()];
            for &n in &order {
                m[n] = true;
            }
            m
        };
        let index_of: HashMap<usize, usize> = order.iter().enumerate().map(|(i, n)| (*n, i)).collect();
        let skin_index: HashMap<usize, usize> = skin.joints.iter().enumerate().map(|(k, j)| (*j, k)).collect();
        let anchor_world = anchor.map_or(math::IDENTITY, |a| world[a]);
        let mut paths = Vec::with_capacity(order.len());
        let mut rest = Vec::with_capacity(order.len());
        let mut bind = Vec::with_capacity(order.len());
        for (i, &n) in order.iter().enumerate() {
            // The nearest member ancestor below the anchor, walked lazily: the
            // nearest is the parent in a chain, and collecting every ancestor
            // per joint made a chain quadratic.
            let parent_joint = {
                let (mut cur, mut steps, mut found) = (parents[n], 0usize, None);
                while let Some(a) = cur {
                    if Some(a) == anchor || steps > parents.len() {
                        break;
                    }
                    if members[a] {
                        found = Some(a);
                        break;
                    }
                    cur = parents[a];
                    steps += 1;
                }
                found
            };
            let name = names.take(scene.nodes[n].name.as_deref().unwrap_or(""), &format!("joint_{i}"));
            let path = match parent_joint {
                Some(p) => format!("{}/{name}", paths[index_of[&p]]),
                None => name,
            };
            path_bytes = path_bytes.saturating_add(path.len());
            if path_bytes > MAX_JOINT_PATH_BYTES {
                return Err(MeshError::Unsupported(format!(
                    "the skeletons' joint paths pass {MAX_JOINT_PATH_BYTES} bytes (UsdSkel names each joint by its whole path, so a deep chain grows with its depth squared)"
                )));
            }
            paths.push(path);
            let base = match parent_joint {
                Some(p) => world[p],
                None => anchor_world,
            };
            let local = if parents[n] == parent_joint.or(anchor) {
                scene.nodes[n].transform.matrix()
            } else {
                math::invert(&base).map(|inv| math::mat_mul(&inv, &world[n])).ok_or_else(|| malformed("a skeleton ancestor has a singular transform"))?
            };
            rest.push(local);
            let b = match skin_index.get(&n).copied() {
                Some(k) => math::invert(&skin.inverse_bind_matrices[k]).ok_or_else(|| malformed(format!("skin {si} has a singular inverse bind matrix")))?,
                None => math::invert(&anchor_world).map(|inv| math::mat_mul(&inv, &world[n])).unwrap_or(math::IDENTITY),
            };
            bind.push(b);
        }
        let skin_to_usd = skin.joints.iter().map(|j| index_of.get(j).copied().unwrap_or(0)).collect();
        out.push(SkelLayout { skin: si, anchor, joints: order, skin_to_usd, paths, rest, bind, meshes });
    }
    Ok(out)
}

// ─── materials ──────────────────────────────────────────────────────────────

/// A material as written: the scene material (or glTF's default) and whether
/// this variant reads vertex colours.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct MatKey {
    material: Option<usize>,
    vertex_color: bool,
}

/// The material a primitive that names none is written bound to, so a file
/// never holds an unbound gprim. [`super::usd::prepare`] binds the same one,
/// so the scene it returns is what the reader reads back.
pub(super) fn default_material() -> Material {
    // glTF's default material: white, fully metallic, fully rough.
    Material { name: Some("DefaultMaterial".into()), ..Default::default() }
}

/// An unlit material as USD holds it: its base colour, base-colour texture,
/// opacity (alpha mode, cutoff) and sidedness, and nothing else. glTF's
/// `KHR_materials_unlit` makes every lighting property inert, and the recipe
/// ([`write_material`], "Unlit") sets `metallic` and `roughness` to 1 and
/// takes the emission from the base colour, so a metallic-roughness map, a
/// normal map, occlusion, an emissive, a clearcoat or an ior, written beside
/// it, would only break it: a clearcoat is a specular lobe of its own. Every
/// other field is its default (metallic 1, roughness 1, which is the recipe's),
/// so [`super::usd::prepare`] and the writer agree on the same material, and
/// the reader reads it back as written.
pub(super) fn unlit_surface(m: &Material) -> Material {
    Material {
        name: m.name.clone(),
        base_color: m.base_color,
        base_color_texture: m.base_color_texture.clone(),
        alpha_mode: m.alpha_mode,
        alpha_cutoff: m.alpha_cutoff,
        double_sided: m.double_sided,
        unlit: true,
        ..Default::default()
    }
}

fn wrap_token(w: Wrap) -> &'static str {
    match w {
        Wrap::Repeat => "repeat",
        Wrap::ClampToEdge => "clamp",
        Wrap::MirroredRepeat => "mirror",
    }
}

#[allow(clippy::too_many_arguments)]
fn write_texture_shader(
    o: &mut Out,
    mat_path: &str,
    shader: &str,
    file: &str,
    r: &TextureRef,
    scene: &Scene,
    raw: bool,
    scale: Option<[f32; 4]>,
    bias: Option<[f32; 4]>,
    outputs: &[(&str, &str)],
) {
    let tex = &scene.textures[r.texture];
    o.open(&format!("def Shader {}", quote(shader)), &[]);
    o.line("uniform token info:id = \"UsdUVTexture\"");
    o.line(&format!("asset inputs:file = @{file}@"));
    let reader = if r.uv_set == 0 { "TexCoordReader".to_string() } else { format!("TexCoordReader{}", r.uv_set) };
    o.line(&format!("float2 inputs:st.connect = <{mat_path}/{reader}.outputs:result>"));
    o.line(&format!("token inputs:sourceColorSpace = \"{}\"", if raw { "raw" } else { "sRGB" }));
    // Always authored: the unauthored `useMetadata` shows black outside
    // [0, 1] in Hydra and RealityKit.
    o.line(&format!("token inputs:wrapS = \"{}\"", wrap_token(tex.sampler.wrap_s)));
    o.line(&format!("token inputs:wrapT = \"{}\"", wrap_token(tex.sampler.wrap_t)));
    if let Some(s) = scale {
        o.line(&format!("float4 inputs:scale = ({}, {}, {}, {})", fmt_f32(s[0]), fmt_f32(s[1]), fmt_f32(s[2]), fmt_f32(s[3])));
    }
    if let Some(b) = bias {
        o.line(&format!("float4 inputs:bias = ({}, {}, {}, {})", fmt_f32(b[0]), fmt_f32(b[1]), fmt_f32(b[2]), fmt_f32(b[3])));
    }
    for (ty, name) in outputs {
        o.line(&format!("{ty} outputs:{name}"));
    }
    o.close();
}

#[allow(clippy::too_many_arguments)]
fn write_material(o: &mut Out, path: &str, name: &str, original: Option<&str>, m: &Material, vc: bool, scene: &Scene, files: &[Option<String>]) {
    // An unlit material is its base colour and opacity (`unlit_surface`); the
    // marker says so to litegen's reader, beside the recipe other readers draw.
    let reduced;
    let m = if m.unlit {
        reduced = unlit_surface(m);
        &reduced
    } else {
        m
    };
    let unlit = m.unlit;
    let mut meta = Vec::new();
    if unlit {
        meta = custom_data(original, &[format!("bool {} = 1", super::usd_read::UNLIT_KEY)]);
    } else if let Some(orig) = original {
        meta.push(format!("customData = {{ string {} = {} }}", super::usd_read::NAME_KEY, quote(orig)));
    }
    o.open(&format!("def Material {}", quote(name)), &meta);
    o.line(&format!("token outputs:surface.connect = <{path}/PreviewSurface.outputs:surface>"));
    let file_of = |r: &TextureRef| -> Option<&str> { files.get(scene.textures.get(r.texture)?.image)?.as_deref() };
    let base_tex = m.base_color_texture.as_ref().filter(|r| file_of(r).is_some() && !vc);
    let mr_tex = m.metallic_roughness_texture.as_ref().filter(|r| file_of(r).is_some());
    let normal_tex = m.normal_texture.as_ref().filter(|r| file_of(r).is_some());
    let occ_tex = m.occlusion_texture.as_ref().filter(|r| file_of(r).is_some());
    let em_tex = m.emissive_texture.as_ref().filter(|r| file_of(r).is_some());
    let coat = m.clearcoat.as_ref();
    let coat_tex = coat.and_then(|c| c.texture.as_ref()).filter(|r| file_of(r).is_some());
    let coat_rough_tex = coat.and_then(|c| c.roughness_texture.as_ref()).filter(|r| file_of(r).is_some());
    let opaque = m.alpha_mode == AlphaMode::Opaque;

    o.open("def Shader \"PreviewSurface\"", &[]);
    o.line("uniform token info:id = \"UsdPreviewSurface\"");
    if unlit {
        // Unlit: no diffuse, no specular (metallic 1 with a black base has a
        // black F0) and the base colour as the emission, below.
        o.line(&format!("color3f inputs:diffuseColor = {}", v3([0.0; 3])));
    } else if vc {
        // The value beside the connection is for readers that cannot evaluate
        // a primvar reader: SceneKit's Model I/O import draws such an input
        // black, unless it has a value of its own, which it then uses. The
        // material's own factor is what a viewer without vertex colours shows.
        // Readers that follow the connection (Storm, this module) ignore it.
        o.line(&format!("color3f inputs:diffuseColor = {}", v3([m.base_color[0], m.base_color[1], m.base_color[2]])));
        o.line(&format!("color3f inputs:diffuseColor.connect = <{path}/VertexColor.outputs:result>"));
    } else if base_tex.is_some() {
        o.line(&format!("color3f inputs:diffuseColor.connect = <{path}/BaseColorTex.outputs:rgb>"));
    } else {
        o.line(&format!("color3f inputs:diffuseColor = {}", v3([m.base_color[0], m.base_color[1], m.base_color[2]])));
    }
    if !opaque {
        if vc {
            o.line(&format!("float inputs:opacity = {}", fmt_f32(m.base_color[3])));
            o.line(&format!("float inputs:opacity.connect = <{path}/VertexOpacity.outputs:result>"));
        } else if base_tex.is_some() {
            o.line(&format!("float inputs:opacity.connect = <{path}/BaseColorTex.outputs:a>"));
        } else {
            o.line(&format!("float inputs:opacity = {}", fmt_f32(m.base_color[3])));
        }
        let threshold = if m.alpha_mode == AlphaMode::Mask { m.alpha_cutoff } else { 0.0 };
        o.line(&format!("float inputs:opacityThreshold = {}", fmt_f32(threshold)));
    }
    if mr_tex.is_some() {
        o.line(&format!("float inputs:metallic.connect = <{path}/MetallicRoughnessTex.outputs:b>"));
        o.line(&format!("float inputs:roughness.connect = <{path}/MetallicRoughnessTex.outputs:g>"));
    } else {
        o.line(&format!("float inputs:metallic = {}", fmt_f32(m.metallic)));
        o.line(&format!("float inputs:roughness = {}", fmt_f32(m.roughness)));
    }
    if normal_tex.is_some() {
        o.line(&format!("normal3f inputs:normal.connect = <{path}/NormalTex.outputs:rgb>"));
    }
    if occ_tex.is_some() {
        o.line(&format!("float inputs:occlusion.connect = <{path}/OcclusionTex.outputs:r>"));
    }
    if unlit {
        // The base colour is the emission, always written: the reader takes the
        // base colour of a marked material from here. A vertex colour (the
        // base colour baked in, as for a lit material) or the base-colour
        // texture is what it connects to.
        let base = v3([m.base_color[0], m.base_color[1], m.base_color[2]]);
        if vc {
            o.line(&format!("color3f inputs:emissiveColor = {base}"));
            o.line(&format!("color3f inputs:emissiveColor.connect = <{path}/VertexColor.outputs:result>"));
        } else if base_tex.is_some() {
            o.line(&format!("color3f inputs:emissiveColor.connect = <{path}/BaseColorTex.outputs:rgb>"));
        } else {
            o.line(&format!("color3f inputs:emissiveColor = {base}"));
        }
    } else if em_tex.is_some() {
        o.line(&format!("color3f inputs:emissiveColor.connect = <{path}/EmissiveTex.outputs:rgb>"));
    } else if m.emissive != [0.0; 3] {
        o.line(&format!("color3f inputs:emissiveColor = {}", v3(m.emissive)));
    }
    // UsdPreviewSurface's second specular lobe: the clearcoat and its
    // roughness, from a map's R and G as glTF packs them. The roughness is
    // always written, USD's default being 0.01 and glTF's 0. The coat's
    // normal is the base normal map (`usd::prepare`), which is written above.
    if let Some(c) = coat {
        if coat_tex.is_some() {
            o.line(&format!("float inputs:clearcoat.connect = <{path}/ClearcoatTex.outputs:r>"));
        } else {
            o.line(&format!("float inputs:clearcoat = {}", fmt_f32(c.factor)));
        }
        if coat_rough_tex.is_some() {
            o.line(&format!("float inputs:clearcoatRoughness.connect = <{path}/ClearcoatRoughnessTex.outputs:g>"));
        } else {
            o.line(&format!("float inputs:clearcoatRoughness = {}", fmt_f32(c.roughness)));
        }
    }
    // The ior, 0 included: UsdPreviewSurface's F0, ((1-ior)/(1+ior))^2, gives
    // it KHR_materials_ior's meaning. USD also takes a clearcoat's
    // reflectance from it, where glTF's clearcoat keeps 1.5: `usd::losses`
    // names such a clearcoat.
    if let Some(ior) = m.ior {
        o.line(&format!("float inputs:ior = {}", fmt_f32(ior)));
    }
    o.line("int inputs:useSpecularWorkflow = 0");
    o.line("token outputs:surface");
    o.close();

    let mut sets: Vec<u32> = [base_tex, mr_tex, normal_tex, occ_tex, em_tex, coat_tex, coat_rough_tex].iter().flatten().map(|r| r.uv_set).collect();
    sets.sort_unstable();
    sets.dedup();
    for set in sets {
        let (shader, var) = if set == 0 { ("TexCoordReader".to_string(), "st".to_string()) } else { (format!("TexCoordReader{set}"), format!("st{set}")) };
        o.open(&format!("def Shader {}", quote(&shader)), &[]);
        o.line("uniform token info:id = \"UsdPrimvarReader_float2\"");
        // `string`, not `token`: the Sdr definition says string and the
        // validator rejects the other.
        o.line(&format!("string inputs:varname = {}", quote(&var)));
        o.line("float2 inputs:fallback = (0, 0)");
        o.line("float2 outputs:result");
        o.close();
    }
    if vc {
        o.open("def Shader \"VertexColor\"", &[]);
        o.line("uniform token info:id = \"UsdPrimvarReader_float3\"");
        o.line("string inputs:varname = \"displayColor\"");
        o.line("float3 inputs:fallback = (1, 1, 1)");
        o.line("float3 outputs:result");
        o.close();
        if !opaque {
            o.open("def Shader \"VertexOpacity\"", &[]);
            o.line("uniform token info:id = \"UsdPrimvarReader_float\"");
            o.line("string inputs:varname = \"displayOpacity\"");
            o.line("float inputs:fallback = 1");
            o.line("float outputs:result");
            o.close();
        }
    }
    let one = [1.0f32; 4];
    if let Some(r) = base_tex {
        // glTF's factor multiplies the texel, which is UsdUVTexture's scale.
        let s = m.base_color;
        let scale = (s != one).then_some(s);
        let file = file_of(r).unwrap_or("");
        write_texture_shader(o, path, "BaseColorTex", file, r, scene, false, scale, None, &[("float3", "rgb"), ("float", "a")]);
    }
    if let Some(r) = mr_tex {
        let s = [1.0, m.roughness, m.metallic, 1.0];
        let file = file_of(r).unwrap_or("");
        write_texture_shader(o, path, "MetallicRoughnessTex", file, r, scene, true, (s != one).then_some(s), None, &[("float", "g"), ("float", "b")]);
    }
    if let Some(r) = normal_tex {
        // glTF's normal scale s applies to x and y: n = (2t − 1)·(s, s, 1).
        let sc = m.normal_scale;
        let file = file_of(r).unwrap_or("");
        write_texture_shader(o, path, "NormalTex", file, r, scene, true, Some([2.0 * sc, 2.0 * sc, 2.0, 1.0]), Some([-sc, -sc, -1.0, 0.0]), &[("float3", "rgb")]);
    }
    if let Some(r) = occ_tex {
        // 1 + strength·(occ − 1) = strength·occ + (1 − strength).
        let st = m.occlusion_strength;
        let (scale, bias) = if st == 1.0 { (None, None) } else { (Some([st, st, st, 1.0]), Some([1.0 - st, 1.0 - st, 1.0 - st, 0.0])) };
        let file = file_of(r).unwrap_or("");
        write_texture_shader(o, path, "OcclusionTex", file, r, scene, true, scale, bias, &[("float", "r")]);
    }
    if let Some(r) = em_tex {
        let e = [m.emissive[0], m.emissive[1], m.emissive[2], 1.0];
        let file = file_of(r).unwrap_or("");
        write_texture_shader(o, path, "EmissiveTex", file, r, scene, false, (e != one).then_some(e), None, &[("float3", "rgb")]);
    }
    // glTF's factors multiply the texels, which is each map's scale.
    if let (Some(c), Some(r)) = (coat, coat_tex) {
        let s = [c.factor, 1.0, 1.0, 1.0];
        let file = file_of(r).unwrap_or("");
        write_texture_shader(o, path, "ClearcoatTex", file, r, scene, true, (s != one).then_some(s), None, &[("float", "r")]);
    }
    if let (Some(c), Some(r)) = (coat, coat_rough_tex) {
        let s = [1.0, c.roughness, 1.0, 1.0];
        let file = file_of(r).unwrap_or("");
        write_texture_shader(o, path, "ClearcoatRoughnessTex", file, r, scene, true, (s != one).then_some(s), None, &[("float", "g")]);
    }
    o.close();
}

// ─── lights and cameras ─────────────────────────────────────────────────────

/// UsdLux `inputs:intensity` per candela of a point or spot light, which is
/// written as a normalised `SphereLight` treated as a point: 4 (Ruling R6).
///
/// glTF gives a point or spot light's luminous intensity in candela
/// (KHR_lights_punctual). UsdLux gives a light's `intensity` as the
/// luminance of its surface, in nits, cd/m² (usdLux/schema.usda L195-L216,
/// OpenUSD v26.11.1), which `normalize` divides by the light's world-space
/// surface area, 4πr² for a sphere (L258-L303). A sphere of luminance L
/// shows a disk of πr² from every side, so its luminous intensity is L·πr²,
/// intensity / 4 once normalised. The radius cancels, so the same holds for
/// the zero-radius sphere `treatAsPoint` lets a renderer take it for
/// (L1218-L1227). Hence intensity = 4 × candela: research Q8's derivation,
/// pinned here from the schema, the spec's authority for USD. A directional
/// light is a normalised `DistantLight` of angle 0, whose intensity is then
/// the illuminance in lux (L334-L337; at angle 0 its `sizeFactor` is 1
/// either way, L306-L318), so glTF's lux go out as they are.
///
/// Blender's exporter agrees with the schema: it writes a P-watt point
/// light as intensity P/π with `normalize` on (usd_writer_light.cc
/// L133-L141 and L171-L174, v5.2.3), which is P/(4π) per steradian,
/// Blender's own radiant intensity. Its watts are radiometric, so its
/// intensities are 683 times smaller than these for the same glTF light
/// (Blender's glTF exporter takes 683 lm/W). Storm, which usdrecord renders
/// with, does not: it lights a normalised sphere as a point light of
/// intensity `intensity` (hdSt/light.cpp L159-L161, v26.11.1), so
/// usdrecord shows these point and spot lights 4 times (2 stops) brighter
/// than glTF means them, a cited deviation. It agrees on the DistantLight
/// because `normalize` is on: off, it scales the intensity by the light's
/// solid angle (L198-L208), which is 0 at angle 0.
///
/// That is the default, `WriteOptions::usd_light_units` =
/// `LightUnits::Photometric`. With `LightUnits::Blender` the intensity is
/// instead what Blender's exporter writes for the `energy` its glTF import
/// gives the light, so the file opens in Blender as bright as the glTF does
/// ([`blender_energy_per_intensity`]): with E = 4π·cd / 683, E / π =
/// 4·cd / 683 for a point or spot light, 683 times less than this; and
/// E / 4 = lux / 2732 for a sun, 2732 times less than the lux written here.
///
/// Blender's importer multiplies a sphere's intensity by π and a distant
/// light's by 4 (usd_reader_light.cc L134-L148), the exact inverse of its
/// exporter. So the default's lights open in Blender 683 times (point,
/// spot) and 2732 times (sun) brighter than the same scene's glTF, and the
/// `Blender` ones open as bright as it: real Blender 5.2.0 LTS imported
/// `staged` written with `Blender` as USDA and USDZ with the `energy` of its
/// glTF import, to f32 (`docs/superpowers/research/research-blender-light-units.md`).
pub const SPHERE_INTENSITY_PER_CANDELA: f32 = 4.0;

/// UsdGeomCamera aperture, in tenths of a scene unit, per scene unit of an
/// orthographic view's half-extent (glTF's `xmag` and `ymag`): 20. GfCamera
/// builds an orthographic window ±aperture/2 wide, times `APERTURE_UNIT`,
/// 0.1 (gf/camera.cpp L24-L26 and L310-L337, OpenUSD v26.11.1): the full
/// width is aperture × 0.1, and half of it aperture / 20.
pub const APERTURE_PER_MAG: f64 = 20.0;

/// The film back every perspective camera is written on, as its
/// `verticalAperture` in tenths of a metre: 0.24, the 24 mm of a 35 mm
/// still frame (36 × 24 mm). The focal length follows from the field of
/// view and the horizontal aperture from the aspect ratio; only the ratios
/// reach a picture, but these are real lengths in the stage's units
/// (UsdGeomCamera's "tenths of a scene unit", usdGeom/schema.usda
/// L2288-L2300), a 28 mm lens and a 36 mm wide frame for staged's camera.
pub const FILM_BACK_HEIGHT: f64 = 0.24;

/// A light's prim type: a `DistantLight` for a directional light, and a
/// `SphereLight` for a point or spot light, a spot with ShapingAPI's cone.
fn light_type(l: &Light) -> &'static str {
    match l.kind {
        LightKind::Directional => "DistantLight",
        LightKind::Point | LightKind::Spot { .. } => "SphereLight",
    }
}

/// The metadata lines of a prim's `customData`: the original name when the
/// prim's name is not it, and the `litegen` dictionary, whose `entries` are
/// litegen's own values, for an exact round trip. It serves a light's or
/// camera's values, a material's unlit marker and a SkelAnimation's `clipEnd`.
fn custom_data(original: Option<&str>, entries: &[String]) -> Vec<String> {
    let mut lines = vec!["customData = {".to_string()];
    if let Some(o) = original {
        lines.push(format!("    string {} = {}", super::usd_read::NAME_KEY, quote(o)));
    }
    lines.push(format!("    dictionary {} = {{", super::usd_read::LITEGEN_KEY));
    lines.extend(entries.iter().map(|e| format!("        {e}")));
    lines.push("    }".to_string());
    lines.push("}".to_string());
    lines
}

/// A light's own values, the entries of its `litegen` dictionary, named
/// after KHR_lights_punctual's properties: its type, its name when it has
/// one, its colour and intensity, a point or spot light's range when it has
/// one (a directional light has none, `usd::prepare`), and a spot's cones.
fn light_entries(l: &Light) -> Vec<String> {
    let ty = match l.kind {
        LightKind::Directional => "directional",
        LightKind::Point => "point",
        LightKind::Spot { .. } => "spot",
    };
    let mut e = vec![format!("token type = \"{ty}\"")];
    if let Some(n) = &l.name {
        e.push(format!("string name = {}", quote(n)));
    }
    e.push(format!("double3 color = {}", v3(l.color)));
    e.push(format!("double intensity = {}", fmt_f32(l.intensity)));
    if let Some(r) = l.range.filter(|_| l.kind != LightKind::Directional) {
        e.push(format!("double range = {}", fmt_f32(r)));
    }
    if let LightKind::Spot { inner_cone, outer_cone } = l.kind {
        e.push(format!("double innerConeAngle = {}", fmt_f32(inner_cone)));
        e.push(format!("double outerConeAngle = {}", fmt_f32(outer_cone)));
    }
    e
}

/// A camera's own values, the entries of its `litegen` dictionary, named
/// after glTF's camera properties: its type, its name when it has one, and
/// its projection. An unset aspect ratio or far plane is left out, so it
/// reads back unset.
fn camera_entries(c: &Camera) -> Vec<String> {
    let perspective = matches!(c.projection, Projection::Perspective { .. });
    let mut e = vec![format!("token type = \"{}\"", if perspective { "perspective" } else { "orthographic" })];
    if let Some(n) = &c.name {
        e.push(format!("string name = {}", quote(n)));
    }
    let mut value = |key: &str, v: f32| e.push(format!("double {key} = {}", fmt_f32(v)));
    match c.projection {
        Projection::Perspective { yfov, aspect, znear, zfar } => {
            value("yfov", yfov);
            if let Some(a) = aspect {
                value("aspectRatio", a);
            }
            value("znear", znear);
            if let Some(f) = zfar {
                value("zfar", f);
            }
        }
        Projection::Orthographic { xmag, ymag, znear, zfar } => {
            value("xmag", xmag);
            value("ymag", ymag);
            value("znear", znear);
            value("zfar", zfar);
        }
    }
    e
}

/// Blender's `energy` per unit of `inputs:intensity`: π for a point or spot
/// light and 4 for a sun. Blender's USD exporter writes a light of `energy`
/// E with `inputs:intensity` = E / π for a point or spot
/// light (a normalised sphere, usd_writer_light.cc L138-L141) and E / 4 for
/// a sun (L134-L137: "Unclear why, but approximately matches Karma"), and its
/// importer multiplies them back, E = `inputs:intensity` × π or × 4
/// (usd_reader_light.cc L142-L146 and L138-L141, Blender v5.2.0 and v5.2.3,
/// both read in Task 1 of the light-units plan and confirmed against Blender
/// 5.2.0 itself). The reader's `Blender` units use this too, so the two
/// sides are one factor and its inverse. `normalize`, `radius`, `angle` and
/// the cone do not enter it: Blender's importer sends them to other fields
/// of its light (L88-L130, L200-L208).
pub(super) fn blender_energy_per_intensity(kind: &LightKind) -> f64 {
    match kind {
        LightKind::Directional => 4.0,
        LightKind::Point | LightKind::Spot { .. } => std::f64::consts::PI,
    }
}

/// A light's UsdLux attributes (Ruling R6 at [`SPHERE_INTENSITY_PER_CANDELA`]):
/// its linear colour and its intensity, normalised; a directional light's
/// parallel rays, angle 0; a point or spot light's point-sized sphere,
/// radius 0, treated as a point (as Blender writes one of radius 0,
/// usd_writer_light.cc L90-L100); and a spot's ShapingAPI cone, whose
/// angle is the outer cone "off the primary axis" in degrees and whose
/// softness 1 − inner/outer leaves the light full out to the inner cone
/// (`𝛳smoothStart = lerp(coneSoftness, 𝛳cutoff, 0)`, usdLux/schema.usda
/// L735-L797).
///
/// `units` chooses the intensity. `Photometric` is the schema's, above. With
/// `Blender` it is what Blender's own USD exporter writes for a Blender light
/// of the `energy` Blender's glTF importer gives this light
/// (`scene::blender_energy`, divided by [`blender_energy_per_intensity`]),
/// so the file opens in Blender as bright as the same scene's glTF does.
/// Both are clamped to `f32::MAX` ("inputs:intensity" is a float); the
/// `Blender` value is below the candela or lux it comes from, so the clamp
/// never bites.
fn write_light(o: &mut Out, l: &Light, units: LightUnits) {
    o.line(&format!("color3f inputs:color = {}", v3(l.color)));
    let intensity = match units {
        LightUnits::Photometric => match l.kind {
            LightKind::Directional => l.intensity,
            // A candela past f32::MAX / 4 writes the largest float, never inf.
            LightKind::Point | LightKind::Spot { .. } => (SPHERE_INTENSITY_PER_CANDELA * l.intensity).min(f32::MAX),
        },
        LightUnits::Blender => (blender_energy(&l.kind, f64::from(l.intensity)) / blender_energy_per_intensity(&l.kind)).min(f64::from(f32::MAX)) as f32,
    };
    o.line(&format!("float inputs:intensity = {}", fmt_f32(intensity)));
    o.line("bool inputs:normalize = 1");
    match l.kind {
        LightKind::Directional => o.line("float inputs:angle = 0"),
        LightKind::Point | LightKind::Spot { .. } => {
            o.line("float inputs:radius = 0");
            o.line("bool treatAsPoint = 1");
        }
    }
    if let LightKind::Spot { inner_cone, outer_cone } = l.kind {
        let outer = f64::from(outer_cone).min(std::f64::consts::FRAC_PI_2);
        o.line(&format!("float inputs:shaping:cone:angle = {}", fmt_f32(outer.to_degrees() as f32)));
        o.line(&format!("float inputs:shaping:cone:softness = {}", fmt_f32((1.0 - f64::from(inner_cone) / outer) as f32)));
    }
}

/// A camera's UsdGeomCamera attributes: its projection; a perspective
/// camera's focal length and apertures on a [`FILM_BACK_HEIGHT`] film back,
/// which give back its yfov as 2·atan(verticalAperture / (2·focalLength))
/// and its aspect ratio as horizontalAperture / verticalAperture
/// (gf/camera.cpp L285-L298), an unset aspect ratio as 16:9
/// (`fbx::UNSET_ASPECT`); an orthographic camera's apertures,
/// [`APERTURE_PER_MAG`] times its half-extents, unsigned (apertures are
/// sizes); and its clipping range in metres, an infinite far plane as
/// `fbx::unset_far`. `usd::losses` names what that sets for other readers;
/// the `litegen` dictionary keeps the scene's own values.
fn write_camera(o: &mut Out, c: &Camera) {
    // A value past f32's range writes the largest float, never inf.
    let float = |v: f64| fmt_f32(v.min(f64::from(f32::MAX)) as f32);
    match c.projection {
        Projection::Perspective { yfov, aspect, znear, zfar } => {
            let focal = FILM_BACK_HEIGHT / (2.0 * (f64::from(yfov) / 2.0).tan());
            let width = FILM_BACK_HEIGHT * f64::from(aspect.unwrap_or(super::fbx::UNSET_ASPECT));
            o.line("token projection = \"perspective\"");
            o.line(&format!("float focalLength = {}", float(focal)));
            o.line(&format!("float horizontalAperture = {}", float(width)));
            o.line(&format!("float verticalAperture = {}", float(FILM_BACK_HEIGHT)));
            let far = zfar.unwrap_or_else(|| super::fbx::unset_far(znear));
            o.line(&format!("float2 clippingRange = ({}, {})", fmt_f32(znear), fmt_f32(far)));
        }
        Projection::Orthographic { xmag, ymag, znear, zfar } => {
            o.line("token projection = \"orthographic\"");
            o.line(&format!("float horizontalAperture = {}", float(APERTURE_PER_MAG * f64::from(xmag.abs()))));
            o.line(&format!("float verticalAperture = {}", float(APERTURE_PER_MAG * f64::from(ymag.abs()))));
            o.line(&format!("float2 clippingRange = ({}, {})", fmt_f32(znear), fmt_f32(zfar)));
        }
    }
}

// ─── animation sampling ─────────────────────────────────────────────────────

fn channel_times(chs: &[&Channel]) -> Vec<f32> {
    let mut t: Vec<f32> = chs.iter().flat_map(|c| c.times.iter().copied()).collect();
    t.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    t.dedup();
    t
}

/// TRS of `node` at `t`: its channels where it has them, its rest otherwise.
/// Translation, rotation (x, y, z, w) and scale.
type Trs = ([f64; 3], [f64; 4], [f64; 3]);

fn trs_at(scene: &Scene, chans: &[&Channel], node: usize, t: f32) -> Trs {
    let (mut tr, mut ro, mut sc) = scene.nodes[node].transform.trs().unwrap_or(([0.0; 3], math::QUAT_IDENTITY, [1.0; 3]));
    for ch in chans.iter().filter(|c| c.node == node) {
        let v = super::pose::sample_channel(ch, t, ch.stride(0));
        match ch.path {
            AnimPath::Translation => tr = [v[0] as f64, v[1] as f64, v[2] as f64],
            AnimPath::Rotation => ro = math::quat_normalize([v[0] as f64, v[1] as f64, v[2] as f64, v[3] as f64]),
            AnimPath::Scale => sc = [v[0] as f64, v[1] as f64, v[2] as f64],
            AnimPath::Weights => {}
        }
    }
    (tr, ro, sc)
}

// ─── the layer ──────────────────────────────────────────────────────────────

/// Where textures go.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TexturePlacement {
    /// Referenced as `textures/<name>`, returned for the caller to package.
    Files,
    /// Not written (the scene was prepared without textures).
    None,
}

/// The width of every point and line this layer writes, in metres: 0.25 % of
/// the diagonal of the scene's bounds at rest (`pose::world_bounds`), so a
/// point or a line shows at any size of scene. A centimetre where there is
/// no diagonal to take it of: no bounds, one point, or a corner or a diagonal
/// that is not a number a width can be (Ruling I10-R9). The layer's unit is
/// the metre (`metersPerUnit = 1`).
fn point_width(bounds: Option<([f64; 3], [f64; 3])>) -> f32 {
    const FALLBACK: f32 = 0.01;
    let Some((lo, hi)) = bounds else { return FALLBACK };
    let diagonal = (0..3).map(|k| (hi[k] - lo[k]).powi(2)).sum::<f64>().sqrt();
    let width = 0.0025 * diagonal;
    if width.is_finite() && width > 0.0 && width <= f64::from(f32::MAX) {
        width as f32
    } else {
        FALLBACK
    }
}

/// `v` at each of `at`, in order.
fn gather<T: Copy>(v: &[T], at: &[usize]) -> Vec<T> {
    at.iter().map(|&i| v[i]).collect()
}

/// A points or lines primitive with each index a vertex of its own: a
/// segment's two ends, and each point, are vertices that nothing else shares
/// (Ruling I10-R8). A USD curve has no vertex shared between segments: one
/// 2-vertex curve is written per segment, and a point is a point. So the
/// primitive is what the reader will give back, vertex for vertex, and every
/// array that is per vertex (positions, normals, tangents, each uv set, the
/// colours, the skin's joints and weights, each morph target's offsets) moves
/// with its vertex. Triangles are returned as they are, and so is a primitive
/// whose indices or arrays do not agree (`Scene::validate` refuses it).
pub(super) fn deindexed(p: &Primitive) -> Primitive {
    let n = p.positions.len();
    let sound = p.indices.iter().all(|&i| (i as usize) < n)
        && p.normals.as_ref().is_none_or(|v| v.len() == n)
        && p.tangents.as_ref().is_none_or(|v| v.len() == n)
        && p.uvs.iter().all(|v| v.len() == n)
        && p.colors.as_ref().is_none_or(|v| v.len() == n)
        && p.skin.as_ref().is_none_or(|s| s.influences > 0 && s.joints.len() == n * s.influences && s.weights.len() == n * s.influences)
        && p.targets.iter().all(|t| t.positions.len() == n && t.normals.as_ref().is_none_or(|v| v.len() == n));
    if p.topology == Topology::Triangles || !sound {
        return p.clone();
    }
    let at: Vec<usize> = p.indices.iter().map(|&i| i as usize).collect();
    Primitive {
        positions: gather(&p.positions, &at),
        normals: p.normals.as_ref().map(|v| gather(v, &at)),
        tangents: p.tangents.as_ref().map(|v| gather(v, &at)),
        uvs: p.uvs.iter().map(|v| gather(v, &at)).collect(),
        colors: p.colors.as_ref().map(|v| gather(v, &at)),
        skin: p.skin.as_ref().map(|s| {
            let k = s.influences;
            let slots: Vec<usize> = at.iter().flat_map(|&i| i * k..(i + 1) * k).collect();
            SkinWeights { influences: k, joints: gather(&s.joints, &slots), weights: gather(&s.weights, &slots) }
        }),
        indices: (0..p.indices.len() as u32).collect(),
        material: p.material,
        targets: p
            .targets
            .iter()
            .map(|t| MorphTarget { positions: gather(&t.positions, &at), normals: t.normals.as_ref().map(|v| gather(v, &at)) })
            .collect(),
        topology: p.topology,
    }
}

/// [`deindexed`] for every points and lines primitive of `scene`: what
/// `usd::prepare` leaves the scene as. Refused first when the scene, with
/// each point and each end of a segment a vertex, would pass the scene's
/// vertex limit (the reader refuses such a file, so `prepare` must not
/// promise it): a point that many indices name is one vertex in the scene and
/// many in the file.
pub(super) fn deindex_scene(scene: &mut Scene) -> Result<(), MeshError> {
    let mut uses = vec![0usize; scene.meshes.len()];
    for m in scene.nodes.iter().filter_map(|n| n.mesh) {
        if let Some(u) = uses.get_mut(m) {
            *u += 1;
        }
    }
    let (mut vertices, mut grew) = (0usize, false);
    for (mesh, &used) in scene.meshes.iter().zip(&uses) {
        let mut per_use = 0usize;
        for p in &mesh.primitives {
            let after = if p.topology == Topology::Triangles { p.positions.len() } else { p.indices.len() };
            grew |= after > p.positions.len();
            per_use = per_use.saturating_add(after);
        }
        vertices = vertices.saturating_add(per_use.saturating_mul(used));
    }
    if grew && vertices > MAX_SCENE_VERTICES {
        return Err(MeshError::Unsupported(format!(
            "with each point and each end of a line a vertex of its own, as USD has them, the scene passes {MAX_SCENE_VERTICES} vertices"
        )));
    }
    for p in scene.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
        // One already a vertex to an index, in order (`prepare` ran, and the
        // writer runs this again), is left as it is: gathering it would only
        // copy every array.
        if p.topology == Topology::Triangles {
            continue;
        }
        let in_order = p.indices.len() == p.positions.len() && p.indices.iter().enumerate().all(|(k, &i)| i as usize == k);
        if !in_order {
            *p = deindexed(p);
        }
    }
    Ok(())
}

/// One clip of a file that holds several (`UsdClips::All`).
struct ClipPlan<'a> {
    /// What the writer writes of it: every channel of the first clip (its
    /// rigid motion stays outside the variants, where a one-clip file has
    /// it), and the skeletal ones of the others.
    channels: Vec<&'a Channel>,
    name: Option<&'a str>,
    /// Its variant of the set `Animation`, unique among the variants.
    variant: String,
    /// Seconds: where the clip ends, recorded on each of its SkelAnimations
    /// as `clipEnd`, since a SkelAnimation holds only its joints' keys.
    end: f32,
}

/// The clips of `scene` as the file holds them, or none when it holds one
/// timeline as ever: the first clip alone is `UsdClips::First`'s, and a
/// scene with nothing but rigid motion beyond its first clip has nothing to
/// switch (`usd::prepare` kept that first clip alone).
fn clip_plan<'a>(scene: &'a Scene, joints: &[bool], clips: UsdClips) -> Vec<ClipPlan<'a>> {
    if clips != UsdClips::All || scene.animations.len() < 2 {
        return Vec::new();
    }
    let mut variants = Names::default();
    let mut plan = Vec::new();
    for (i, a) in scene.animations.iter().enumerate().take(MAX_CLIPS) {
        let channels: Vec<&Channel> = a.channels.iter().filter(|c| i == 0 || is_skeletal(scene, joints, c)).collect();
        if i > 0 && channels.is_empty() {
            continue;
        }
        let end = channels.iter().filter_map(|c| c.times.last().copied()).reduce(f32::max).unwrap_or(0.0);
        let variant = variants.take(a.name.as_deref().unwrap_or(""), "Animation");
        plan.push(ClipPlan { channels, name: a.name.as_deref(), variant, end });
    }
    if plan.len() > 1 {
        plan
    } else {
        Vec::new()
    }
}

/// Write `scene` (already reduced by `usd::prepare`) as USDA text, returning
/// the texture files it references.
/// `fps` is the rate the clip was keyed at, said as `framesPerSecond`; time
/// codes stay 30 per second, the rate three.js assumes (module docs).
/// `light_units` is the units each light's `inputs:intensity` is written in
/// ([`write_light`]); its `customData` is the scene's own either way.
pub fn write_layer(
    scene: &Scene,
    placement: TexturePlacement,
    fps: f32,
    light_units: LightUnits,
    clips: UsdClips,
) -> Result<(String, Vec<TextureFile>), MeshError> {
    scene.validate()?;
    let mut owned = super::glb::hoist_skinned_meshes(scene);
    // A prim is one thing: a light or camera that shares its node with a
    // mesh, a joint or the other gets a child node of its own, as
    // `usd::prepare` gives it one (a no-op on a prepared scene).
    super::fbx::separate_lights_and_cameras(&mut owned);
    // One width for every point and line, from the scene as it stands at
    // rest. The scene is prepared (`usd::prepare` has de-indexed it), so these
    // are the bounds of the vertices the file holds; one written without
    // `prepare` is measured before the de-indexing below drops the vertices
    // no index names.
    let width = if owned.meshes.iter().flat_map(|m| &m.primitives).any(|p| p.topology != Topology::Triangles) {
        point_width(super::pose::world_bounds(&owned, &super::pose::default_pose(&owned)))
    } else {
        point_width(None)
    };
    // Each point and each end of a segment a vertex of its own (a no-op on
    // a prepared scene).
    deindex_scene(&mut owned)?;
    let scene = &owned;
    let parents = scene.parents();
    let world = scene.world_matrices();
    let is_joint = skeleton_joints(scene);
    let mut names = Names::default();
    let anim = scene.animations.first();
    let channels: Vec<&Channel> = anim.map(|a| a.channels.iter().collect()).unwrap_or_default();
    // Several clips (module docs, "Clips"): empty for a file of one timeline,
    // which is written exactly as before there were clips.
    let plan = clip_plan(scene, &is_joint, clips);
    let multi = !plan.is_empty();

    let root_name = names.take(scene.name.as_deref().unwrap_or("Root"), "Root");
    let materials_name = names.take("Materials", "Materials");
    let root_path = format!("/{root_name}");
    let materials_path = format!("{root_path}/{materials_name}");

    // Texture files.
    let mut files: Vec<Option<String>> = vec![None; scene.images.len()];
    let mut tex_out = Vec::new();
    if placement == TexturePlacement::Files {
        let used: Vec<bool> = (0..scene.images.len())
            .map(|i| scene.materials.iter().any(|m| m.texture_refs().any(|(_, r)| scene.textures.get(r.texture).is_some_and(|t| t.image == i))))
            .collect();
        let mut taken: Vec<String> = Vec::new();
        for (i, im) in scene.images.iter().enumerate() {
            if !used[i] || !im.format.is_universal() {
                continue;
            }
            let stem = package::safe_file_stem(im.name.as_deref().unwrap_or(""), &format!("image_{i}"));
            let mut name = format!("textures/{stem}.{}", im.format.extension());
            let mut n = 1;
            // Folded: paths that differ only in case are one file on macOS and Windows.
            while taken.contains(&name.to_ascii_lowercase()) {
                name = format!("textures/{stem}_{n}.{}", im.format.extension());
                n += 1;
            }
            taken.push(name.to_ascii_lowercase());
            files[i] = Some(name.clone());
            tex_out.push(TextureFile { path: name, bytes: im.data.clone() });
        }
    }

    // Skins and morphs decide the root type and the skeleton prims.
    let layouts = skeleton_layouts(scene, &parents, &world, &mut names)?;
    let skinned_mesh_node: Vec<bool> = (0..scene.nodes.len()).map(|n| scene.nodes[n].skin.is_some() && scene.nodes[n].mesh.is_some()).collect();
    let morph_nodes: Vec<usize> = (0..scene.nodes.len())
        .filter(|&n| !skinned_mesh_node[n] && scene.nodes[n].mesh.is_some_and(|m| scene.meshes[m].target_count() > 0))
        .collect();
    let skel_root = !layouts.is_empty() || !morph_nodes.is_empty();

    // Which nodes get prims: every non-joint node, and joints that carry a
    // mesh or lead to something that is not a joint (mirrored as Xforms).
    let mut needs = vec![false; scene.nodes.len()];
    for &n in scene.topological_order().iter().rev() {
        let node = &scene.nodes[n];
        needs[n] = !is_joint[n] || (node.mesh.is_some() && !skinned_mesh_node[n]) || node.children.iter().any(|c| needs[*c]);
    }
    let anchored: Vec<Vec<usize>> = {
        let mut v = vec![Vec::new(); scene.nodes.len()];
        for (li, l) in layouts.iter().enumerate() {
            if let Some(a) = l.anchor {
                v[a].push(li);
            }
        }
        v
    };

    // Node prim names, in the order the prims are written.
    let mut node_names: Vec<Option<String>> = vec![None; scene.nodes.len()];
    {
        let mut stack: Vec<usize> = scene.roots.iter().rev().copied().collect();
        while let Some(n) = stack.pop() {
            if skinned_mesh_node[n] {
                // Its children's prims hang where it stands (see the walk).
                stack.extend(scene.nodes[n].children.iter().rev().copied());
                continue;
            }
            if !needs[n] {
                continue;
            }
            node_names[n] = Some(names.take(scene.nodes[n].name.as_deref().unwrap_or(""), &format!("node_{n}")));
            stack.extend(scene.nodes[n].children.iter().rev().copied());
        }
    }

    // Material variants in use.
    let mut mat_keys: Vec<MatKey> = Vec::new();
    for m in &scene.meshes {
        for p in &m.primitives {
            let k = MatKey { material: p.material, vertex_color: p.colors.is_some() };
            if !mat_keys.contains(&k) {
                mat_keys.push(k);
            }
        }
    }
    let mut mat_names: HashMap<MatKey, String> = HashMap::new();
    for k in &mat_keys {
        let base = match k.material {
            Some(i) => scene.materials[i].name.clone().unwrap_or_else(|| format!("Material_{i}")),
            None => "DefaultMaterial".to_string(),
        };
        let both = mat_keys.contains(&MatKey { material: k.material, vertex_color: !k.vertex_color });
        let wanted = if k.vertex_color && both { format!("{base}_vertexColor") } else { base };
        mat_names.insert(*k, names.take(&wanted, "Material"));
    }
    let default_mat = default_material();
    let morph_skel_name = (!morph_nodes.is_empty()).then(|| names.take("MorphTargets", "MorphTargets"));
    let anim_name = anim.and_then(|a| a.name.clone()).unwrap_or_else(|| "Animation".to_string());

    let mut o = Out { text: String::new(), depth: 0 };
    // Layer metadata.
    o.line("#usda 1.0");
    o.line("(");
    o.depth += 1;
    o.line("customLayerData = {");
    o.depth += 1;
    if let Some(n) = anim.and_then(|a| a.name.as_deref()) {
        o.line(&format!("string animationName = {}", quote(n)));
    }
    o.line("string creator = \"litegen mesh converter\"");
    o.depth -= 1;
    o.line("}");
    o.line(&format!("defaultPrim = {}", quote(&root_name)));
    // One timeline as long as the longest clip.
    let span = if multi {
        plan.iter()
            .flat_map(|c| c.channels.iter())
            .flat_map(|c| c.times.iter().copied())
            .fold(None, |s: Option<(f32, f32)>, t| Some(s.map_or((t, t), |(lo, hi)| (lo.min(t), hi.max(t)))))
    } else {
        let codes: Vec<f32> = channels.iter().flat_map(|c| c.times.iter().copied()).collect();
        codes.iter().copied().reduce(f32::min).zip(codes.iter().copied().reduce(f32::max))
    };
    if let Some((lo, hi)) = span {
        o.line(&format!("endTimeCode = {}", time_code(hi)));
        o.line(&format!("framesPerSecond = {fps}"));
        o.line("metersPerUnit = 1");
        // Playback starts at 0 (see `pose::resample_times`), not at the first key.
        o.line(&format!("startTimeCode = {}", time_code(lo.min(0.0))));
        o.line(&format!("timeCodesPerSecond = {}", TIME_CODES_PER_SECOND));
    } else {
        o.line("metersPerUnit = 1");
    }
    o.line("upAxis = \"Y\"");
    o.depth -= 1;
    o.line(")");
    o.line("");

    let mut root_meta = Vec::new();
    if scene.name.as_deref().is_some_and(|n| n != root_name) {
        root_meta.push(format!("customData = {{ string {} = {} }}", super::usd_read::NAME_KEY, quote(scene.name.as_deref().unwrap_or(""))));
    }
    if multi {
        // The first clip is the one selected.
        root_meta.push(format!("prepend variantSets = {}", quote(super::usd_stage::ANIMATION_SET)));
        root_meta.push(format!("variants = {{ string {} = {} }}", super::usd_stage::ANIMATION_SET, quote(&plan[0].variant)));
    }
    o.open(&format!("def {} {}", if skel_root { "SkelRoot" } else { "Xform" }, quote(&root_name)), &root_meta);

    // Materials.
    if !mat_keys.is_empty() {
        o.open(&format!("def Scope {}", quote(&materials_name)), &[]);
        for k in &mat_keys {
            let name = &mat_names[k];
            let m = k.material.map_or(&default_mat, |i| &scene.materials[i]);
            // Recorded whenever the prim name is not the name itself: sanitised,
            // made unique, or the vertex-colour variant's.
            let original = m.name.as_deref().filter(|n| *n != name.as_str());
            write_material(&mut o, &format!("{materials_path}/{name}"), name, original, m, k.vertex_color, scene, &files);
        }
        o.close();
    }

    // Nodes, with an explicit stack: (node, its prim path, is-close marker).
    let mut ctx = Ctx {
        scene,
        names: &mut names,
        channels: &channels,
        materials_path: &materials_path,
        mat_names: &mat_names,
        morph_skel: morph_skel_name.as_ref().map(|n| format!("{root_path}/{n}")),
        layouts: &layouts,
        anim_name: &anim_name,
        node_names: &node_names,
        skel_paths: HashMap::new(),
        width,
        plan: &plan,
        bindings: Vec::new(),
    };
    enum Step {
        Open(usize, String),
        Close,
        Skeletons(usize, String),
    }
    let mut stack: Vec<Step> = Vec::new();
    for &r in scene.roots.iter().rev() {
        stack.push(Step::Open(r, root_path.clone()));
    }
    // Skeletons whose anchor is the root itself.
    stack.insert(0, Step::Skeletons(usize::MAX, root_path.clone()));
    let mut steps = 0usize;
    while let Some(step) = stack.pop() {
        steps += 1;
        if steps > 4 * scene.nodes.len() + 16 {
            return Err(malformed("the node graph is not a tree"));
        }
        match step {
            Step::Close => o.close(),
            Step::Skeletons(anchor, parent_path) => {
                for (li, l) in layouts.iter().enumerate() {
                    let here = if anchor == usize::MAX { l.anchor.is_none() } else { l.anchor == Some(anchor) };
                    if here {
                        ctx.write_skeleton(&mut o, li, &parent_path)?;
                    }
                }
            }
            Step::Open(n, parent_path) => {
                if skinned_mesh_node[n] {
                    // Written beside its skeleton. It stands at a root,
                    // untransformed (`hoist_skinned_meshes` leaves it only
                    // there), so its children hang where it stands, under
                    // its parent's prim: a light or camera `usd::prepare`
                    // moved off it among them.
                    for &c in scene.nodes[n].children.iter().rev() {
                        stack.push(Step::Open(c, parent_path.clone()));
                    }
                    continue;
                }
                if !needs[n] {
                    continue;
                }
                let node = &scene.nodes[n];
                let name = ctx.node_names[n].clone().unwrap_or_else(|| format!("node_{n}"));
                let path = format!("{parent_path}/{name}");
                let kids: Vec<usize> = node.children.iter().copied().filter(|c| needs[*c] && !skinned_mesh_node[*c]).collect();
                let single = node
                    .mesh
                    .filter(|m| scene.meshes[*m].primitives.len() == 1 && kids.is_empty() && anchored[n].is_empty());
                match single {
                    Some(m) => {
                        ctx.write_mesh(&mut o, &name, &path, node.name.as_deref(), m, 0, Some(n), None)?;
                    }
                    None => {
                        // A node's light or camera is its prim (module docs):
                        // separated above, it shares the node with no mesh.
                        let light = node.light.and_then(|l| scene.lights.get(l));
                        let camera = node.camera.and_then(|c| scene.cameras.get(c));
                        let original = node.name.as_deref().filter(|o| *o != name);
                        let mut meta = Vec::new();
                        let ty = match (light, camera) {
                            (Some(l), _) => {
                                if matches!(l.kind, LightKind::Spot { .. }) {
                                    meta.push("prepend apiSchemas = [\"ShapingAPI\"]".to_string());
                                }
                                meta.extend(custom_data(original, &light_entries(l)));
                                light_type(l)
                            }
                            (None, Some(c)) => {
                                meta.extend(custom_data(original, &camera_entries(c)));
                                "Camera"
                            }
                            (None, None) => {
                                if let Some(orig) = original {
                                    meta.push(format!("customData = {{ string {} = {} }}", super::usd_read::NAME_KEY, quote(orig)));
                                }
                                "Xform"
                            }
                        };
                        o.open(&format!("def {ty} {}", quote(&name)), &meta);
                        ctx.write_xform(&mut o, n);
                        if let Some(l) = light {
                            write_light(&mut o, l, light_units);
                        } else if let Some(c) = camera {
                            write_camera(&mut o, c);
                        }
                        if let Some(m) = node.mesh {
                            let mesh = &scene.meshes[m];
                            for k in 0..mesh.primitives.len() {
                                let base = mesh.name.clone().unwrap_or_else(|| name.clone());
                                let pname = ctx.names.take(&format!("{base}_{k}"), "Mesh");
                                let ppath = format!("{path}/{pname}");
                                ctx.write_mesh(&mut o, &pname, &ppath, None, m, k, None, Some(n))?;
                            }
                        }
                        stack.push(Step::Close);
                        for &c in kids.iter().rev() {
                            stack.push(Step::Open(c, path.clone()));
                        }
                        stack.push(Step::Skeletons(n, path.clone()));
                    }
                }
            }
        }
    }
    // The joint-less skeleton that drives unskinned blend shapes.
    if let Some(ms) = &morph_skel_name {
        ctx.write_morph_skeleton(&mut o, ms, &morph_nodes)?;
    }
    if multi {
        ctx.write_animation_variants(&mut o, &root_path);
    }
    o.close();
    Ok((o.text, tex_out))
}

struct Ctx<'a> {
    scene: &'a Scene,
    names: &'a mut Names,
    channels: &'a [&'a Channel],
    materials_path: &'a str,
    mat_names: &'a HashMap<MatKey, String>,
    morph_skel: Option<String>,
    layouts: &'a [SkelLayout],
    anim_name: &'a str,
    /// Prim names of the nodes that have prims, allocated up front so nodes
    /// win name clashes against materials and helper prims.
    node_names: &'a [Option<String>],
    skel_paths: HashMap<usize, String>,
    /// The `widths` of every Points and BasisCurves prim ([`point_width`]).
    width: f32,
    /// The clips of a file that holds several; empty for one timeline.
    plan: &'a [ClipPlan<'a>],
    /// Each animated skeleton's prim path with the path of its SkelAnimation
    /// for each clip, in `plan`'s order: what the variants switch
    /// ([`Ctx::write_animation_variants`]). Empty for one timeline.
    bindings: Vec<(String, Vec<String>)>,
}

/// Blend-shape binding names: unique per mesh instance, so two instances of
/// one mesh can hold different weights.
fn shape_tokens(scene: &Scene, node: usize, mesh: usize) -> Vec<String> {
    let m = &scene.meshes[mesh];
    (0..m.target_count())
        .map(|k| {
            let t = m.target_names.get(k).cloned().unwrap_or_else(|| format!("target_{k}"));
            identifier(&format!("n{node}_{t}"), &format!("n{node}_target_{k}"))
        })
        .collect()
}

fn mesh_weights(scene: &Scene, node: usize, mesh: usize) -> Vec<f32> {
    let m = &scene.meshes[mesh];
    match &scene.nodes[node].weights {
        Some(w) => w.clone(),
        None if m.weights.len() == m.target_count() => m.weights.clone(),
        None => vec![0.0; m.target_count()],
    }
}

impl<'a> Ctx<'a> {
    fn write_xform(&self, o: &mut Out, n: usize) {
        let node = &self.scene.nodes[n];
        let chans: Vec<&&Channel> = self.channels.iter().filter(|c| c.node == n && c.path != AnimPath::Weights).collect();
        let animated = |p: AnimPath| chans.iter().find(|c| c.path == p);
        match (&node.transform, chans.is_empty()) {
            (Transform::Matrix(m), true) => {
                if !Transform::Matrix(*m).is_identity() {
                    o.line(&format!("matrix4d xformOp:transform = {}", matrix(m)));
                    o.line("uniform token[] xformOpOrder = [\"xformOp:transform\"]");
                }
            }
            _ => {
                let (t, r, s) = node.transform.trs().unwrap_or(([0.0; 3], math::QUAT_IDENTITY, [1.0; 3]));
                let mut order = Vec::new();
                let t32 = [t[0] as f32, t[1] as f32, t[2] as f32];
                let r32 = math::quat_normalize(r).map(|v| v as f32);
                let s32 = [s[0] as f32, s[1] as f32, s[2] as f32];
                // float3, not double3: three.js's crate reader throws on a
                // double3 whose components inline as small integers.
                if t != [0.0; 3] || animated(AnimPath::Translation).is_some() {
                    o.line(&format!("float3 xformOp:translate = {}", v3(t32)));
                    if let Some(ch) = animated(AnimPath::Translation) {
                        write_samples(o, "float3 xformOp:translate", &ch.times, |i| v3([ch.values[i * 3], ch.values[i * 3 + 1], ch.values[i * 3 + 2]]));
                    }
                    order.push("xformOp:translate");
                }
                if r != math::QUAT_IDENTITY || animated(AnimPath::Rotation).is_some() {
                    o.line(&format!("quatf xformOp:orient = {}", quat(r32)));
                    if let Some(ch) = animated(AnimPath::Rotation) {
                        let qs = continuous_quats(&ch.values);
                        write_samples(o, "quatf xformOp:orient", &ch.times, |i| quat([qs[i * 4], qs[i * 4 + 1], qs[i * 4 + 2], qs[i * 4 + 3]]));
                    }
                    order.push("xformOp:orient");
                }
                if s != [1.0; 3] || animated(AnimPath::Scale).is_some() {
                    o.line(&format!("float3 xformOp:scale = {}", v3(s32)));
                    if let Some(ch) = animated(AnimPath::Scale) {
                        write_samples(o, "float3 xformOp:scale", &ch.times, |i| v3([ch.values[i * 3], ch.values[i * 3 + 1], ch.values[i * 3 + 2]]));
                    }
                    order.push("xformOp:scale");
                }
                if !order.is_empty() {
                    o.line(&format!("uniform token[] xformOpOrder = {}", list(&order, |s| quote(s))));
                }
            }
        }
    }

    /// One primitive as a Mesh prim. `xform_node` puts that node's transform
    /// on the prim; `skin` binds it to a skeleton layout.
    #[allow(clippy::too_many_arguments)]
    fn write_mesh(
        &mut self,
        o: &mut Out,
        name: &str,
        path: &str,
        original: Option<&str>,
        mesh: usize,
        k: usize,
        xform_node: Option<usize>,
        owner: Option<usize>,
    ) -> Result<(), MeshError> {
        self.write_mesh_full(o, name, path, original, mesh, k, xform_node, owner, None)
    }

    #[allow(clippy::too_many_arguments)]
    fn write_mesh_full(
        &mut self,
        o: &mut Out,
        name: &str,
        path: &str,
        original: Option<&str>,
        mesh: usize,
        k: usize,
        xform_node: Option<usize>,
        owner: Option<usize>,
        skin: Option<(usize, usize)>,
    ) -> Result<(), MeshError> {
        let scene = self.scene;
        let m = &scene.meshes[mesh];
        let p: &Primitive = &m.primitives[k];
        let node = xform_node.or(owner).or(skin.map(|s| s.1));
        let morph = !p.targets.is_empty();
        let mut schemas = vec!["\"MaterialBindingAPI\""];
        if skin.is_some() || morph {
            schemas.insert(0, "\"SkelBindingAPI\"");
        }
        let mut meta = vec![format!("prepend apiSchemas = [{}]", schemas.join(", "))];
        if let Some(orig) = original.filter(|o| *o != name) {
            meta.push(format!("customData = {{ string {} = {} }}", super::usd_read::NAME_KEY, quote(orig)));
        }
        // A primitive of points or lines is a Points or a BasisCurves prim,
        // not a Mesh with faces that draw nothing (UsdGeomPoints,
        // UsdGeomBasisCurves; `OpenUSD@v26.11.1 pxr/usd/usdGeom/schema.usda`
        // L1877 and L1594-1812).
        let topology = p.topology;
        let type_name = match topology {
            Topology::Triangles => "Mesh",
            Topology::Lines => "BasisCurves",
            Topology::Points => "Points",
        };
        o.open(&format!("def {type_name} {}", quote(name)), &meta);
        if let Some(n) = xform_node {
            self.write_xform(o, n);
        }
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for v in &p.positions {
            for a in 0..3 {
                lo[a] = lo[a].min(v[a]);
                hi[a] = hi[a].max(v[a]);
            }
        }
        if p.positions.is_empty() {
            lo = [0.0; 3];
            hi = [0.0; 3];
        } else if topology != Topology::Triangles {
            // A point or a line has a width: its extent is its positions
            // padded by half of it on every side, as `ComputeExtent` of
            // UsdGeomPoints and UsdGeomBasisCurves does with widths.
            for a in 0..3 {
                lo[a] -= self.width / 2.0;
                hi[a] += self.width / 2.0;
            }
        }
        o.line(&format!("float3[] extent = [{}, {}]", v3(lo), v3(hi)));
        // The element counts: a face of 3 vertices, or a curve of 2 (Ruling
        // I10-R8: one 2-vertex curve a segment). Points have none.
        let counted = |size: usize, name: &str, o: &mut Out| {
            let n = p.indices.len() / size;
            let mut counts = String::with_capacity(n * (size.to_string().len() + 2) + 2);
            counts.push('[');
            for i in 0..n {
                if i > 0 {
                    counts.push_str(", ");
                }
                counts.push_str(&size.to_string());
            }
            counts.push(']');
            o.line(&format!("int[] {name} = {counts}"));
        };
        match topology {
            Topology::Triangles => {
                counted(3, "faceVertexCounts", o);
                o.line(&format!("int[] faceVertexIndices = {}", list(&p.indices, |i| i.to_string())));
            }
            Topology::Lines => {
                // Linear and nonperiodic: a polyline of its two vertices.
                o.line("uniform token type = \"linear\"");
                o.line("uniform token wrap = \"nonperiodic\"");
                counted(2, "curveVertexCounts", o);
            }
            Topology::Points => {}
        }
        o.line(&format!("point3f[] points = {}", list(&p.positions, |v| v3(*v))));
        if topology != Topology::Triangles {
            o.line(&format!("float[] widths = [{}] (", fmt_f32(self.width)));
            o.line("    interpolation = \"constant\"");
            o.line(")");
        }
        // A curve's normals would turn it into a ribbon: not written.
        if let Some(ns) = p.normals.as_ref().filter(|_| topology != Topology::Lines) {
            o.line(&format!("normal3f[] normals = {} (", list(ns, |v| v3(*v))));
            o.line("    interpolation = \"vertex\"");
            o.line(")");
        }
        // Every set the prepared scene has, which `pose::degrade` cut to the
        // target's count (eight in a USDA, two in a USDZ); never past `st7`,
        // the last name the reader takes.
        for (si, set) in p.uvs.iter().enumerate().take(super::usd::UV_SETS) {
            let n = if si == 0 { "st".to_string() } else { format!("st{si}") };
            // USD's st origin is the bottom-left: t = 1 − v.
            o.line(&format!("texCoord2f[] primvars:{n} = {} (", list(set, |v| v2([v[0], 1.0 - v[1]]))));
            o.line("    interpolation = \"vertex\"");
            o.line(")");
        }
        let material = p.material.and_then(|i| scene.materials.get(i));
        if let Some(cs) = &p.colors {
            // The material's base colour multiplies the vertex colour here:
            // the surface reads only the primvar.
            let base = material.map_or([1.0; 4], |m| m.base_color);
            o.line(&format!("color3f[] primvars:displayColor = {} (", list(cs, |c| v3([c[0] * base[0], c[1] * base[1], c[2] * base[2]]))));
            o.line("    interpolation = \"vertex\"");
            o.line(")");
            if material.is_some_and(|m| m.alpha_mode != AlphaMode::Opaque) {
                o.line(&format!("float[] primvars:displayOpacity = {} (", list(cs, |c| fmt_f32(c[3] * base[3]))));
                o.line("    interpolation = \"vertex\"");
                o.line(")");
            }
        }
        if let (Some((li, _)), Some(sw)) = (skin, &p.skin) {
            let l = &self.layouts[li];
            let n = sw.influences;
            let mut ji = Vec::with_capacity(sw.joints.len());
            let mut jw = Vec::with_capacity(sw.weights.len());
            for v in 0..p.positions.len() {
                let (js, ws) = sw.vertex(v);
                let sum: f64 = ws.iter().map(|w| w.max(0.0) as f64).sum();
                for (j, w) in js.iter().zip(ws) {
                    let w = if sum > 0.0 { (w.max(0.0) as f64 / sum) as f32 } else { 0.0 };
                    ji.push(if w > 0.0 { l.skin_to_usd.get(*j as usize).copied().unwrap_or(0) } else { 0 });
                    jw.push(w);
                }
                if sum <= 0.0 {
                    // Bound wholly to its first joint, as the scene evaluates it.
                    let at = jw.len() - n;
                    jw[at] = 1.0;
                    ji[at] = l.skin_to_usd.get(js[0] as usize).copied().unwrap_or(0);
                }
            }
            o.line(&format!("int[] primvars:skel:jointIndices = {} (", list(&ji, |i| i.to_string())));
            o.line(&format!("    elementSize = {n}"));
            o.line("    interpolation = \"vertex\"");
            o.line(")");
            o.line(&format!("float[] primvars:skel:jointWeights = {} (", list(&jw, |w| fmt_f32(*w))));
            o.line(&format!("    elementSize = {n}"));
            o.line("    interpolation = \"vertex\"");
            o.line(")");
            o.line(&format!("matrix4d primvars:skel:geomBindTransform = {}", matrix(&math::IDENTITY)));
        }
        let skeleton_path = match skin {
            Some((li, _)) => Some(self.skeleton_path(li)),
            None if morph => self.morph_skel.clone(),
            None => None,
        };
        let mut shapes = Vec::new();
        if morph {
            let node = node.ok_or_else(|| malformed("a morphed mesh without a node"))?;
            let tokens = shape_tokens(scene, node, mesh);
            let mut local = Names::default();
            for (ti, _) in p.targets.iter().enumerate() {
                let tname = m.target_names.get(ti).cloned().unwrap_or_else(|| format!("target_{ti}"));
                shapes.push(local.take(&tname, &format!("target_{ti}")));
            }
            o.line(&format!("uniform token[] skel:blendShapes = {}", list(&tokens, |t| quote(t))));
            o.line(&format!("rel skel:blendShapeTargets = {}", list(&shapes, |s| format!("<{path}/{s}>"))));
        }
        if let Some(sp) = &skeleton_path {
            o.line(&format!("rel skel:skeleton = <{sp}>"));
        }
        if topology == Topology::Triangles {
            o.line("uniform token orientation = \"rightHanded\"");
            o.line("uniform token subdivisionScheme = \"none\"");
        }
        if material.is_some_and(|m| m.double_sided) {
            o.line("uniform bool doubleSided = 1");
        }
        let key = MatKey { material: p.material, vertex_color: p.colors.is_some() };
        if let Some(mn) = self.mat_names.get(&key) {
            o.line(&format!("rel material:binding = <{}/{mn}>", self.materials_path));
        }
        for (ti, t) in p.targets.iter().enumerate() {
            // A name the prim cannot carry (`smile:L`) travels in customData,
            // as a node's does; the reader names the target from it.
            let meta: Vec<String> = match m.target_names.get(ti) {
                Some(orig) if *orig != shapes[ti] => {
                    vec![format!("customData = {{ string {} = {} }}", super::usd_read::NAME_KEY, quote(orig))]
                }
                _ => Vec::new(),
            };
            o.open(&format!("def BlendShape {}", quote(&shapes[ti])), &meta);
            o.line(&format!("uniform vector3f[] offsets = {}", list(&t.positions, |v| v3(*v))));
            if let Some(ns) = &t.normals {
                o.line(&format!("uniform vector3f[] normalOffsets = {}", list(ns, |v| v3(*v))));
            }
            o.close();
        }
        o.close();
        Ok(())
    }

    fn skeleton_path(&self, li: usize) -> String {
        // Set by write_skeleton, which writes a skeleton before its meshes.
        self.skel_paths.get(&li).cloned().unwrap_or_default()
    }

    fn write_skeleton(&mut self, o: &mut Out, li: usize, parent_path: &str) -> Result<(), MeshError> {
        let scene = self.scene;
        let l = &self.layouts[li];
        let skin = &scene.skins[l.skin];
        let name = self.names.take(skin.name.as_deref().unwrap_or("Skeleton"), "Skeleton");
        let path = format!("{parent_path}/{name}");
        self.skel_paths.insert(li, path.clone());
        // Joint animation: every channel on a joint of this skeleton, in each
        // clip (one timeline: the one clip's).
        let on_joints = |chans: &[&'a Channel]| -> Vec<&'a Channel> {
            chans.iter().copied().filter(|c| l.joints.contains(&c.node) && c.path != AnimPath::Weights).collect()
        };
        let clip_chans: Vec<Vec<&Channel>> = if self.plan.is_empty() {
            vec![on_joints(self.channels)]
        } else {
            self.plan.iter().map(|clip| on_joints(&clip.channels)).collect()
        };
        // Blend shapes of the meshes bound here.
        let mut shape_tokens_all: Vec<String> = Vec::new();
        let mut weight_sources: Vec<(usize, usize)> = Vec::new();
        for &n in &l.meshes {
            if let Some(m) = scene.nodes[n].mesh {
                if scene.meshes[m].target_count() > 0 {
                    shape_tokens_all.extend(shape_tokens(scene, n, m));
                    weight_sources.push((n, m));
                }
            }
        }
        let has_anim = clip_chans.iter().any(|c| !c.is_empty()) || !weight_sources.is_empty();
        o.open(&format!("def Skeleton {}", quote(&name)), &["prepend apiSchemas = [\"SkelBindingAPI\"]".to_string()]);
        o.line(&format!("uniform token[] joints = {}", list(&l.paths, |p| quote(p))));
        // A joint whose name is not a prim name (Mixamo's `mixamorig:Hips`)
        // keeps it here: UsdSkel's jointNames, "If authored, provides a unique
        // name per joint", which readers name the joint by.
        // Unique, as UsdSkel asks ("a unique name per joint"): a name already
        // taken falls back to the joint's own (unique) prim name.
        let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
        let joint_names: Vec<String> = l
            .joints
            .iter()
            .zip(&l.paths)
            .map(|(&j, p)| {
                let leaf = p.rsplit('/').next().unwrap_or(p).to_string();
                let name = scene.nodes[j].name.clone().filter(|n| !taken.contains(n)).unwrap_or(leaf);
                taken.insert(name.clone());
                name
            })
            .collect();
        if joint_names.iter().zip(&l.paths).any(|(n, p)| p.rsplit('/').next() != Some(n.as_str())) {
            o.line(&format!("uniform token[] jointNames = {}", list(&joint_names, |n| quote(n))));
        }
        o.line(&format!("uniform matrix4d[] bindTransforms = {}", list(&l.bind, matrix)));
        o.line(&format!("uniform matrix4d[] restTransforms = {}", list(&l.rest, matrix)));
        // The SkelAnimation prims: one for one timeline, one for each clip of
        // a file that holds several, those named for the clip.
        let mut prims: Vec<String> = Vec::new();
        if has_anim {
            if self.plan.is_empty() {
                prims.push(self.names.take(self.anim_name, "Animation"));
            } else {
                for clip in self.plan {
                    prims.push(self.names.take(clip.name.unwrap_or(""), "Animation"));
                }
            }
        }
        // One timeline: the skeleton binds its animation itself. With clips
        // the variants of the set `Animation` do, and nothing outside them
        // does: a local opinion would beat every variant's.
        if let (true, Some(a)) = (self.plan.is_empty(), prims.first()) {
            o.line(&format!("rel skel:animationSource = <{path}/{a}>"));
        }
        let mut sampled = 0usize;
        for (i, prim) in prims.iter().enumerate() {
            let meta = self.clip_meta(i, prim);
            let whole: &[&Channel] = self.plan.get(i).map_or(self.channels, |clip| clip.channels.as_slice());
            self.write_skel_animation(o, prim, &meta, &l.joints, &l.paths, &clip_chans[i], whole, &shape_tokens_all, &weight_sources, &mut sampled)?;
        }
        o.close();
        if !self.plan.is_empty() && !prims.is_empty() {
            self.bindings.push((path.clone(), prims.iter().map(|p| format!("{path}/{p}")).collect()));
        }
        // Skinned meshes beside the skeleton, in the same space.
        for &n in &l.meshes {
            let Some(m) = scene.nodes[n].mesh else { continue };
            let mesh = &scene.meshes[m];
            for k in 0..mesh.primitives.len() {
                let base = if mesh.primitives.len() == 1 {
                    scene.nodes[n].name.clone().unwrap_or_else(|| format!("skinned_{n}"))
                } else {
                    format!("{}_{k}", mesh.name.clone().or_else(|| scene.nodes[n].name.clone()).unwrap_or_else(|| format!("skinned_{n}")))
                };
                let pname = self.names.take(&base, "Mesh");
                let ppath = format!("{parent_path}/{pname}");
                let original = if mesh.primitives.len() == 1 { scene.nodes[n].name.clone() } else { None };
                self.write_mesh_full(o, &pname, &ppath, original.as_deref(), m, k, None, None, Some((li, n)))?;
            }
        }
        Ok(())
    }

    /// The metadata of SkelAnimation prim `name` of clip `clip`: for a file of
    /// several clips the clip's own name when the prim's is not it
    /// (sanitised, or made unique), and where it ends (`clipEnd`, seconds, a
    /// number of the scene's own and so a double, Ruling U15); none for one
    /// timeline.
    fn clip_meta(&self, clip: usize, name: &str) -> Vec<String> {
        match self.plan.get(clip) {
            Some(c) => custom_data(c.name.filter(|n| *n != name), &[format!("double clipEnd = {}", fmt_f32(c.end))]),
            None => Vec::new(),
        }
    }

    /// One SkelAnimation prim: `joints` (scene nodes, with their `paths`)
    /// posed by `joint_chans`, the blend shapes of `sources` weighted by the
    /// weights among `clip_chans`, the clip's channels. A clip that moves no
    /// joint here lists none, and the skeleton is at rest. `sampled` counts
    /// the values the skeleton's clips have sampled so far, against the
    /// budget they share.
    #[allow(clippy::too_many_arguments)]
    fn write_skel_animation(
        &self,
        o: &mut Out,
        name: &str,
        meta: &[String],
        joints: &[usize],
        paths: &[String],
        joint_chans: &[&Channel],
        clip_chans: &[&Channel],
        tokens: &[String],
        sources: &[(usize, usize)],
        sampled: &mut usize,
    ) -> Result<(), MeshError> {
        let scene = self.scene;
        o.open(&format!("def SkelAnimation {}", quote(name)), meta);
        if !joint_chans.is_empty() {
            o.line(&format!("uniform token[] joints = {}", list(paths, |p| quote(p))));
            let times = channel_times(joint_chans);
            // Every joint at every key of any joint: budgeted before the
            // samples exist (`pose::MAX_RESAMPLED_VALUES`, ten values each).
            *sampled = sampled.saturating_add(times.len().saturating_mul(joints.len()).saturating_mul(10));
            if *sampled > super::pose::MAX_RESAMPLED_VALUES {
                return Err(MeshError::Unsupported(format!(
                    "the skeleton animation would sample more than {} values",
                    super::pose::MAX_RESAMPLED_VALUES
                )));
            }
            let rest: Vec<Trs> = joints.iter().map(|&n| trs_at(scene, &[], n, 0.0)).collect();
            o.line(&format!("float3[] translations = {}", list(&rest, |r| v3(r.0.map(|v| v as f32)))));
            o.line(&format!("quatf[] rotations = {}", list(&rest, |r| quat(math::quat_normalize(r.1).map(|v| v as f32)))));
            // Halves, as Sdf stores `half3[]` (Ruling U13b): a value that
            // is not one is a value no reader sees.
            o.line(&format!("half3[] scales = {}", list(&rest, |r| v3(r.2.map(|v| super::usd::to_half_precision(v as f32))))));
            let mut samples: Vec<Vec<Trs>> = times.iter().map(|&t| joints.iter().map(|&n| trs_at(scene, joint_chans, n, t)).collect()).collect();
            // Neighbouring keys on the same hemisphere, for readers that
            // do not take the short way round themselves.
            for j in 0..joints.len() {
                for i in 1..samples.len() {
                    let prev = samples[i - 1][j].1;
                    let q = &mut samples[i][j].1;
                    if math::quat_dot(prev, *q) < 0.0 {
                        *q = [-q[0], -q[1], -q[2], -q[3]];
                    }
                }
            }
            write_samples(o, "float3[] translations", &times, |i| list(&samples[i], |r| v3(r.0.map(|v| v as f32))));
            write_samples(o, "quatf[] rotations", &times, |i| list(&samples[i], |r| quat(r.1.map(|v| v as f32))));
            // The samples between the keys `prepare` rounded are rounded again.
            write_samples(o, "half3[] scales", &times, |i| list(&samples[i], |r| v3(r.2.map(|v| super::usd::to_half_precision(v as f32)))));
        } else {
            o.line("uniform token[] joints = []");
        }
        if !sources.is_empty() {
            self.write_weights(o, tokens, sources, clip_chans);
        }
        o.close();
        Ok(())
    }

    /// The variant set `Animation` on the default prim, Apple's layout
    /// (`variant_combiner.py`): a variant for each clip, each authoring
    /// `skel:animationSource` on each animated skeleton, through `over`s down
    /// from the default prim, and nothing else. Skeletons whose prims share a
    /// path's start share its `over`s, so a deep hierarchy is written once per
    /// variant, not once per skeleton.
    fn write_animation_variants(&self, o: &mut Out, root_path: &str) {
        struct Bind {
            name: String,
            children: Vec<usize>,
            anims: Option<Vec<String>>,
        }
        let mut arena = vec![Bind { name: String::new(), children: Vec::new(), anims: None }];
        for (skeleton, anims) in &self.bindings {
            let below = skeleton.strip_prefix(root_path).unwrap_or(skeleton);
            let mut at = 0;
            for segment in below.split('/').filter(|s| !s.is_empty()) {
                let existing = arena[at].children.iter().copied().find(|&c| arena[c].name == segment);
                at = match existing {
                    Some(c) => c,
                    None => {
                        arena.push(Bind { name: segment.to_string(), children: Vec::new(), anims: None });
                        let id = arena.len() - 1;
                        arena[at].children.push(id);
                        id
                    }
                };
            }
            arena[at].anims = Some(anims.clone());
        }
        enum Step {
            Open(usize),
            Close,
        }
        o.line(&format!("variantSet {} = {{", quote(super::usd_stage::ANIMATION_SET)));
        o.depth += 1;
        for (k, clip) in self.plan.iter().enumerate() {
            o.line(&format!("{} {{", quote(&clip.variant)));
            o.depth += 1;
            let mut stack: Vec<Step> = arena[0].children.iter().rev().map(|&c| Step::Open(c)).collect();
            while let Some(step) = stack.pop() {
                match step {
                    Step::Close => o.close(),
                    Step::Open(n) => {
                        o.open(&format!("over {}", quote(&arena[n].name)), &[]);
                        if let Some(anims) = &arena[n].anims {
                            o.line(&format!("rel skel:animationSource = <{}>", anims[k]));
                        }
                        stack.push(Step::Close);
                        stack.extend(arena[n].children.iter().rev().map(|&c| Step::Open(c)));
                    }
                }
            }
            o.close();
        }
        o.close();
    }

    fn write_weights(&self, o: &mut Out, tokens: &[String], sources: &[(usize, usize)], channels: &[&Channel]) {
        let scene = self.scene;
        o.line(&format!("uniform token[] blendShapes = {}", list(tokens, |t| quote(t))));
        let rest: Vec<f32> = sources.iter().flat_map(|(n, m)| mesh_weights(scene, *n, *m)).collect();
        o.line(&format!("float[] blendShapeWeights = {}", list(&rest, |w| fmt_f32(*w))));
        let wchans: Vec<&Channel> = channels.iter().copied().filter(|c| c.path == AnimPath::Weights && sources.iter().any(|(n, _)| *n == c.node)).collect();
        if wchans.is_empty() {
            return;
        }
        let times = channel_times(&wchans);
        let rows: Vec<Vec<f32>> = times
            .iter()
            .map(|&t| {
                sources
                    .iter()
                    .flat_map(|(n, m)| {
                        let count = scene.meshes[*m].target_count();
                        match wchans.iter().find(|c| c.node == *n) {
                            Some(ch) => super::pose::sample_channel(ch, t, count),
                            None => mesh_weights(scene, *n, *m),
                        }
                    })
                    .collect()
            })
            .collect();
        write_samples(o, "float[] blendShapeWeights", &times, |i| list(&rows[i], |w| fmt_f32(*w)));
    }

    fn write_morph_skeleton(&mut self, o: &mut Out, name: &str, morph_nodes: &[usize]) -> Result<(), MeshError> {
        let scene = self.scene;
        let path = self.morph_skel.clone().unwrap_or_default();
        let sources: Vec<(usize, usize)> = morph_nodes.iter().filter_map(|&n| scene.nodes[n].mesh.map(|m| (n, m))).collect();
        let tokens: Vec<String> = sources.iter().flat_map(|(n, m)| shape_tokens(scene, *n, *m)).collect();
        // One SkelAnimation for one timeline, one for each clip of a file that
        // holds several ([`Ctx::write_skeleton`] does the same).
        let mut prims: Vec<String> = Vec::new();
        if self.plan.is_empty() {
            prims.push(self.names.take(self.anim_name, "Animation"));
        } else {
            for clip in self.plan {
                prims.push(self.names.take(clip.name.unwrap_or(""), "Animation"));
            }
        }
        o.open(&format!("def Skeleton {}", quote(name)), &["prepend apiSchemas = [\"SkelBindingAPI\"]".to_string()]);
        o.line("uniform token[] joints = []");
        o.line("uniform matrix4d[] bindTransforms = []");
        o.line("uniform matrix4d[] restTransforms = []");
        if let (true, Some(a)) = (self.plan.is_empty(), prims.first()) {
            o.line(&format!("rel skel:animationSource = <{path}/{a}>"));
        }
        let mut sampled = 0usize;
        for (i, prim) in prims.iter().enumerate() {
            let meta = self.clip_meta(i, prim);
            let whole: &[&Channel] = self.plan.get(i).map_or(self.channels, |clip| clip.channels.as_slice());
            self.write_skel_animation(o, prim, &meta, &[], &[], &[], whole, &tokens, &sources, &mut sampled)?;
        }
        o.close();
        if !self.plan.is_empty() {
            self.bindings.push((path.clone(), prims.iter().map(|p| format!("{path}/{p}")).collect()));
        }
        Ok(())
    }
}

/// `name.timeSamples = { code: value, ... }`, one sample per line: three.js's
/// USDA parser is line-based and joins nothing.
fn write_samples(o: &mut Out, decl: &str, times: &[f32], value: impl Fn(usize) -> String) {
    o.line(&format!("{decl}.timeSamples = {{"));
    o.depth += 1;
    let mut line = String::new();
    for (i, t) in times.iter().enumerate() {
        line.clear();
        let _ = write!(line, "{}: {},", time_code(*t), value(i));
        o.line(&line);
    }
    o.depth -= 1;
    o.line("}");
}

/// A rotation track with neighbouring keys on the same hemisphere.
fn continuous_quats(values: &[f32]) -> Vec<f32> {
    let mut out = values.to_vec();
    for i in 1..out.len() / 4 {
        let (a, b) = out.split_at_mut(i * 4);
        let prev = &a[(i - 1) * 4..];
        let q = &mut b[..4];
        let dot: f32 = prev.iter().zip(q.iter()).map(|(x, y)| x * y).sum();
        if dot < 0.0 {
            for v in q.iter_mut() {
                *v = -*v;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::usda::valid_prim_name;

    #[test]
    fn identifiers_are_valid_prim_names() {
        for n in ["box node", "1st", "ünï", "", "a-b", "__", "C:\\x\\"] {
            let id = identifier(n, "fallback");
            assert!(valid_prim_name(&id), "{n:?} -> {id:?}");
        }
        let mut names = Names::default();
        assert_eq!(names.take("a", "x"), "a");
        assert_eq!(names.take("a", "x"), "a_1");
        assert_eq!(names.take("a b", "x"), "a_b");
    }

    #[test]
    fn time_codes_read_back_to_the_same_seconds() {
        for t in [0.0f32, 0.7, 1.0 / 3.0, 2.0, 0.34, 1.9999, 123.456] {
            let s = time_code(t);
            assert_eq!((s.parse::<f64>().unwrap() / TIME_CODES_PER_SECOND) as f32, t, "{t} -> {s}");
        }
        assert_eq!(time_code(0.7), "21");
    }

    /// A primitive of four vertices whose every per-vertex array names the
    /// vertex it belongs to (its tag is in the array's first free slot).
    fn tagged(topology: super::super::scene::Topology, indices: Vec<u32>) -> Primitive {
        use super::super::scene::{MorphTarget, SkinWeights};
        let v = |i: u32| i as f32;
        Primitive {
            positions: (0..4).map(|i| [v(i), 0.0, 0.0]).collect(),
            normals: Some((0..4).map(|i| [0.0, v(i), 0.0]).collect()),
            tangents: Some((0..4).map(|i| [0.0, 0.0, v(i), 1.0]).collect()),
            uvs: vec![(0..4).map(|i| [v(i), 0.5]).collect(), (0..4).map(|i| [0.25, v(i)]).collect()],
            colors: Some((0..4).map(|i| [v(i), 0.0, 0.0, 1.0]).collect()),
            skin: Some(SkinWeights { influences: 4, joints: (0..16).map(|k| (k / 4) as u16).collect(), weights: (0..16).map(|k| (k / 4) as f32).collect() }),
            targets: vec![MorphTarget { positions: (0..4).map(|i| [0.0, 0.0, v(i)]).collect(), normals: Some((0..4).map(|i| [v(i), v(i), 0.0]).collect()) }],
            indices,
            topology,
            ..Default::default()
        }
    }

    /// Every array that is per vertex follows the indices together: the
    /// positions, normals, tangents, each uv set, the colours, the skin's
    /// joints and weights, and each morph target's offsets. A segment's
    /// ends and a point become vertices of their own, in index order.
    #[test]
    fn deindexing_moves_every_per_vertex_array_together() {
        use super::super::scene::Topology;
        for (topology, indices) in [(Topology::Lines, vec![2, 0, 0, 3]), (Topology::Points, vec![1, 1, 3])] {
            let p = tagged(topology, indices.clone());
            let d = deindexed(&p);
            let n = indices.len();
            assert_eq!((d.topology, d.indices.clone()), (topology, (0..n as u32).collect::<Vec<_>>()), "{topology:?}: one vertex per index");
            let tags = |f: &dyn Fn(usize) -> f32| -> Vec<f32> { (0..n).map(f).collect() };
            let want: Vec<f32> = indices.iter().map(|&i| i as f32).collect();
            assert_eq!(tags(&|k| d.positions[k][0]), want, "{topology:?}: positions");
            assert_eq!(tags(&|k| d.normals.as_ref().unwrap()[k][1]), want, "{topology:?}: normals");
            assert_eq!(tags(&|k| d.tangents.as_ref().unwrap()[k][2]), want, "{topology:?}: tangents");
            assert_eq!((d.uvs.len(), tags(&|k| d.uvs[0][k][0]), tags(&|k| d.uvs[1][k][1])), (2, want.clone(), want.clone()), "{topology:?}: uv sets");
            assert_eq!(tags(&|k| d.colors.as_ref().unwrap()[k][0]), want, "{topology:?}: colours");
            let skin = d.skin.as_ref().unwrap();
            assert_eq!(skin.influences, 4);
            let (joints, weights): (Vec<f32>, Vec<f32>) = (0..n).map(|k| (skin.vertex(k).0[0] as f32, skin.vertex(k).1[3])).unzip();
            assert_eq!((joints, weights), (want.clone(), want.clone()), "{topology:?}: the skin of each vertex, whole");
            assert_eq!(tags(&|k| d.targets[0].positions[k][2]), want, "{topology:?}: morph offsets");
            assert_eq!(tags(&|k| d.targets[0].normals.as_ref().unwrap()[k][0]), want, "{topology:?}: morph normal offsets");
            assert_eq!(deindexed(&d), d, "{topology:?}: de-indexed already");
        }
        let tris = tagged(Topology::Triangles, vec![0, 1, 2, 2, 1, 3]);
        assert_eq!(deindexed(&tris), tris, "triangles are not touched");
    }

    /// A point that many indices name is one vertex in the scene and many in
    /// the file, which the reader refuses past 4M vertices: `usd::prepare`
    /// must not promise what it cannot read, so it refuses first, and a
    /// scene of triangles is not touched by the check. The count is of the
    /// vertices of every node that draws the mesh.
    #[test]
    fn deindexing_refuses_the_scene_the_reader_would_refuse() {
        use super::super::scene::{Node, SceneMesh, Topology, MAX_SCENE_VERTICES};
        let scene_of = |indices: usize, uses: usize| -> Scene {
            let prim = Primitive {
                positions: vec![[0.0; 3]],
                indices: vec![0; indices],
                topology: Topology::Points,
                ..Default::default()
            };
            let mut s = Scene::default();
            s.meshes.push(SceneMesh { primitives: vec![prim], ..Default::default() });
            for _ in 0..uses {
                s.nodes.push(Node { mesh: Some(0), ..Default::default() });
            }
            s.roots = (0..uses).collect();
            s
        };
        let mut over = scene_of(MAX_SCENE_VERTICES + 1, 1);
        let e = deindex_scene(&mut over).unwrap_err();
        assert!(matches!(&e, MeshError::Unsupported(m) if m.contains("vertices")), "{e:?}");
        assert_eq!(over.meshes[0].primitives[0].positions.len(), 1, "a refused scene is left as it was");

        let mut by_nodes = scene_of(MAX_SCENE_VERTICES / 2 + 1, 2);
        assert!(deindex_scene(&mut by_nodes).is_err(), "two nodes draw it twice");
        let mut once = scene_of(MAX_SCENE_VERTICES, 1);
        assert!(deindex_scene(&mut once).is_ok(), "exactly the limit is allowed");
        assert_eq!(once.meshes[0].primitives[0].positions.len(), MAX_SCENE_VERTICES);

        // Triangles over one vertex are not de-indexed, and not refused.
        let mut tris = scene_of(MAX_SCENE_VERTICES + 3, 1);
        tris.meshes[0].primitives[0].topology = Topology::Triangles;
        assert!(deindex_scene(&mut tris).is_ok());
        assert_eq!(tris.meshes[0].primitives[0].positions.len(), 1);
    }

    /// `prepare` de-indexes, and `write_layer` does it again on what it is
    /// given: the second pass finds each point and line primitive already one
    /// vertex to an index, in order, and leaves it where it is, without
    /// gathering its arrays into new ones.
    #[test]
    fn deindexing_what_is_already_deindexed_gathers_nothing() {
        use super::super::scene::{Node, SceneMesh, Topology};
        let mut s = Scene::default();
        s.meshes.push(SceneMesh {
            primitives: vec![
                tagged(Topology::Lines, vec![2, 0, 0, 3]),
                tagged(Topology::Points, vec![1, 1, 3]),
                tagged(Topology::Triangles, vec![0, 1, 2, 2, 1, 3]),
            ],
            ..Default::default()
        });
        s.nodes.push(Node { mesh: Some(0), ..Default::default() });
        s.roots = vec![0];
        deindex_scene(&mut s).unwrap();
        let places = |s: &Scene| -> Vec<(*const [f32; 3], usize)> {
            s.meshes[0].primitives.iter().map(|p| (p.positions.as_ptr(), p.positions.len())).collect()
        };
        let (once, before) = (s.clone(), places(&s));
        deindex_scene(&mut s).unwrap();
        assert_eq!(s, once, "nothing changes");
        assert_eq!(places(&s), before, "no array was gathered anew");
        assert_eq!(s.meshes[0].primitives[0].positions.len(), 4, "the lines were de-indexed the first time");
    }

    /// 0.25 % of the diagonal of the rest bounds, and a centimetre (the
    /// writer's unit is the metre) when there is no diagonal to take it of:
    /// no bounds, one point, or numbers that do not make a width.
    #[test]
    fn a_width_is_a_quarter_of_a_percent_of_the_diagonal_or_a_centimetre() {
        let w = point_width(Some(([0.0; 3], [3.0, 4.0, 12.0])));
        assert!((w - 0.0325).abs() < 1e-9, "13 m across: {w}");
        let w = point_width(Some(([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0])));
        assert!((w - 0.0025 * 12.0f32.sqrt()).abs() < 1e-8, "a 2 m cube: {w}");
        for (what, bounds) in [
            ("no bounds", None),
            ("one point", Some(([1.0; 3], [1.0; 3]))),
            ("an infinite corner", Some(([0.0; 3], [f64::INFINITY, 1.0, 1.0]))),
            ("a NaN corner", Some(([f64::NAN; 3], [1.0; 3]))),
            ("a diagonal that overflows", Some(([0.0; 3], [1e300; 3]))),
            ("a width past f32", Some(([0.0; 3], [1e42, 0.0, 0.0]))),
        ] {
            assert_eq!(point_width(bounds), 0.01, "{what}");
        }
    }
}
