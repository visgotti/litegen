//! The scene, written as one USDA layer.
//!
//! # Shape of the output, and why
//!
//! ```text
//! def SkelRoot "<scene>"          (Xform when nothing is skinned or morphed)
//!     def Scope "Materials"       UsdPreviewSurface networks
//!     def Xform / Mesh "<node>"   one prim per node, transforms as xformOps
//!         def Mesh "<node>_<k>"   one Mesh per primitive of a multi-part mesh
//!         def Skeleton "<skin>"   at the deepest non-joint ancestor of its joints
//!             def SkelAnimation   joint and blend-shape animation
//!         def Mesh                skinned meshes beside their skeleton
//!     def Skeleton "MorphTargets" joint-less, drives unskinned blend shapes
//! ```
//!
//! - **One Mesh per primitive**, not GeomSubsets: three.js's loader (the
//!   dashboard's viewer) drops every face no subset covers, and per-part Meshes
//!   need no partition bookkeeping. A node with one primitive and no children
//!   IS its Mesh prim, so a typical vendor model round-trips node for node.
//! - **One SkelRoot, the default prim.** UsdSkel only skins and morphs inside a
//!   SkelRoot. Unskinned meshes with morph targets still need a skeleton to
//!   carry their weights, so they bind to one joint-less Skeleton whose
//!   SkelAnimation holds every such weight; blend shapes on an unskinned mesh
//!   apply in its own space and keep its transform (verified with usdrecord).
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
//! - **Time** is written at 30 time codes per second: three.js's USDA parser
//!   ignores `timeCodesPerSecond` and assumes 30, and every other reader honours
//!   the metadata, so 30 is the one rate everyone plays at the right speed.

use std::collections::HashMap;
use std::fmt::Write as _;

use super::math::{self, Mat4};
use super::package;
use super::scene::{AlphaMode, AnimPath, Channel, Material, Primitive, Scene, TextureRef, Transform, Wrap};
use super::usda::{fmt_f32, fmt_f64, quote};
use super::MeshError;

/// Time codes per second of every layer this writer produces.
pub const TIME_CODES_PER_SECOND: f64 = 30.0;
/// Indentation stops growing past this depth: a hostile hierarchy thousands
/// of nodes deep would otherwise make the text quadratic in its depth.
const MAX_INDENT: usize = 48;

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
}

impl Names {
    fn take(&mut self, wanted: &str, fallback: &str) -> String {
        let base = identifier(wanted, fallback);
        let mut name = base.clone();
        let mut n = 1;
        while !self.used.insert(name.clone()) {
            name = format!("{base}_{n}");
            n += 1;
        }
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

fn ancestors(parents: &[Option<usize>], mut n: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut steps = 0;
    while let Some(p) = parents[n] {
        out.push(p);
        n = p;
        steps += 1;
        if steps > parents.len() {
            break;
        }
    }
    out
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

/// For each written skin, in skin order: the anchor (the deepest common
/// ancestor of its joints that is not a joint, `None` for the root) and the
/// USD skeleton's joints — the skin's own plus every joint on the way from
/// them up to the anchor, so that no animated joint is ever folded into
/// another. Shared by the writer and by `usd::prepare`, which must agree on it.
pub fn skeleton_sets(scene: &Scene) -> Vec<(usize, Option<usize>, Vec<usize>)> {
    let used = used_skins(scene);
    let is_joint = skeleton_joints(scene);
    let parents = scene.parents();
    let mut out = Vec::new();
    for (si, skin) in scene.skins.iter().enumerate() {
        if !used[si] || skin.joints.is_empty() {
            continue;
        }
        let chains: Vec<Vec<usize>> = skin
            .joints
            .iter()
            .map(|&j| {
                let mut c = ancestors(&parents, j);
                c.reverse();
                c.push(j);
                c
            })
            .collect();
        let mut common: Vec<usize> = chains[0].clone();
        for c in &chains[1..] {
            let k = common.iter().zip(c).take_while(|(a, b)| a == b).count();
            common.truncate(k);
        }
        while common.last().is_some_and(|n| is_joint[*n]) {
            common.pop();
        }
        let anchor = common.last().copied();
        let mut members = vec![false; scene.nodes.len()];
        for &j in &skin.joints {
            members[j] = true;
            for a in ancestors(&parents, j) {
                if Some(a) == anchor {
                    break;
                }
                if is_joint[a] {
                    members[a] = true;
                }
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
        let anchor_world = anchor.map_or(math::IDENTITY, |a| world[a]);
        let mut paths = Vec::with_capacity(order.len());
        let mut rest = Vec::with_capacity(order.len());
        let mut bind = Vec::with_capacity(order.len());
        for (i, &n) in order.iter().enumerate() {
            let parent_joint = ancestors(parents, n).into_iter().take_while(|a| Some(*a) != anchor).find(|a| members[*a]);
            let name = names.take(scene.nodes[n].name.as_deref().unwrap_or(""), &format!("joint_{i}"));
            let path = match parent_joint {
                Some(p) => format!("{}/{name}", paths[index_of[&p]]),
                None => name,
            };
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
            let b = match skin.joints.iter().position(|j| *j == n) {
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

fn default_material() -> Material {
    // glTF's default material: white, fully metallic, fully rough.
    Material { name: Some("DefaultMaterial".into()), ..Default::default() }
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
    let mut meta = Vec::new();
    if let Some(orig) = original {
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
    let opaque = m.alpha_mode == AlphaMode::Opaque;

    o.open("def Shader \"PreviewSurface\"", &[]);
    o.line("uniform token info:id = \"UsdPreviewSurface\"");
    if vc {
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
    if em_tex.is_some() {
        o.line(&format!("color3f inputs:emissiveColor.connect = <{path}/EmissiveTex.outputs:rgb>"));
    } else if m.emissive != [0.0; 3] {
        o.line(&format!("color3f inputs:emissiveColor = {}", v3(m.emissive)));
    }
    o.line("int inputs:useSpecularWorkflow = 0");
    o.line("token outputs:surface");
    o.close();

    let mut sets: Vec<u32> = [base_tex, mr_tex, normal_tex, occ_tex, em_tex].iter().flatten().map(|r| r.uv_set).collect();
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
    o.close();
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

/// Write `scene` (already reduced by `usd::prepare`) as USDA text, returning
/// the texture files it references.
pub fn write_layer(scene: &Scene, placement: TexturePlacement) -> Result<(String, Vec<TextureFile>), MeshError> {
    scene.validate()?;
    let scene = &super::glb::hoist_skinned_meshes(scene);
    let parents = scene.parents();
    let world = scene.world_matrices();
    let is_joint = skeleton_joints(scene);
    let mut names = Names::default();
    let anim = scene.animations.first();
    let channels: Vec<&Channel> = anim.map(|a| a.channels.iter().collect()).unwrap_or_default();

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
            while taken.contains(&name) {
                name = format!("textures/{stem}_{n}.{}", im.format.extension());
                n += 1;
            }
            taken.push(name.clone());
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
            if !needs[n] || skinned_mesh_node[n] {
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
    let codes: Vec<f32> = channels.iter().flat_map(|c| c.times.iter().copied()).collect();
    if let (Some(lo), Some(hi)) = (codes.iter().copied().reduce(f32::min), codes.iter().copied().reduce(f32::max)) {
        o.line(&format!("endTimeCode = {}", time_code(hi)));
        o.line(&format!("framesPerSecond = {}", TIME_CODES_PER_SECOND));
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
                if !needs[n] || skinned_mesh_node[n] {
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
                        let mut meta = Vec::new();
                        if let Some(orig) = node.name.as_deref().filter(|o| *o != name) {
                            meta.push(format!("customData = {{ string {} = {} }}", super::usd_read::NAME_KEY, quote(orig)));
                        }
                        o.open(&format!("def Xform {}", quote(&name)), &meta);
                        ctx.write_xform(&mut o, n);
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

impl Ctx<'_> {
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
        o.open(&format!("def Mesh {}", quote(name)), &meta);
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
        }
        o.line(&format!("float3[] extent = [{}, {}]", v3(lo), v3(hi)));
        let tris = p.indices.len() / 3;
        let mut counts = String::with_capacity(tris * 3 + 2);
        counts.push('[');
        for i in 0..tris {
            counts.push_str(if i == 0 { "3" } else { ", 3" });
        }
        counts.push(']');
        o.line(&format!("int[] faceVertexCounts = {counts}"));
        o.line(&format!("int[] faceVertexIndices = {}", list(&p.indices, |i| i.to_string())));
        o.line(&format!("point3f[] points = {}", list(&p.positions, |v| v3(*v))));
        if let Some(ns) = &p.normals {
            o.line(&format!("normal3f[] normals = {} (", list(ns, |v| v3(*v))));
            o.line("    interpolation = \"vertex\"");
            o.line(")");
        }
        for (si, set) in p.uvs.iter().enumerate().take(2) {
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
        o.line("uniform token orientation = \"rightHanded\"");
        o.line("uniform token subdivisionScheme = \"none\"");
        if material.is_some_and(|m| m.double_sided) {
            o.line("uniform bool doubleSided = 1");
        }
        let key = MatKey { material: p.material, vertex_color: p.colors.is_some() };
        if let Some(mn) = self.mat_names.get(&key) {
            o.line(&format!("rel material:binding = <{}/{mn}>", self.materials_path));
        }
        for (ti, t) in p.targets.iter().enumerate() {
            o.open(&format!("def BlendShape {}", quote(&shapes[ti])), &[]);
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
        // Joint animation: every channel on a joint of this skeleton.
        let joint_chans: Vec<&Channel> = self.channels.iter().copied().filter(|c| l.joints.contains(&c.node) && c.path != AnimPath::Weights).collect();
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
        let has_anim = !joint_chans.is_empty() || !weight_sources.is_empty();
        o.open(&format!("def Skeleton {}", quote(&name)), &["prepend apiSchemas = [\"SkelBindingAPI\"]".to_string()]);
        o.line(&format!("uniform token[] joints = {}", list(&l.paths, |p| quote(p))));
        o.line(&format!("uniform matrix4d[] bindTransforms = {}", list(&l.bind, matrix)));
        o.line(&format!("uniform matrix4d[] restTransforms = {}", list(&l.rest, matrix)));
        let anim_prim = if has_anim { Some(self.names.take(self.anim_name, "Animation")) } else { None };
        if let Some(a) = &anim_prim {
            o.line(&format!("rel skel:animationSource = <{path}/{a}>"));
        }
        if let Some(a) = &anim_prim {
            o.open(&format!("def SkelAnimation {}", quote(a)), &[]);
            if !joint_chans.is_empty() {
                o.line(&format!("uniform token[] joints = {}", list(&l.paths, |p| quote(p))));
                let times = channel_times(&joint_chans);
                let rest: Vec<Trs> = l.joints.iter().map(|&n| trs_at(scene, &[], n, 0.0)).collect();
                o.line(&format!("float3[] translations = {}", list(&rest, |r| v3(r.0.map(|v| v as f32)))));
                o.line(&format!("quatf[] rotations = {}", list(&rest, |r| quat(math::quat_normalize(r.1).map(|v| v as f32)))));
                o.line(&format!("half3[] scales = {}", list(&rest, |r| v3(r.2.map(|v| v as f32)))));
                let mut samples: Vec<Vec<Trs>> = times.iter().map(|&t| l.joints.iter().map(|&n| trs_at(scene, &joint_chans, n, t)).collect()).collect();
                // Neighbouring keys on the same hemisphere, for readers that
                // do not take the short way round themselves.
                for j in 0..l.joints.len() {
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
                write_samples(o, "half3[] scales", &times, |i| list(&samples[i], |r| v3(r.2.map(|v| v as f32))));
            } else {
                o.line("uniform token[] joints = []");
            }
            if !weight_sources.is_empty() {
                self.write_weights(o, &shape_tokens_all, &weight_sources);
            }
            o.close();
        }
        o.close();
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

    fn write_weights(&self, o: &mut Out, tokens: &[String], sources: &[(usize, usize)]) {
        let scene = self.scene;
        o.line(&format!("uniform token[] blendShapes = {}", list(tokens, |t| quote(t))));
        let rest: Vec<f32> = sources.iter().flat_map(|(n, m)| mesh_weights(scene, *n, *m)).collect();
        o.line(&format!("float[] blendShapeWeights = {}", list(&rest, |w| fmt_f32(*w))));
        let wchans: Vec<&Channel> = self.channels.iter().copied().filter(|c| c.path == AnimPath::Weights && sources.iter().any(|(n, _)| *n == c.node)).collect();
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
        let anim = self.names.take(self.anim_name, "Animation");
        o.open(&format!("def Skeleton {}", quote(name)), &["prepend apiSchemas = [\"SkelBindingAPI\"]".to_string()]);
        o.line("uniform token[] joints = []");
        o.line("uniform matrix4d[] bindTransforms = []");
        o.line("uniform matrix4d[] restTransforms = []");
        o.line(&format!("rel skel:animationSource = <{path}/{anim}>"));
        o.open(&format!("def SkelAnimation {}", quote(&anim)), &[]);
        o.line("uniform token[] joints = []");
        self.write_weights(o, &tokens, &sources);
        o.close();
        o.close();
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
}
