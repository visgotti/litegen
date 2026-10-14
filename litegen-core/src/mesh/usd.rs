//! USDZ and USDA — Pixar's Universal Scene Description, packaged and as text.
//!
//! # Why USD, and why these two spellings
//!
//! USDZ is what iOS opens in AR Quick Look and what Apple's tools, Reality
//! Composer and a growing share of 3D vendors ship; a model that should "open on
//! an iPhone" has to be one. USDA is the same scene as plain text: the layer
//! inside a USDZ, what pipelines diff and patch, and the format most USD
//! tooling round-trips.
//!
//! - **Writing** produces USDA text in both cases. The research behind this
//!   module verified that `usdchecker --arkit` accepts a usda root layer (the
//!   rule demanding usdc was removed in OpenUSD 24.03) and that
//!   `<model-viewer>` hands exactly that to Quick Look. A USDZ is a ZIP whose
//!   entries are stored, 64-byte aligned, root layer first, sizes and CRC in
//!   the local headers (see `zip.rs`); textures go in as `textures/<name>`.
//! - **Reading** accepts text and binary crate layers (`usdc.rs`) in
//!   either container, composes them (`usd_stage.rs`) and maps the stage to
//!   the scene (`usd_read.rs`). `.usdc`/`.usd` files read through the USDA
//!   reader too, since a crate file is one layer like a text one.
//!
//! # What a USD file holds (the capabilities row)
//!
//! Hierarchy, normals, two uv sets (RealityKit's limit), vertex colours,
//! UsdPreviewSurface (metallic-roughness PBR), textures (USDZ always; USDA
//! only as a package, beside it), wrap modes, skins, morph targets and one
//! animation. Not texture filters (UsdUVTexture has none) and not texture
//! transforms as such: UsdTransform2d exists, but three.js's USD loader
//! applies it in the wrong order and sense and RealityKit honours one per
//! material, so transforms are baked into the coordinates, which every reader
//! draws right.
//!
//! [`prepare`] reduces a scene to exactly what the writer encodes, beyond the
//! generic reduction: one timeline (USD has no clips), linear keys (USD
//! interpolates nothing else; STEP becomes a 1 ms ramp, CUBICSPLINE is
//! resampled), PNG/JPEG only, no unlit or tangents, no vertex colour under a
//! base-colour texture (UsdPreviewSurface cannot multiply the two), skeletons
//! whose joints are parented to joints, and joint scales at half precision
//! (UsdSkel's `half3[]`). [`losses`] names each of those.
//!
//! # How it is verified
//!
//! Unit tests here round-trip every fixture through both formats, and read
//! files other tools made from our own content (`testdata/usd/README.md`):
//! Pixar's crate writer (every value encoding, and composition arcs), Apple's
//! packager and glTF importer, and three.js's exporter. They fuzz both parsers
//! and the container. The render harness (`scripts/mesh-verify`) renders every
//! USD output with three.js's loader and with `usdrecord` — Pixar's own
//! renderer, the authority where three.js's loader is wrong (its USD bugs are
//! listed there with the source lines) — and runs `usdchecker --arkit` on each.

use super::package::{Package, PackageFile, Sidecars};
use super::scene::{AlphaMode, AnimPath, Capabilities, Interpolation, Loss, LossKind, MaterialSupport, Scene, Transform};
use super::usd_stage::{Layer, Stage};
use super::zip;
use super::{math, pose, MeshError, MeshFormat, SceneRead, WriteOptions};

/// Name of the root layer inside a USDZ and of a USDA package's primary file.
const ROOT_USDA: &str = "model.usda";
/// The STEP ramp: a key this long before each step holds the previous value.
const STEP_EPSILON: f32 = 1e-3;
/// Rate cubic-spline channels and folded skeleton animation are resampled at.
const RESAMPLE_FPS: f32 = 30.0;

/// The capabilities row for USDZ and USDA.
pub fn capabilities(format: MeshFormat, opts: WriteOptions) -> Capabilities {
    Capabilities {
        hierarchy: true,
        normals: true,
        uv_sets: 2,
        vertex_colors: true,
        materials: MaterialSupport::Pbr,
        // USDZ embeds its textures; a USDA can only reference them, so one
        // self-contained file has none.
        textures: format == MeshFormat::Usdz || !opts.self_contained,
        texture_filters: false,
        texture_wrap: true,
        texture_transforms: false,
        skinning: true,
        morph_targets: true,
        animation: true,
        multiple_meshes: true,
    }
}

// ─── reading ────────────────────────────────────────────────────────────────

/// Parse one layer, text or crate, by its magic.
fn parse_layer(bytes: &[u8], id: &str) -> Result<Layer, MeshError> {
    if super::usdc::looks_like_crate(bytes) {
        super::usdc::parse(bytes, id)
    } else if super::usda::looks_like_usda(bytes) {
        super::usda::parse(bytes, id)
    } else {
        Err(MeshError::Malformed(format!("usd: layer {:?} is neither USDA text nor a USDC crate", super::usd_stage::short(id))))
    }
}

fn read_stage(root: Layer, fetch: &mut dyn FnMut(&str) -> Option<Vec<u8>>) -> Result<SceneRead, MeshError> {
    let stage = Stage::compose(root, fetch, parse_layer)?;
    let (scene, warnings) = super::usd_read::read_stage(&stage, fetch)?;
    Ok(SceneRead { scene, warnings })
}

/// Read a USDZ package: its first entry is the root layer, and everything it
/// references (sublayers, references, textures) resolves inside the package.
pub fn read_usdz(bytes: &[u8], _sidecars: &dyn Sidecars) -> Result<SceneRead, MeshError> {
    let not_usdz = |detail: String| MeshError::NotThisFormat { expected: MeshFormat::Usdz, detail };
    if !zip::looks_like_zip(bytes) {
        return Err(not_usdz("a USDZ package is a ZIP archive and these bytes are not one".into()));
    }
    let archive = zip::Archive::parse(bytes)?;
    let first = archive.entries.first().ok_or_else(|| not_usdz("the archive is empty".into()))?;
    let ext = first.name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    if !matches!(ext.as_str(), "usd" | "usda" | "usdc") {
        return Err(not_usdz(format!(
            "the first file in the archive is {:?}; a USDZ package starts with its root USD layer",
            super::usd_stage::short(&first.name)
        )));
    }
    let root_bytes = archive.read(first)?;
    let root = parse_layer(&root_bytes, &first.name)?;
    let compressed = archive.entries.iter().any(|e| !e.is_stored());
    let mut fetch = |path: &str| -> Option<Vec<u8>> { archive.find(path).and_then(|e| archive.read(e).ok()) };
    let mut r = read_stage(root, &mut fetch)?;
    if compressed {
        r.warnings.push("the package compresses its entries, which USD itself refuses to open; it was read anyway".into());
    }
    Ok(r)
}

/// Read a standalone USD layer — USDA text or a USDC crate — resolving the
/// files it references through `sidecars`.
pub fn read_usda(bytes: &[u8], sidecars: &dyn Sidecars) -> Result<SceneRead, MeshError> {
    if zip::looks_like_zip(bytes) {
        return Err(MeshError::NotThisFormat {
            expected: MeshFormat::Usda,
            detail: "these bytes are a ZIP archive (a USDZ package?), not a USD layer".into(),
        });
    }
    let id = if super::usdc::looks_like_crate(bytes) { "model.usdc" } else { ROOT_USDA };
    if !super::usdc::looks_like_crate(bytes) && !super::usda::looks_like_usda(bytes) {
        return Err(MeshError::NotThisFormat {
            expected: MeshFormat::Usda,
            detail: "a USD layer starts with `#usda 1.0` or `PXR-USDC`".into(),
        });
    }
    let root = parse_layer(bytes, id)?;
    let mut fetch = |path: &str| sidecars.get(path);
    read_stage(root, &mut fetch)
}

// ─── writing ────────────────────────────────────────────────────────────────

/// Write a prepared scene as a USDZ package: one file.
pub fn write_usdz(scene: &Scene) -> Result<Package, MeshError> {
    if scene.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    let (text, textures) = super::usd_write::write_layer(scene, super::usd_write::TexturePlacement::Files)?;
    let mut w = zip::Writer::new();
    // Root layer first, every entry stored and 64-byte aligned: the USDZ rules
    // that let USD map the package in place.
    w.add(ROOT_USDA, text.as_bytes(), zip::Store::Stored { align: 64 })?;
    for t in &textures {
        w.add(&t.path, &t.bytes, zip::Store::Stored { align: 64 })?;
    }
    Ok(Package::single("model.usdz", w.finish()))
}

/// Write a prepared scene as a USDA layer; as a package, with its textures
/// beside it.
pub fn write_usda(scene: &Scene, opts: WriteOptions) -> Result<Package, MeshError> {
    if scene.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    let placement = if opts.self_contained { super::usd_write::TexturePlacement::None } else { super::usd_write::TexturePlacement::Files };
    let (text, textures) = super::usd_write::write_layer(scene, placement)?;
    let mut files = vec![PackageFile { path: ROOT_USDA.into(), bytes: text.into_bytes() }];
    files.extend(textures.into_iter().map(|t| PackageFile { path: t.path, bytes: t.bytes }));
    Ok(Package { files })
}

// ─── lossiness ──────────────────────────────────────────────────────────────

/// The scene reduced to exactly what the USD writer encodes. See the module
/// docs for each step and why.
pub fn prepare(scene: &Scene, caps: &Capabilities) -> Result<Scene, MeshError> {
    let mut s = pose::degrade(scene, caps)?;
    s.animations.truncate(1);
    // Only PNG and JPEG travel in a USD package.
    for m in s.materials.iter_mut() {
        let images = &s.images;
        let textures = &s.textures;
        let portable = |r: &Option<super::scene::TextureRef>| {
            r.as_ref().is_some_and(|r| textures.get(r.texture).and_then(|t| images.get(t.image)).is_some_and(|i| i.format.is_universal()))
        };
        for slot in [
            &mut m.base_color_texture,
            &mut m.metallic_roughness_texture,
            &mut m.normal_texture,
            &mut m.occlusion_texture,
            &mut m.emissive_texture,
        ] {
            if slot.is_some() && !portable(slot) {
                *slot = None;
            }
        }
        m.unlit = false;
    }
    pose::prune_unused_textures(&mut s);
    let materials = s.materials.clone();
    for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
        p.tangents = None;
        let mat = p.material.and_then(|i| materials.get(i));
        if mat.is_some_and(|m| m.base_color_texture.is_some()) {
            p.colors = None;
        }
    }
    // Alpha, as the writer spells it and a reader reads it back.
    let mut varies = vec![false; s.materials.len()];
    for p in s.meshes.iter().flat_map(|m| &m.primitives) {
        if let (Some(i), Some(cs)) = (p.material, &p.colors) {
            if cs.iter().any(|c| c[3] < 1.0) {
                varies[i] = true;
            }
        }
    }
    for (i, m) in s.materials.iter_mut().enumerate() {
        let constant_alpha = m.base_color_texture.is_none() && !varies[i];
        let opaque = match m.alpha_mode {
            AlphaMode::Opaque => true,
            AlphaMode::Mask => m.alpha_cutoff <= 0.0 || (constant_alpha && m.base_color[3] >= 1.0),
            AlphaMode::Blend => constant_alpha && m.base_color[3] >= 1.0,
        };
        if opaque {
            m.alpha_mode = AlphaMode::Opaque;
            m.base_color[3] = 1.0;
        }
    }
    let materials = s.materials.clone();
    for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
        let opaque = p.material.and_then(|i| materials.get(i)).is_none_or(|m| m.alpha_mode == AlphaMode::Opaque);
        if let (true, Some(cs)) = (opaque, p.colors.as_mut()) {
            for c in cs.iter_mut() {
                c[3] = 1.0;
            }
        }
    }
    drop_unused_skins(&mut s);
    pose::drop_constant_channels(&mut s);
    linearize_animation(&mut s);
    normalize_skeletons(&mut s)?;
    quantize_joint_scales(&mut s);
    s.validate()?;
    Ok(s)
}

/// Skins no mesh node binds: USD has nothing to write them as (a skeleton
/// exists for the meshes it deforms), and their joints stay plain nodes.
fn drop_unused_skins(s: &mut Scene) {
    let used = super::usd_write::used_skins(s);
    if used.iter().all(|u| *u) {
        return;
    }
    let mut map = vec![None; s.skins.len()];
    let mut kept = Vec::new();
    for (i, sk) in s.skins.drain(..).enumerate() {
        if used[i] {
            map[i] = Some(kept.len());
            kept.push(sk);
        }
    }
    s.skins = kept;
    for n in s.nodes.iter_mut() {
        n.skin = n.skin.and_then(|i| map[i]);
    }
}

/// STEP and CUBICSPLINE channels as the linear keys USD interpolates.
fn linearize_animation(s: &mut Scene) {
    if s.animations.is_empty() {
        return;
    }
    let grid = pose::resample_times(s, 0, RESAMPLE_FPS);
    let targets: Vec<usize> = s.nodes.iter().map(|n| n.mesh.map_or(0, |m| s.meshes[m].target_count())).collect();
    let joints: std::collections::HashSet<usize> = s.skins.iter().flat_map(|k| k.joints.iter().copied()).collect();
    for ch in s.animations[0].channels.iter_mut() {
        let stride = ch.stride(targets[ch.node]);
        match ch.interpolation {
            Interpolation::Linear => {}
            Interpolation::Step => {
                let n = ch.times.len();
                let mut times = Vec::with_capacity(n * 2);
                let mut values = Vec::with_capacity(ch.values.len() * 2);
                for i in 0..n {
                    times.push(ch.times[i]);
                    values.extend_from_slice(&ch.values[i * stride..(i + 1) * stride]);
                    if i + 1 < n {
                        let (a, b) = (ch.times[i], ch.times[i + 1]);
                        let eps = STEP_EPSILON.min((b - a) / 4.0).max(f32::EPSILON * b.abs() * 8.0);
                        let hold = b - eps;
                        if hold > a && hold < b {
                            times.push(hold);
                            values.extend_from_slice(&ch.values[i * stride..(i + 1) * stride]);
                        }
                    }
                }
                ch.times = times;
                ch.values = values;
                ch.interpolation = Interpolation::Linear;
            }
            Interpolation::CubicSpline => {
                let (lo, hi) = (ch.times[0], ch.times[ch.times.len() - 1]);
                let mut times: Vec<f32> = grid.iter().copied().filter(|t| *t >= lo && *t <= hi).collect();
                if times.first() != Some(&lo) {
                    times.insert(0, lo);
                }
                if times.last() != Some(&hi) {
                    times.push(hi);
                }
                times.dedup();
                let values = times.iter().flat_map(|&t| pose::sample_channel(ch, t, stride)).collect();
                ch.times = times;
                ch.values = values;
                ch.interpolation = Interpolation::Linear;
            }
        }
        // What the SkelAnimation carries (joints, blend-shape weights) is
        // keyed from 0, holding the first value. USD holds a first sample
        // anyway, but SceneKit starts a skeleton's clip at its first sample:
        // CesiumMan, keyed from 1/24 s, drew 1/24 s ahead at every frame. An
        // xformOp must NOT get that key: SceneKit then stretches the op's
        // animation (Khronos BoxAnimated, whose rotation starts at 1.25 s,
        // held still until 1.6 s), and without it plays it exactly.
        let skel = joints.contains(&ch.node) || ch.path == AnimPath::Weights;
        if skel && ch.times.first().is_some_and(|t| *t > 0.0) && ch.values.len() >= stride {
            let first = ch.values[..stride].to_vec();
            ch.times.insert(0, 0.0);
            ch.values.splice(0..0, first);
        }
    }
}

/// Parent every joint of a skeleton directly to its nearest joint ancestor
/// (or to the skeleton's anchor), folding the non-joint nodes between them.
///
/// A USD skeleton's joints are only joints: there is nowhere to put a group
/// node between a hip and a knee. Folding a static group is exact; folding an
/// animated one samples the relative motion at the resampling rate.
fn normalize_skeletons(s: &mut Scene) -> Result<(), MeshError> {
    if s.skins.is_empty() {
        return Ok(());
    }
    let sets = super::usd_write::skeleton_sets(s);
    let parents = s.parents();
    let mut moves: Vec<(usize, Option<usize>)> = Vec::new();
    for (_, anchor, members) in &sets {
        for &j in members {
            let mut cur = parents[j];
            let mut target = *anchor;
            let mut steps = 0;
            while let Some(p) = cur {
                if Some(p) == *anchor || members.contains(&p) {
                    target = Some(p);
                    break;
                }
                cur = parents[p];
                steps += 1;
                if steps > s.nodes.len() {
                    return Err(MeshError::Malformed("usd: the node graph is not a tree".into()));
                }
            }
            if parents[j] != target && !moves.iter().any(|(n, _)| *n == j) {
                moves.push((j, target));
            }
        }
    }
    if moves.is_empty() {
        return Ok(());
    }
    let rest_world = s.world_matrices();
    // The nodes animated by the one clip USD keeps (morph weights aside).
    let animated: Vec<usize> = s
        .animations
        .first()
        .map(|a| a.channels.iter().filter(|c| c.path != AnimPath::Weights).map(|c| c.node).collect())
        .unwrap_or_default();
    // A moved joint whose transform relative to its new parent is animated —
    // through its own channels or a folded node's — is resampled at the
    // resampling rate, before the graph changes. Its own channels were
    // relative to its old parent and go either way.
    let mut resampled: Vec<Resampled> = Vec::new();
    let anim_times = if animated.is_empty() { Vec::new() } else { pose::resample_times(s, 0, RESAMPLE_FPS) };
    let worlds: Vec<Vec<math::Mat4>> = anim_times
        .iter()
        .map(|t| super::scene::world_from_locals(s, &pose::sample_pose(s, 0, *t).locals()))
        .collect();
    for &(j, target) in &moves {
        let mut chain = vec![j];
        let mut cur = parents[j];
        while cur != target {
            let Some(p) = cur else { break };
            chain.push(p);
            cur = parents[p];
        }
        if !chain.iter().any(|n| animated.contains(n)) {
            continue;
        }
        let mut r = Resampled { node: j, times: anim_times.clone(), translations: Vec::new(), rotations: Vec::new(), scales: Vec::new() };
        let mut prev: Option<[f64; 4]> = None;
        for w in &worlds {
            let base = target.map_or(math::IDENTITY, |t| w[t]);
            let rel = math::invert(&base)
                .map(|inv| math::mat_mul(&inv, &w[j]))
                .ok_or_else(|| MeshError::Malformed("usd: a skeleton ancestor collapses to a singular transform".into()))?;
            // A key scaled to nothing (pop-in) decomposes; shear has no TRS,
            // and joints carry TRS only.
            let (t, mut q, sv) = math::decompose_near(&rel, prev).ok_or_else(|| {
                MeshError::Unsupported("usd: a folded skeleton group shears a joint, which joint TRS cannot hold".into())
            })?;
            if prev.is_some_and(|pq| math::quat_dot(pq, q) < 0.0) {
                q = [-q[0], -q[1], -q[2], -q[3]];
            }
            prev = Some(q);
            r.translations.extend(t.iter().map(|v| *v as f32));
            r.rotations.extend(q.iter().map(|v| *v as f32));
            r.scales.extend(sv.iter().map(|v| *v as f32));
        }
        resampled.push(r);
    }
    for &(j, target) in &moves {
        // Detach, then attach under the target with the folded rest transform.
        for n in s.nodes.iter_mut() {
            n.children.retain(|c| *c != j);
        }
        s.roots.retain(|r| *r != j);
        match target {
            Some(t) => s.nodes[t].children.push(j),
            None => s.roots.push(j),
        }
        let base = target.map_or(math::IDENTITY, |t| rest_world[t]);
        let rel = math::invert(&base)
            .map(|inv| math::mat_mul(&inv, &rest_world[j]))
            .ok_or_else(|| MeshError::Malformed("usd: a skeleton ancestor has a singular transform".into()))?;
        s.nodes[j].transform = Transform::from_matrix(rel);
        if let Some(a) = s.animations.first_mut() {
            a.channels.retain(|c| c.node != j || c.path == AnimPath::Weights);
        }
    }
    if let Some(a) = s.animations.first_mut() {
        for r in resampled {
            for (path, values) in [(AnimPath::Translation, r.translations), (AnimPath::Rotation, r.rotations), (AnimPath::Scale, r.scales)] {
                a.channels.push(super::scene::Channel { node: r.node, path, interpolation: Interpolation::Linear, times: r.times.clone(), values });
            }
        }
    }
    Ok(())
}

/// A folded joint's motion relative to its new parent, flattened per key.
struct Resampled {
    node: usize,
    times: Vec<f32>,
    translations: Vec<f32>,
    rotations: Vec<f32>,
    scales: Vec<f32>,
}

/// The nearest half-precision value, as UsdSkel stores animated joint scales.
pub fn to_half_precision(x: f32) -> f32 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let a = x.abs().min(65504.0);
    // Halves have 11 significant bits; below 2^-14 they are subnormal with a
    // fixed step of 2^-24.
    let e = a.log2().floor().max(-14.0);
    let step = 2f32.powi(e as i32 - 10);
    let q = (a / step).round_ties_even() * step;
    q.copysign(x)
}

/// Joint scales in animated skeletons as half floats: the SkelAnimation's
/// `half3[] scales` is what every USD reader poses them with.
fn quantize_joint_scales(s: &mut Scene) {
    let Some(anim) = s.animations.first() else { return };
    let sets = super::usd_write::skeleton_sets(s);
    let mut quantize = vec![false; s.nodes.len()];
    for (_, _, members) in &sets {
        let animated = anim.channels.iter().any(|c| c.path != AnimPath::Weights && members.contains(&c.node));
        if animated {
            for &j in members {
                quantize[j] = true;
            }
        }
    }
    for (j, q) in quantize.iter().enumerate() {
        if !q {
            continue;
        }
        if let Some((t, r, sc)) = s.nodes[j].transform.trs() {
            let h = sc.map(|v| to_half_precision(v as f32) as f64);
            if h != sc {
                s.nodes[j].transform = Transform::Trs { translation: t, rotation: r, scale: h };
            }
        }
    }
    if let Some(a) = s.animations.first_mut() {
        for c in a.channels.iter_mut() {
            if c.path == AnimPath::Scale && quantize[c.node] {
                for v in c.values.iter_mut() {
                    *v = to_half_precision(*v);
                }
            }
        }
    }
}

/// What USD loses beyond the generic capability table, named for the caller.
pub fn losses(scene: &Scene, target: &str) -> Vec<Loss> {
    let mut out = Vec::new();
    let mut push = |kind: LossKind, what: String, why: &str| out.push(Loss::new(kind, what, format!("{target} {why}")));
    if scene.animations.len() > 1 {
        let names: Vec<String> = scene.animations.iter().enumerate().skip(1).map(|(i, a)| a.name.clone().unwrap_or_else(|| format!("#{i}"))).collect();
        push(LossKind::Animation, format!("{} animation clip(s) [{}]", names.len(), names.join(", ")), "has one timeline; only the first clip is kept");
    }
    if let Some(a) = scene.animations.first() {
        let cubic = a.channels.iter().filter(|c| c.interpolation == Interpolation::CubicSpline).count();
        if cubic > 0 {
            push(LossKind::Animation, format!("{cubic} cubic-spline channel(s)"), "interpolates linearly; they are resampled at 30 keys per second");
        }
        let step = a.channels.iter().filter(|c| c.interpolation == Interpolation::Step).count();
        if step > 0 {
            push(LossKind::Animation, format!("{step} step channel(s)"), "interpolates linearly; each step becomes a 1 ms ramp");
        }
    }
    let unlit = scene.materials.iter().filter(|m| m.unlit).count();
    if unlit > 0 {
        push(LossKind::Materials, format!("{unlit} unlit material(s)"), "has no unlit surface (UsdPreviewSurface); they are shaded as lit");
    }
    let prims: Vec<&super::scene::Primitive> = scene.meshes.iter().flat_map(|m| &m.primitives).collect();
    if prims.iter().any(|p| p.tangents.is_some()) {
        push(LossKind::Normals, "vertex tangents".into(), "stores no tangents; readers derive them");
    }
    let colored_textured = prims
        .iter()
        .filter(|p| p.colors.is_some() && p.material.and_then(|m| scene.materials.get(m)).is_some_and(|m| m.base_color_texture.is_some()))
        .count();
    if colored_textured > 0 {
        push(
            LossKind::VertexColors,
            format!("vertex colours on {colored_textured} textured primitive(s)"),
            "cannot multiply a base-colour texture by a vertex colour (UsdPreviewSurface has no multiply); the vertex colours are dropped",
        );
    }
    let filtered = scene.textures.iter().filter(|t| t.sampler.mag_filter.is_some() || t.sampler.min_filter.is_some()).count();
    if filtered > 0 {
        push(LossKind::TextureSampling, format!("filter modes of {filtered} texture(s)"), "has no texture filter settings; readers use their defaults");
    }
    let clashing = scene.materials.iter().filter(|m| {
        let refs: Vec<_> = m.texture_refs().map(|(_, r)| r).collect();
        refs.iter().any(|a| refs.iter().any(|b| a.uv_set == b.uv_set && a.transform != b.transform))
    }).count();
    if clashing > 0 {
        push(
            LossKind::TextureTransforms,
            format!("texture transforms of {clashing} material(s)"),
            "bakes texture transforms into texture coordinates, and these apply different transforms to one coordinate set; only the first is kept",
        );
    }
    // Group nodes between a skeleton's joints have no place in a USD
    // skeleton, which holds only joints.
    let parents = scene.parents();
    let folded = super::usd_write::skeleton_sets(scene).iter().any(|(_, anchor, members)| {
        members.iter().any(|&j| parents[j] != *anchor && parents[j].is_some_and(|p| !members.contains(&p)))
    });
    if folded {
        push(LossKind::Hierarchy, "group nodes between skeleton joints".into(), "skeletons hold only joints; those nodes' transforms are folded into the joints below them");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::fixtures;
    use super::super::package::{MemorySidecars, NoSidecars, PackageFile};
    use super::super::scene::{Animation, Channel, Filter, Image, ImageFormat, Material, Node, Primitive, Skin, UvTransform};
    use super::super::usd_stage::Value;
    use super::super::{read_scene, write_scene};
    use super::*;

    const STRESS_USDA: &[u8] = include_bytes!("testdata/usd/stress.usda");
    const STRESS_USDC: &[u8] = include_bytes!("testdata/usd/stress.usdc");
    const COMP_USDA: &[u8] = include_bytes!("testdata/usd/comp.usda");
    const COMP_USDC: &[u8] = include_bytes!("testdata/usd/comp.usdc");
    const COMP_SUB: &[u8] = include_bytes!("testdata/usd/comp_sub.usda");
    const COMP_PAYLOAD: &[u8] = include_bytes!("testdata/usd/comp_payload.usda");
    const COMP_ANIM: &[u8] = include_bytes!("testdata/usd/comp_anim.usda");
    const CUBE_USDC: &[u8] = include_bytes!("testdata/usd/cube.usdc");
    const SKINNED_USDC: &[u8] = include_bytes!("testdata/usd/skinned.usdc");
    const MORPH_USDC: &[u8] = include_bytes!("testdata/usd/morph.usdc");
    const TEXTURED_ARKIT: &[u8] = include_bytes!("testdata/usd/textured_arkit.usdz");
    const TEXTURED_APPLE: &[u8] = include_bytes!("testdata/usd/textured_apple.usdz");
    const SKINNED_APPLE: &[u8] = include_bytes!("testdata/usd/skinned_apple.usdz");
    const TEXTURED_THREE: &[u8] = include_bytes!("testdata/usd/textured_three.usdz");

    // ── evaluation-based comparison ─────────────────────────────────────────

    /// One drawn primitive in world space, with what it looks like.
    #[derive(Debug)]
    struct Drawn {
        positions: Vec<[f32; 3]>,
        normals: Option<Vec<[f32; 3]>>,
        uvs: Vec<Vec<[f32; 2]>>,
        colors: Option<Vec<[f32; 4]>>,
        triangles: Vec<[u32; 3]>,
        material: Option<Material>,
        base_image: Option<Vec<u8>>,
    }

    fn drawn(scene: &Scene, t: Option<f32>) -> Vec<Drawn> {
        let pose = match t {
            Some(t) if !scene.animations.is_empty() => pose::sample_pose(scene, 0, t),
            _ => pose::default_pose(scene),
        };
        pose::world_primitives(scene, &pose)
            .unwrap()
            .into_iter()
            .map(|wp| {
                let material = wp.material.and_then(|m| scene.materials.get(m)).cloned();
                let base_image = material
                    .as_ref()
                    .and_then(|m| m.base_color_texture.as_ref())
                    .and_then(|r| scene.textures.get(r.texture))
                    .and_then(|t| scene.images.get(t.image))
                    .map(|i| i.data.clone());
                Drawn {
                    positions: wp.positions,
                    normals: wp.normals,
                    uvs: wp.uvs,
                    colors: wp.colors,
                    triangles: wp.indices.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
                    material,
                    base_image,
                }
            })
            .collect()
    }

    fn close3(a: &[[f32; 3]], b: &[[f32; 3]], tol: f32) -> Option<String> {
        if a.len() != b.len() {
            return Some(format!("{} vs {} entries", a.len(), b.len()));
        }
        a.iter()
            .zip(b)
            .enumerate()
            .find(|(_, (x, y))| (0..3).any(|k| (x[k] - y[k]).abs() > tol * x[k].abs().max(1.0)))
            .map(|(i, (x, y))| format!("entry {i}: {x:?} vs {y:?}"))
    }

    #[derive(Clone, Copy)]
    struct Check {
        materials: bool,
        textures: bool,
        colors: bool,
        normals: bool,
        uvs: bool,
        /// Also compare coordinates no texture reads (which no renderer
        /// shows, and which a writer may encode any way it likes).
        unread_uvs: bool,
        tol: f32,
    }

    const FULL: Check =
        Check { materials: true, textures: true, colors: true, normals: true, uvs: true, unread_uvs: true, tol: 1e-5 };

    /// The two scenes draw the same thing: primitive by primitive in
    /// traversal order, at rest and at every sample time.
    fn assert_draws_alike(want: &Scene, got: &Scene, times: &[f32], check: Check, ctx: &str) {
        let mut moments: Vec<Option<f32>> = vec![None];
        moments.extend(times.iter().copied().map(Some));
        for t in moments {
            let (a, b) = (drawn(want, t), drawn(got, t));
            assert_eq!(a.len(), b.len(), "{ctx} @ {t:?}: drawn primitive count");
            for (i, (x, y)) in a.iter().zip(&b).enumerate() {
                let at = format!("{ctx} @ {t:?} primitive {i}");
                assert_eq!(x.triangles, y.triangles, "{at}: triangles");
                if let Some(e) = close3(&x.positions, &y.positions, check.tol) {
                    panic!("{at}: positions differ: {e}");
                }
                if check.normals {
                    match (&x.normals, &y.normals) {
                        (Some(p), Some(q)) => {
                            if let Some(e) = close3(p, q, 1e-4) {
                                panic!("{at}: normals differ: {e}");
                            }
                        }
                        (None, None) => {}
                        (p, q) => panic!("{at}: normals present {} vs {}", p.is_some(), q.is_some()),
                    }
                }
                let read = x.material.as_ref().is_some_and(|m| m.texture_refs().next().is_some());
                if check.uvs && (check.unread_uvs || read) {
                    assert_eq!(x.uvs.len(), y.uvs.len(), "{at}: uv set count");
                    for (u, v) in x.uvs.iter().zip(&y.uvs) {
                        let bad = u.iter().zip(v).position(|(p, q)| (p[0] - q[0]).abs() > 1e-6 || (p[1] - q[1]).abs() > 1e-6);
                        assert!(bad.is_none(), "{at}: uv {bad:?}: {:?} vs {:?}", bad.map(|i| u[i]), bad.map(|i| v[i]));
                    }
                }
                if check.colors {
                    match (&x.colors, &y.colors) {
                        (Some(p), Some(q)) => assert!(
                            p.iter().zip(q).all(|(c, d)| (0..4).all(|k| (c[k] - d[k]).abs() < 1e-5)),
                            "{at}: vertex colours differ"
                        ),
                        (None, None) => {}
                        (p, q) => panic!("{at}: vertex colours present {} vs {}", p.is_some(), q.is_some()),
                    }
                }
                if check.materials {
                    let (m, n) = (x.material.clone().unwrap_or_default(), y.material.clone().unwrap_or_default());
                    // A vertex-coloured primitive's base colour rides in its
                    // vertex colours in USD: compare the product.
                    let base = |m: &Material, d: &Drawn| -> [f32; 4] {
                        let c = d.colors.as_ref().and_then(|c| c.first()).copied().unwrap_or([1.0; 4]);
                        [m.base_color[0] * c[0], m.base_color[1] * c[1], m.base_color[2] * c[2], m.base_color[3] * c[3]]
                    };
                    let (bm, bn) = (base(&m, x), base(&n, y));
                    assert!((0..4).all(|k| (bm[k] - bn[k]).abs() < 1e-6), "{at}: base colour {bm:?} vs {bn:?}");
                    assert!((m.metallic - n.metallic).abs() < 1e-6 && (m.roughness - n.roughness).abs() < 1e-6, "{at}: metallic/roughness");
                    assert_eq!(m.alpha_mode, n.alpha_mode, "{at}: alpha mode");
                    assert_eq!(m.double_sided, n.double_sided, "{at}: double-sidedness");
                    assert!(m.emissive.iter().zip(&n.emissive).all(|(p, q)| (p - q).abs() < 1e-6), "{at}: emissive");
                }
                if check.textures {
                    assert_eq!(x.base_image, y.base_image, "{at}: base-colour image bytes");
                }
            }
        }
    }

    fn round(scene: &Scene, fmt: MeshFormat, self_contained: bool) -> (SceneRead, Vec<PackageFile>) {
        let w = write_scene(scene, fmt, WriteOptions { self_contained }).unwrap_or_else(|e| panic!("write {fmt}: {e}"));
        let files = w.package.files.clone();
        let side = MemorySidecars { files: &files };
        let r = read_scene(&files[0].bytes, fmt, &side).unwrap_or_else(|e| panic!("read {fmt}: {e}"));
        (r, files)
    }

    fn names(s: &Scene) -> Vec<Option<String>> {
        s.nodes.iter().map(|n| n.name.clone()).collect()
    }

    // ── our own files ───────────────────────────────────────────────────────

    #[test]
    fn every_fixture_round_trips_through_usdz_and_usda() {
        for (name, fixture) in fixtures::all() {
            for (fmt, single) in [(MeshFormat::Usdz, true), (MeshFormat::Usda, false), (MeshFormat::Usda, true)] {
                let ctx = format!("{name} {fmt}{}", if single { " single" } else { " package" });
                let opts = WriteOptions { self_contained: single };
                let want = fmt.prepare(&fixture, opts).unwrap();
                let (r, _) = round(&fixture, fmt, single);
                let got = &r.scene;
                // Our own files come back node for node, name for name.
                assert_eq!(names(got), names(&want), "{ctx}: nodes");
                assert_eq!(got.roots, want.roots, "{ctx}: roots");
                assert_eq!(got.skins.len(), want.skins.len(), "{ctx}: skins");
                for (a, b) in got.skins.iter().zip(&want.skins) {
                    assert_eq!(a.joints, b.joints, "{ctx}: skin joints");
                    for (p, q) in a.inverse_bind_matrices.iter().zip(&b.inverse_bind_matrices) {
                        assert!(p.iter().zip(q).all(|(x, y)| (x - y).abs() < 1e-9), "{ctx}: inverse bind matrices");
                    }
                }
                for (gn, wn) in got.nodes.iter().zip(&want.nodes) {
                    if let (Some(gm), Some(wm)) = (gn.mesh, wn.mesh) {
                        assert_eq!(got.meshes[gm].target_names, want.meshes[wm].target_names, "{ctx}: morph target names");
                        assert_eq!(got.meshes[gm].weights, want.meshes[wm].weights, "{ctx}: default morph weights");
                    }
                }
                assert_eq!(got.animations.len(), want.animations.len(), "{ctx}: animations");
                if let (Some(a), Some(b)) = (got.animations.first(), want.animations.first()) {
                    assert_eq!(a.name, b.name, "{ctx}: animation name");
                    assert_eq!(a.time_range(), b.time_range(), "{ctx}: clip range");
                }
                let mut times = fixtures::sample_times(&fixture);
                // And between every pair of keys, where interpolation shows.
                if let Some(a) = want.animations.first() {
                    let mut keys: Vec<f32> = a.channels.iter().flat_map(|c| c.times.iter().copied()).collect();
                    keys.sort_by(|x, y| x.partial_cmp(y).unwrap());
                    keys.dedup();
                    times.extend(keys.windows(2).map(|w| (w[0] + w[1]) / 2.0));
                }
                let check = Check { textures: !(fmt == MeshFormat::Usda && single), tol: 2e-5, ..FULL };
                assert_draws_alike(&want, got, &times, check, &ctx);
            }
        }
    }

    #[test]
    fn writing_is_deterministic() {
        for (name, fixture) in fixtures::all() {
            for fmt in [MeshFormat::Usdz, MeshFormat::Usda] {
                let a = write_scene(&fixture, fmt, WriteOptions { self_contained: false }).unwrap();
                let b = write_scene(&fixture, fmt, WriteOptions { self_contained: false }).unwrap();
                assert_eq!(a.package, b.package, "{name} {fmt}");
            }
        }
    }

    #[test]
    fn usdz_follows_the_package_rules() {
        // Stored, data 64-byte aligned, root layer first, sizes and CRC in the
        // local headers with no data descriptors: what USD's own zip reader
        // (which walks local headers, not the central directory) requires.
        let pkg = write_scene(&fixtures::textured(), MeshFormat::Usdz, WriteOptions::default()).unwrap().package;
        assert_eq!(pkg.files.len(), 1);
        let bytes = &pkg.files[0].bytes;
        let mut at = 0usize;
        let mut names = Vec::new();
        while u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) == 0x0403_4b50 {
            let u16_at = |o: usize| u16::from_le_bytes(bytes[at + o..at + o + 2].try_into().unwrap()) as usize;
            let u32_at = |o: usize| u32::from_le_bytes(bytes[at + o..at + o + 4].try_into().unwrap()) as usize;
            assert_eq!(u16_at(6) & 0x0008, 0, "no data descriptor");
            assert_eq!(u16_at(8), 0, "stored");
            let (crc, csize, usize_) = (u32_at(14), u32_at(18), u32_at(22));
            assert_eq!(csize, usize_);
            let name = String::from_utf8(bytes[at + 30..at + 30 + u16_at(26)].to_vec()).unwrap();
            let data = at + 30 + u16_at(26) + u16_at(28);
            assert_eq!(data % 64, 0, "{name} data at {data}");
            assert_eq!(super::super::zip::crc32(&bytes[data..data + csize]) as usize, crc, "{name} crc");
            names.push(name);
            at = data + csize;
        }
        assert_eq!(names, vec!["model.usda".to_string(), "textures/quadrants.png".to_string()]);
    }

    #[test]
    fn a_usda_package_carries_its_textures_and_a_single_file_reports_them_lost() {
        let w = write_scene(&fixtures::skinned(), MeshFormat::Usda, WriteOptions { self_contained: false }).unwrap();
        let paths: Vec<&str> = w.package.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["model.usda", "textures/bands.png"]);
        assert!(String::from_utf8_lossy(&w.package.files[0].bytes).contains("@textures/bands.png@"));
        let one = write_scene(&fixtures::skinned(), MeshFormat::Usda, WriteOptions { self_contained: true }).unwrap();
        assert_eq!(one.package.files.len(), 1);
        assert!(!String::from_utf8_lossy(&one.package.files[0].bytes).contains("@textures"));
        assert!(one.losses.iter().any(|l| l.kind == LossKind::Textures), "{:?}", one.losses);
    }

    #[test]
    fn names_are_sanitised_and_restored() {
        let mut s = fixtures::cube();
        s.nodes[0].name = Some("my part #1 (ünï)".into());
        s.meshes[0].name = Some("my part #1 (ünï)".into());
        s.materials[0].name = Some("red / glossy".into());
        let (r, files) = round(&s, MeshFormat::Usda, true);
        let text = String::from_utf8_lossy(&files[0].bytes);
        assert!(text.contains("def Mesh \"my_part__1__"), "{text}");
        assert_eq!(r.scene.nodes[0].name.as_deref(), Some("my part #1 (ünï)"));
        assert_eq!(r.scene.materials[0].name.as_deref(), Some("red / glossy"));
    }

    // ── what prepare and losses promise ─────────────────────────────────────

    fn scene_with_everything_usd_reduces() -> Scene {
        let mut s = fixtures::textured();
        // A second clip, a step, a cubic, unlit, tangents, filters, and vertex
        // colours under a base-colour texture.
        let ch = |path, interpolation, values: Vec<f32>, times: Vec<f32>| Channel { node: 1, path, interpolation, times, values };
        let mut cubic = Vec::new();
        for v in [0.0f32, 1.0] {
            cubic.extend_from_slice(&[0.0, 0.0, 0.0, v, 0.0, 0.0, 0.0, 0.0, 0.0]);
        }
        s.animations.push(Animation {
            name: Some("First".into()),
            channels: vec![
                ch(AnimPath::Translation, Interpolation::Step, vec![-1.2, 0.0, 0.0, -1.2, 1.0, 0.0], vec![0.0, 1.0]),
                Channel { node: 2, ..ch(AnimPath::Translation, Interpolation::CubicSpline, cubic, vec![0.0, 2.0]) },
            ],
        });
        s.animations.push(Animation {
            name: Some("Second".into()),
            channels: vec![ch(AnimPath::Translation, Interpolation::Linear, vec![0.0; 6], vec![0.0, 1.0])],
        });
        s.materials[1].unlit = true;
        let n = s.meshes[0].primitives[0].positions.len();
        s.meshes[0].primitives[0].tangents = Some(vec![[1.0, 0.0, 0.0, 1.0]; n]);
        s.meshes[0].primitives[0].colors = Some(vec![[0.5, 0.5, 0.5, 1.0]; n]);
        s.textures[0].sampler.mag_filter = Some(Filter::Nearest);
        s
    }

    #[test]
    fn losses_name_everything_usd_reduces() {
        let s = scene_with_everything_usd_reduces();
        let w = write_scene(&s, MeshFormat::Usdz, WriteOptions::default()).unwrap();
        let text: Vec<String> = w.losses.iter().map(|l| format!("{}: {l}", l.kind)).collect();
        for needle in ["Second", "cubic-spline", "step channel", "unlit", "tangents", "vertex colours on 1 textured", "filter modes"] {
            assert!(text.iter().any(|t| t.contains(needle)), "no loss mentions {needle:?}: {text:#?}");
        }
    }

    #[test]
    fn prepare_reduces_to_exactly_what_the_writer_encodes() {
        let s = scene_with_everything_usd_reduces();
        let p = MeshFormat::Usdz.prepare(&s, WriteOptions::default()).unwrap();
        assert_eq!(p.animations.len(), 1, "one timeline");
        let a = &p.animations[0];
        assert!(a.channels.iter().all(|c| c.interpolation == Interpolation::Linear));
        // The step became a hold until just before the key, then a 1 ms ramp.
        let step = a.channels.iter().find(|c| c.node == 1).unwrap();
        assert_eq!(step.times, vec![0.0, 1.0 - STEP_EPSILON, 1.0]);
        assert_eq!(&step.values[3..6], &[-1.2, 0.0, 0.0]);
        // The cubic was resampled through its own curve.
        let cubic = a.channels.iter().find(|c| c.node == 2).unwrap();
        assert!(cubic.times.len() > 30);
        let mid = pose::sample_channel(cubic, 1.0, 3);
        assert!((mid[0] - 0.5).abs() < 1e-6, "{mid:?}");
        assert!(!p.materials[1].unlit);
        let prim = &p.meshes[0].primitives[0];
        assert!(prim.tangents.is_none() && prim.colors.is_none());
        assert!(p.textures.iter().all(|t| t.sampler.mag_filter.is_none()));
        // And what the writer encodes reads back as exactly that.
        let (r, _) = round(&s, MeshFormat::Usdz, true);
        let times: Vec<f32> = (0..=20).map(|i| i as f32 * 0.1).collect();
        assert_draws_alike(&p, &r.scene, &times, FULL, "reduced scene");
    }

    #[test]
    fn half_precision_reaches_animated_joint_scales_only() {
        assert_eq!(to_half_precision(1.25), 1.25);
        assert_eq!(to_half_precision(1.1), 1.099_609_4);
        assert_eq!(to_half_precision(-0.01), -0.010_002_136);
        let mut s = fixtures::skinned();
        if let Transform::Trs { scale, .. } = &mut s.nodes[1].transform {
            *scale = [1.1, 1.1, 1.1];
        }
        let p = MeshFormat::Usdz.prepare(&s, WriteOptions::default()).unwrap();
        let (_, _, sc) = p.nodes[1].transform.trs().unwrap();
        assert_eq!(sc[0] as f32, to_half_precision(1.1));
        let (r, _) = round(&s, MeshFormat::Usdz, true);
        let (_, _, sc) = r.scene.nodes[1].transform.trs().unwrap();
        assert_eq!(sc[0] as f32, to_half_precision(1.1), "the reader sees what USD stores");
    }

    #[test]
    fn texture_transforms_are_baked_into_the_coordinates() {
        let mut s = fixtures::textured();
        let tr = UvTransform { offset: [0.25, -0.5], rotation: 0.6, scale: [2.0, 0.5] };
        s.materials[0].base_color_texture.as_mut().unwrap().transform = Some(tr);
        let (r, _) = round(&s, MeshFormat::Usdz, true);
        let m = &r.scene.materials[0];
        assert!(m.base_color_texture.as_ref().unwrap().transform.is_none());
        let want: Vec<[f32; 2]> = s.meshes[0].primitives[0].uvs[0].iter().map(|uv| tr.apply(*uv)).collect();
        let got = &r.scene.meshes[0].primitives[0].uvs[0];
        assert!(want.iter().zip(got).all(|(a, b)| (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5));
    }

    #[test]
    fn webp_textures_are_dropped_and_reported() {
        let mut s = fixtures::textured();
        s.images[0] = Image { name: Some("w".into()), format: ImageFormat::Webp, data: b"RIFF\0\0\0\0WEBPVP8 ".to_vec() };
        let w = write_scene(&s, MeshFormat::Usdz, WriteOptions::default()).unwrap();
        assert!(w.losses.iter().any(|l| l.kind == LossKind::ImageFormats), "{:?}", w.losses);
        let r = read_scene(&w.package.files[0].bytes, MeshFormat::Usdz, &NoSidecars).unwrap();
        assert!(r.scene.images.is_empty() && r.scene.materials[0].base_color_texture.is_none());
    }

    // ── crate decoding ──────────────────────────────────────────────────────

    #[test]
    fn every_crate_encoding_decodes_to_its_text_original() {
        let text = super::super::usda::parse(STRESS_USDA, "stress.usda").unwrap();
        let crate_ = super::super::usdc::parse(STRESS_USDC, "stress.usdc").unwrap();
        assert_eq!(text.meta_str("defaultPrim"), crate_.meta_str("defaultPrim"));
        assert_eq!(text.meta("customLayerData"), crate_.meta("customLayerData"));
        assert_eq!(text.root_children, crate_.root_children);
        let (a, b) = (&text.specs["/S"], &crate_.specs["/S"]);
        assert_eq!(a.children, b.children);
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.custom_data, b.custom_data);
        let mut checked = 0;
        for (name, prop) in &a.props {
            let other = b.prop(name).unwrap_or_else(|| panic!("crate lacks {name}"));
            match (prop, other) {
                (super::super::usd_stage::Prop::Attr(x), super::super::usd_stage::Prop::Attr(y)) => {
                    assert_eq!(x.type_name, y.type_name, "{name}");
                    let same = |p: &Value, q: &Value| -> bool {
                        match (p.doubles(), q.doubles(), p.as_tuple(), q.as_tuple()) {
                            (Some(m), Some(n), _, _) => m.len() == n.len() && m.iter().zip(n.iter()).all(|(u, v)| (u - v).abs() <= 1e-3 * u.abs().max(1e-3)),
                            (_, _, Some(m), Some(n)) => m.len() == n.len() && m.iter().zip(n).all(|(u, v)| (u - v).abs() <= 1e-3 * u.abs().max(1e-3)),
                            _ => match (p.as_f64(), q.as_f64()) {
                                (Some(u), Some(v)) => (u - v).abs() <= 1e-3 * u.abs().max(1e-3),
                                _ => p == q,
                            },
                        }
                    };
                    match (&x.default, &y.default) {
                        (Some(p), Some(q)) => assert!(same(p, q), "{name}: {p:?} vs {q:?}"),
                        (None, None) => {}
                        (p, q) => panic!("{name}: {p:?} vs {q:?}"),
                    }
                    let (sx, sy) = (x.samples.clone().unwrap_or_default(), y.samples.clone().unwrap_or_default());
                    assert_eq!(sx.len(), sy.len(), "{name} samples");
                    for ((t, p), (u, q)) in sx.iter().zip(&sy) {
                        assert_eq!(t, u, "{name} sample time");
                        assert!(same(p, q), "{name} sample {p:?} vs {q:?}");
                    }
                    assert_eq!(x.connections, y.connections, "{name} connections");
                }
                (super::super::usd_stage::Prop::Rel(x), super::super::usd_stage::Prop::Rel(y)) => assert_eq!(x, y, "{name}"),
                _ => panic!("{name}: attribute vs relationship"),
            }
            checked += 1;
        }
        assert_eq!(checked, 29, "properties compared");
        // The exact values that matter most: integers survive their
        // compression, LUT floats their table, an int8-inlined double3 and a
        // diagonal-inlined matrix their shortcuts.
        let get = |n: &str| b.attr(n).unwrap().default.clone().unwrap();
        assert_eq!(get("ints").ints().unwrap()[20..], [-5, 1_000_000, -70_000, 3]);
        assert_eq!(get("wide").ints().unwrap()[3..5], [5_000_000_000, -5_000_000_000]);
        assert_eq!(get("small"), Value::Tuple(vec![1.0, -2.0, 3.0]));
        assert_eq!(get("ident").as_tuple().unwrap()[15], 1.0);
        assert_eq!(get("moved").as_tuple().unwrap()[12..15], [5.0, -6.0, 7.5]);
        assert_eq!(get("q"), Value::Tuple(vec![0.5, -0.5, 0.5, 0.5]), "quaternions are stored x, y, z, w");
        assert_eq!(get("hlut").floats().unwrap()[..2], [1.25, 0.5]);
        assert_eq!(get("str"), Value::Str("a string with \"quotes\"".into()));
    }

    // ── composition ─────────────────────────────────────────────────────────

    fn comp_sidecars() -> Vec<PackageFile> {
        vec![
            PackageFile { path: "comp_sub.usda".into(), bytes: COMP_SUB.to_vec() },
            PackageFile { path: "comp_payload.usda".into(), bytes: COMP_PAYLOAD.to_vec() },
            PackageFile { path: "comp_anim.usda".into(), bytes: COMP_ANIM.to_vec() },
        ]
    }

    #[test]
    fn composition_matches_what_usdcat_flatten_shows() {
        let side = comp_sidecars();
        for (what, bytes) in [("text", COMP_USDA), ("crate", COMP_USDC)] {
            let r = read_usda(bytes, &MemorySidecars { files: &side }).unwrap_or_else(|e| panic!("{what}: {e}"));
            let s = &r.scene;
            let find = |n: &str| s.nodes.iter().position(|x| x.name.as_deref() == Some(n));
            let node = |n: &str| &s.nodes[find(n).unwrap_or_else(|| panic!("{what}: no node {n}"))];
            // Root opinions beat the sublayer's.
            let (t, _, _) = node("Instance").transform.trs().unwrap();
            assert_eq!(t, [3.0, 0.0, 0.0], "{what}");
            // The reference brought the mesh, with its binding remapped.
            let inst = node("Instance");
            let m = &s.meshes[inst.mesh.unwrap()];
            let mat = &s.materials[m.primitives[0].material.unwrap()];
            assert_eq!(&mat.base_color[..3], &[0.0, 1.0, 0.0], "{what}: remapped material binding");
            let (t, _, _) = node("Lifted").transform.trs().unwrap();
            assert_eq!(t, [0.0, 5.0, 0.0], "{what}: inherited from the class");
            assert!(find("Quad").is_some() && find("Tri").map(|i| s.nodes[i].mesh.is_some()) == Some(true), "{what}: variant");
            let shapes = node("Shapes");
            assert_eq!(shapes.children.len(), 1, "{what}: only the selected variant");
            assert!(node("Unselected").children.is_empty(), "{what}");
            assert!(r.warnings.iter().any(|w| w.contains("\"look\"") && w.contains("no selection")), "{what}: {:?}", r.warnings);
            assert!(find("Inactive").is_none() && find("NotDrawn").is_none() && find("Detached").is_none(), "{what}");
            assert!(find("Payload").is_some() && find("FromSublayer").is_some(), "{what}");
            // The offset reference: keys 0 and 5 at offset 10, scale 2, at 10
            // time codes per second, are seconds 1 and 2.
            let timed = find("Timed").unwrap();
            let ch = r.scene.animations[0].channels.iter().find(|c| c.node == timed).unwrap();
            assert_eq!(ch.times, vec![1.0, 2.0], "{what}");
        }
    }

    #[test]
    fn composition_cycles_and_missing_layers_are_warnings_not_hangs() {
        let text = br#"#usda 1.0
(
    defaultPrim = "A"
    subLayers = [@self.usda@, @missing.usda@]
)
def Xform "A" (
    prepend references = [</A>, @missing.usda@</X>, @self.usda@</A>]
)
{
    def Mesh "M"
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
    }
}
"#;
        let side = vec![PackageFile { path: "self.usda".into(), bytes: text.to_vec() }];
        let started = std::time::Instant::now();
        let r = read_usda(text, &MemorySidecars { files: &side }).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(r.warnings.iter().any(|w| w.contains("missing.usda")), "{:?}", r.warnings);
        assert_eq!(r.scene.triangle_count(), 1);
    }

    #[test]
    fn instancing_cannot_amplify_a_small_file_past_the_vertex_ceiling() {
        // One 1000-point mesh referenced 5000 times is 5M vertices from a
        // ~300 KB file.
        let mut t = String::from("#usda 1.0\ndef Mesh \"Proto\" {\n    int[] faceVertexCounts = [3]\n    int[] faceVertexIndices = [0, 1, 2]\n    point3f[] points = [");
        for i in 0..1000 {
            t.push_str(&format!("({i}, 0, 0), "));
        }
        t.push_str("]\n}\n");
        for i in 0..5000 {
            t.push_str(&format!("def \"I{i}\" (references = </Proto>) {{}}\n"));
        }
        let started = std::time::Instant::now();
        let e = read_usda(t.as_bytes(), &NoSidecars).unwrap_err();
        assert!(matches!(e, MeshError::Unsupported(_)), "{e:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    }

    // ── files written by other tools ────────────────────────────────────────

    #[test]
    fn reads_the_crate_files_usdcat_wrote_from_our_fixtures() {
        for (name, bytes) in [("cube", CUBE_USDC), ("skinned", SKINNED_USDC), ("morph", MORPH_USDC)] {
            let fixture = fixtures::by_name(name).unwrap();
            let want = MeshFormat::Usda.prepare(&fixture, WriteOptions { self_contained: false }).unwrap();
            let r = read_scene(bytes, MeshFormat::Usda, &NoSidecars).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(names(&r.scene), names(&want), "{name}");
            // Textures are sidecars these tests do not supply.
            let check = Check { textures: false, materials: name != "skinned", ..FULL };
            assert_draws_alike(&want, &r.scene, &fixtures::sample_times(&fixture), check, name);
        }
    }

    #[test]
    fn reads_a_usdz_apples_packager_rewrote_with_a_crate_root() {
        let fixture = fixtures::textured();
        let want = MeshFormat::Usdz.prepare(&fixture, WriteOptions::default()).unwrap();
        let r = read_scene(TEXTURED_ARKIT, MeshFormat::Usdz, &NoSidecars).unwrap();
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_draws_alike(&want, &r.scene, &[], FULL, "arkit");
    }

    #[test]
    fn reads_apples_gltf_importer_output() {
        // NodeGraph and Material-interface connections, unflipped st with a
        // UsdTransform2d V-flip per texture (baked back to plain UVs on read),
        // and the mirrored box instanced through an internal reference.
        let fixture = fixtures::textured();
        let want = MeshFormat::Usdz.prepare(&fixture, WriteOptions::default()).unwrap();
        let r = read_scene(TEXTURED_APPLE, MeshFormat::Usdz, &NoSidecars).unwrap();
        assert!(r.scene.materials.iter().all(|m| m.texture_refs().all(|(_, t)| t.transform.is_none())));
        // Apple's panel material never reads displayColor, so USD draws it
        // white and so do we; and with no texture to carry a V-flip, the
        // coordinates on the untextured parts stay as glTF had them, which
        // nothing draws. Otherwise the same picture.
        let check = Check { colors: false, materials: false, unread_uvs: false, ..FULL };
        assert_draws_alike(&want, &r.scene, &[], check, "apple");
        let box_mat = r.scene.materials.iter().find(|m| m.name.as_deref() == Some("box_mat")).unwrap();
        assert!(box_mat.base_color_texture.is_some());
    }

    #[test]
    fn reads_apples_skinned_import_with_its_quirks() {
        let r = read_scene(SKINNED_APPLE, MeshFormat::Usdz, &NoSidecars).unwrap();
        let s = &r.scene;
        // The invisible, proxy-purpose Skeleton still skins.
        assert_eq!(s.skins.len(), 1);
        let joint_names: Vec<_> = s.skins[0].joints.iter().map(|j| s.nodes[*j].name.clone().unwrap()).collect();
        assert_eq!(joint_names, ["hip", "knee", "ankle"], "jointNames name the joints");
        assert!(!s.animations.is_empty());
        // Its hip scale really is zero at t = 0 (the importer read glTF
        // in-tangents as values), and is carried as such, as TRS.
        let hip = s.skins[0].joints[0];
        let (_, _, sc) = s.nodes[hip].transform.trs().unwrap();
        assert_eq!(sc, [0.0; 3]);
        assert!(fixtures::bounds_over_animation(s).is_some());
    }

    #[test]
    fn reads_threejs_exporter_output() {
        // Meshes in external layers, the box referenced twice, materials in a
        // root prim outside the default prim.
        let fixture = fixtures::textured();
        let want = MeshFormat::Usdz.prepare(&fixture, WriteOptions::default()).unwrap();
        let r = read_scene(TEXTURED_THREE, MeshFormat::Usdz, &NoSidecars).unwrap();
        // three.js keeps only displayColor's first value and re-encodes the
        // PNG, so compare the geometry and the coordinates.
        let check = Check { colors: false, materials: false, textures: false, tol: 1e-5, ..FULL };
        assert_draws_alike(&want, &r.scene, &[], check, "three.js");
    }

    // ── hostile input ───────────────────────────────────────────────────────

    #[test]
    fn never_panics_on_truncated_or_corrupted_files() {
        let ours = write_scene(&fixtures::skinned(), MeshFormat::Usdz, WriteOptions::default()).unwrap().package.files[0].bytes.clone();
        let text = write_scene(&fixtures::morph(), MeshFormat::Usda, WriteOptions::default()).unwrap().package.files[0].bytes.clone();
        let cases: [(&str, &[u8], MeshFormat); 6] = [
            ("our usdz", &ours, MeshFormat::Usdz),
            ("our usda", &text, MeshFormat::Usda),
            ("stress crate", STRESS_USDC, MeshFormat::Usda),
            ("skinned crate", SKINNED_USDC, MeshFormat::Usda),
            ("composition crate", COMP_USDC, MeshFormat::Usda),
            ("apple usdz", TEXTURED_ARKIT, MeshFormat::Usdz),
        ];
        for (what, bytes, fmt) in cases {
            // Every byte of the small crate files (where the offsets, counts
            // and tables are dense); elsewhere a stride that still lands in
            // every header, table and count.
            let step = if bytes.len() <= 4096 { 1 } else { bytes.len() / 1000 };
            for cut in (0..bytes.len()).step_by(step) {
                let _ = read_scene(&bytes[..cut], fmt, &NoSidecars);
            }
            for i in (0..bytes.len()).step_by(step) {
                for flip in [0x01u8, 0x80, 0xFF] {
                    let mut v = bytes.to_vec();
                    v[i] ^= flip;
                    let r = read_scene(&v, fmt, &NoSidecars);
                    if let Ok(r) = r {
                        r.scene.validate().unwrap_or_else(|e| panic!("{what}: a corrupted file read as an invalid scene: {e}"));
                    }
                }
            }
        }
    }

    #[test]
    fn readers_reject_other_containers() {
        let mut w = super::super::zip::Writer::new();
        w.add("[Content_Types].xml", b"<Types/>", super::super::zip::Store::Deflated).unwrap();
        w.add("3D/3dmodel.model", b"<model/>", super::super::zip::Store::Deflated).unwrap();
        let three_mf = w.finish();
        assert!(matches!(read_usdz(&three_mf, &NoSidecars), Err(MeshError::NotThisFormat { .. })));
        assert!(matches!(read_usda(&three_mf, &NoSidecars), Err(MeshError::NotThisFormat { .. })));
        let glb = write_scene(&fixtures::cube(), MeshFormat::Glb, WriteOptions::default()).unwrap().package.files[0].bytes.clone();
        assert!(matches!(read_usdz(&glb, &NoSidecars), Err(MeshError::NotThisFormat { .. })));
        assert!(matches!(read_usda(&glb, &NoSidecars), Err(MeshError::NotThisFormat { .. })));
        assert!(read_usdz(TEXTURED_ARKIT, &NoSidecars).is_ok());
        assert!(matches!(read_usda(TEXTURED_ARKIT, &NoSidecars), Err(MeshError::NotThisFormat { .. })));
    }

    // ── what other writers put in USD ───────────────────────────────────────

    fn usda(body: &str) -> String {
        format!("#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n)\ndef Xform \"R\"\n{{\n{body}\n}}\n")
    }

    fn read_text(body: &str, files: &[PackageFile]) -> SceneRead {
        let text = usda(body);
        read_usda(text.as_bytes(), &MemorySidecars { files }).unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    const QUAD: &str = r#"
        int[] faceVertexCounts = [4]
        int[] faceVertexIndices = [0, 1, 2, 3]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)]
    "#;

    #[test]
    fn reads_face_varying_indexed_and_polygonal_meshes() {
        let r = read_text(
            r#"def Mesh "M" {
                int[] faceVertexCounts = [4, 5]
                int[] faceVertexIndices = [0, 1, 2, 3, 1, 4, 5, 6, 2]
                point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0), (2, 0, 0), (2.5, 0.5, 0), (2, 1, 0)]
                normal3f[] normals = [(0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1), (0, 0, 1)] (interpolation = "faceVarying")
                texCoord2f[] primvars:st = [(0, 0), (1, 0), (1, 1), (0, 1)] (interpolation = "faceVarying")
                int[] primvars:st:indices = [0, 1, 2, 3, 0, 1, 1, 2, 3]
            }"#,
            &[],
        );
        let p = &r.scene.meshes[0].primitives[0];
        assert_eq!(p.triangle_count(), 2 + 3, "a quad and a pentagon, fanned");
        // Point 1 is used with uv (1,0) and (0,0): split into two vertices.
        let uses: Vec<[f32; 2]> = p.positions.iter().zip(&p.uvs[0]).filter(|(q, _)| **q == [1.0, 0.0, 0.0]).map(|(_, uv)| *uv).collect();
        assert_eq!(uses.len(), 2, "{uses:?}");
        assert!(uses.contains(&[1.0, 1.0]) && uses.contains(&[0.0, 1.0]), "st (1,0)/(0,0) flip to uv (1,1)/(0,1): {uses:?}");
    }

    #[test]
    fn left_handed_faces_and_holes_are_honoured() {
        let body = |o: &str| format!("def Mesh \"M\" {{ {QUAD} uniform token orientation = \"{o}\" }}");
        let right = read_text(&body("rightHanded"), &[]);
        let left = read_text(&body("leftHanded"), &[]);
        let facet = |s: &Scene| {
            let p = &s.meshes[0].primitives[0];
            let i = &p.indices;
            super::super::ir::Mesh::facet_normal(&[p.positions[i[0] as usize], p.positions[i[1] as usize], p.positions[i[2] as usize]])
        };
        assert!(facet(&right.scene)[2] > 0.0 && facet(&left.scene)[2] < 0.0);
        let holed = read_text(
            r#"def Mesh "M" {
                int[] faceVertexCounts = [3, 3]
                int[] faceVertexIndices = [0, 1, 2, 0, 2, 3]
                point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)]
                int[] holeIndices = [1]
            }"#,
            &[],
        );
        assert_eq!(holed.scene.triangle_count(), 1);
    }

    #[test]
    fn geom_subsets_split_by_material_and_keep_uncovered_faces() {
        let r = read_text(
            r#"def Mesh "M" (prepend apiSchemas = ["MaterialBindingAPI"]) {
                int[] faceVertexCounts = [3, 3, 3]
                int[] faceVertexIndices = [0, 1, 2, 0, 2, 3, 1, 4, 2]
                point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0), (2, 0, 0)]
                uniform token subsetFamily:materialBind:familyType = "nonOverlapping"
                rel material:binding = </R/Red>
                def GeomSubset "G" (prepend apiSchemas = ["MaterialBindingAPI"]) {
                    uniform token elementType = "face"
                    uniform token familyName = "materialBind"
                    int[] indices = [1]
                    rel material:binding = </R/Green>
                }
            }
            def Material "Red" { token outputs:surface.connect = </R/Red/S.outputs:surface>
                def Shader "S" { uniform token info:id = "UsdPreviewSurface"
                    color3f inputs:diffuseColor = (1, 0, 0)
                    token outputs:surface } }
            def Material "Green" { token outputs:surface.connect = </R/Green/S.outputs:surface>
                def Shader "S" { uniform token info:id = "UsdPreviewSurface"
                    color3f inputs:diffuseColor = (0, 1, 0)
                    token outputs:surface } }"#,
            &[],
        );
        let m = &r.scene.meshes[0];
        assert_eq!(m.primitives.len(), 2);
        let color = |p: &Primitive| r.scene.materials[p.material.unwrap()].base_color;
        assert_eq!((m.primitives[0].triangle_count(), color(&m.primitives[0])), (1, [0.0, 1.0, 0.0, 1.0]));
        assert_eq!((m.primitives[1].triangle_count(), color(&m.primitives[1])), (2, [1.0, 0.0, 0.0, 1.0]), "uncovered faces keep the mesh's binding");
    }

    #[test]
    fn reads_preview_surface_networks_as_glb_materials() {
        let png = fixtures::quadrant_png();
        let files = vec![
            PackageFile { path: "tex/c.png".into(), bytes: png.clone() },
            PackageFile { path: "tex/mr.png".into(), bytes: png.clone() },
            PackageFile { path: "tex/n.png".into(), bytes: png.clone() },
        ];
        let r = read_text(
            &format!(
                r#"def Mesh "M" (prepend apiSchemas = ["MaterialBindingAPI"]) {{ {QUAD}
                    texCoord2f[] primvars:st = [(0, 0), (1, 0), (1, 1), (0, 1)] (interpolation = "vertex")
                    rel material:binding = </R/Mat> }}
                def Material "Mat" {{
                    asset inputs:colorTex = @tex/c.png@
                    token outputs:surface.connect = </R/Mat/NG/PS.outputs:surface>
                    def NodeGraph "NG" {{
                        def Shader "PS" {{
                            uniform token info:id = "UsdPreviewSurface"
                            color3f inputs:diffuseColor.connect = </R/Mat/NG/C.outputs:rgb>
                            float inputs:opacity.connect = </R/Mat/NG/C.outputs:a>
                            float inputs:opacityThreshold = 0.4
                            float inputs:roughness.connect = </R/Mat/NG/MR.outputs:g>
                            float inputs:metallic.connect = </R/Mat/NG/MR.outputs:b>
                            normal3f inputs:normal.connect = </R/Mat/NG/N.outputs:rgb>
                            color3f inputs:emissiveColor = (0.5, 0.25, 0)
                            token outputs:surface }}
                        def Shader "R" {{ uniform token info:id = "UsdPrimvarReader_float2"
                            string inputs:varname = "st"
                            float2 outputs:result }}
                        def Shader "C" {{ uniform token info:id = "UsdUVTexture"
                            asset inputs:file.connect = </R/Mat.inputs:colorTex>
                            float2 inputs:st.connect = </R/Mat/NG/R.outputs:result>
                            float4 inputs:scale = (0.5, 0.5, 0.5, 1)
                            token inputs:wrapS = "mirror"
                            token inputs:wrapT = "clamp"
                            float3 outputs:rgb
                            float outputs:a }}
                        def Shader "MR" {{ uniform token info:id = "UsdUVTexture"
                            asset inputs:file = @tex/mr.png@
                            float2 inputs:st.connect = </R/Mat/NG/R.outputs:result>
                            token inputs:sourceColorSpace = "raw"
                            float4 inputs:scale = (1, 0.8, 0.3, 1)
                            float outputs:g
                            float outputs:b }}
                        def Shader "N" {{ uniform token info:id = "UsdUVTexture"
                            asset inputs:file = @tex/n.png@
                            float2 inputs:st.connect = </R/Mat/NG/R.outputs:result>
                            token inputs:sourceColorSpace = "raw"
                            float4 inputs:scale = (1.5, 1.5, 2, 1)
                            float4 inputs:bias = (-0.75, -0.75, -1, 0)
                            float3 outputs:rgb }}
                    }}
                }}"#
            ),
            &files,
        );
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let m = &r.scene.materials[0];
        assert_eq!(m.base_color, [0.5, 0.5, 0.5, 1.0]);
        assert!(m.base_color_texture.is_some());
        assert_eq!(m.alpha_mode, super::super::scene::AlphaMode::Mask);
        assert!((m.alpha_cutoff - 0.4).abs() < 1e-6);
        assert!(m.metallic_roughness_texture.is_some());
        assert!((m.roughness - 0.8).abs() < 1e-6 && (m.metallic - 0.3).abs() < 1e-6);
        assert!(m.normal_texture.is_some() && (m.normal_scale - 0.75).abs() < 1e-6);
        assert_eq!(m.emissive, [0.5, 0.25, 0.0]);
        let t = &r.scene.textures[m.base_color_texture.as_ref().unwrap().texture];
        assert_eq!((t.sampler.wrap_s, t.sampler.wrap_t), (super::super::scene::Wrap::MirroredRepeat, super::super::scene::Wrap::ClampToEdge));
        assert_eq!(r.scene.images.len(), 3, "one image per file");
    }

    #[test]
    fn display_color_counts_only_where_usd_draws_it() {
        let colored = r#"color3f[] primvars:displayColor = [(1, 0, 0), (0, 1, 0), (0, 0, 1), (1, 1, 1)] (interpolation = "vertex")"#;
        // Unbound: Hydra draws displayColor.
        let r = read_text(&format!("def Mesh \"M\" {{ {QUAD} {colored} }}"), &[]);
        assert!(r.scene.meshes[0].primitives[0].colors.is_some());
        // Bound to a material that ignores it: the material is drawn.
        let r = read_text(
            &format!(
                r#"def Mesh "M" (prepend apiSchemas = ["MaterialBindingAPI"]) {{ {QUAD} {colored} rel material:binding = </R/W> }}
                def Material "W" {{ token outputs:surface.connect = </R/W/S.outputs:surface>
                    def Shader "S" {{ uniform token info:id = "UsdPreviewSurface"
                        color3f inputs:diffuseColor = (1, 1, 1)
                        token outputs:surface }} }}"#
            ),
            &[],
        );
        assert!(r.scene.meshes[0].primitives[0].colors.is_none());
    }

    #[test]
    fn reads_skinning_as_usdskel_defines_it() {
        // Joints listed child-first, a mesh-level joint order, a non-identity
        // geomBindTransform and a rigid (constant) binding.
        let r = read_text(
            r#"def SkelRoot "Rig" {
                def Skeleton "Skel" (prepend apiSchemas = ["SkelBindingAPI"]) {
                    uniform token[] joints = ["a/b", "a"]
                    uniform matrix4d[] bindTransforms = [((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 1, 0, 1)), ((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))]
                    uniform matrix4d[] restTransforms = [((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 2, 0, 1)), ((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))]
                }
                def Mesh "Rigid" (prepend apiSchemas = ["SkelBindingAPI"]) {
                    int[] faceVertexCounts = [3]
                    int[] faceVertexIndices = [0, 1, 2]
                    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
                    uniform token[] skel:joints = ["a", "a/b"]
                    int[] primvars:skel:jointIndices = [1] (elementSize = 1
                        interpolation = "constant")
                    float[] primvars:skel:jointWeights = [1] (elementSize = 1
                        interpolation = "constant")
                    matrix4d primvars:skel:geomBindTransform = ((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (10, 0, 0, 1))
                    rel skel:skeleton = </R/Rig/Skel>
                }
            }"#,
            &[],
        );
        let s = &r.scene;
        assert_eq!(s.skins.len(), 1);
        // USD: world = J(a/b) · bind(a/b)⁻¹ · geomBind · p. J(a/b) = rest(a) ·
        // rest(a/b) = translate(0, 2, 0); bind(a/b) = translate(0, 1, 0); so
        // every point moves by (10, 1, 0).
        let d = drawn(s, None);
        assert_eq!(d[0].positions, vec![[10.0, 1.0, 0.0], [11.0, 1.0, 0.0], [10.0, 2.0, 0.0]]);
    }

    #[test]
    fn a_skeleton_outside_a_skelroot_skins_nothing() {
        let r = read_text(
            r#"def Skeleton "Skel" {
                uniform token[] joints = ["a"]
                uniform matrix4d[] bindTransforms = [((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1))]
                uniform matrix4d[] restTransforms = [((1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (5, 0, 0, 1))]
            }
            def Mesh "M" {
                int[] faceVertexCounts = [3]
                int[] faceVertexIndices = [0, 1, 2]
                point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
                int[] primvars:skel:jointIndices = [0, 0, 0] (elementSize = 1
                    interpolation = "vertex")
                float[] primvars:skel:jointWeights = [1, 1, 1] (elementSize = 1
                    interpolation = "vertex")
                rel skel:skeleton = </R/Skel>
            }"#,
            &[],
        );
        assert!(r.scene.skins.is_empty());
        assert!(r.warnings.iter().any(|w| w.contains("SkelRoot")), "{:?}", r.warnings);
        assert_eq!(drawn(&r.scene, None)[0].positions[0], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn reads_every_xform_op_and_converts_up_axis_and_units() {
        let r = read_usda(
            br#"#usda 1.0
(
    metersPerUnit = 0.01
    upAxis = "Z"
)
def Xform "A"
{
    float3 xformOp:translate = (100, 0, 0)
    float3 xformOp:rotateXYZ = (0, 0, 90)
    float3 xformOp:translate:pivot = (0, 100, 0)
    float xformOp:scale = 2
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:translate:pivot", "xformOp:scale", "!invert!xformOp:translate:pivot"]
    def Mesh "M"
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 100, 0), (100, 100, 0), (0, 100, 100)]
    }
}
"#,
            &NoSidecars,
        )
        .unwrap();
        // In stage space: p → T(100,0,0)·Rz(90)·T(pivot)·S(2)·T(-pivot)·p.
        // (0,100,0) stays at the pivot: Rz(90)(0,100,0) = (-100,0,0), + (100,0,0)
        // = (0,0,0). Then Z-up → Y-up and centimetres → metres.
        let d = drawn(&r.scene, None);
        let p = d[0].positions[0];
        assert!(p.iter().all(|v| v.abs() < 1e-5), "{p:?}");
        // (0,100,100): 2·(0,0,100) about the pivot is (0,100,200); rotated,
        // (-100,0,200); moved, (0,0,200) cm, Z up: (0, 2, 0) m.
        let q = d[0].positions[2];
        assert!((q[0]).abs() < 1e-5 && (q[1] - 2.0).abs() < 1e-5 && q[2].abs() < 1e-5, "{q:?}");
    }

    #[test]
    fn euler_animation_is_sampled_the_way_usd_interpolates_it() {
        let r = read_text(
            r#"def Xform "Spin" {
                float3 xformOp:rotateXYZ.timeSamples = {
                    0: (0, 0, 0),
                    24: (0, 0, 170),
                }
                uniform token[] xformOpOrder = ["xformOp:rotateXYZ"]
                def Mesh "M" { int[] faceVertexCounts = [3]
                    int[] faceVertexIndices = [0, 1, 2]
                    point3f[] points = [(1, 0, 0), (2, 0, 0), (1, 1, 0)] }
            }"#,
            &[],
        );
        // USD lerps the angle: at code 12, 85°. A slerp between the two keys
        // would agree here, but the resampling is what makes any stack right.
        let s = &r.scene;
        let at = |t: f32| drawn(s, Some(t))[0].positions[0];
        let p = at(0.5); // 24 tcps default: code 12
        let want = 85f32.to_radians();
        assert!((p[0] - want.cos()).abs() < 2e-3 && (p[1] - want.sin()).abs() < 2e-3, "{p:?}");
    }

    #[test]
    fn a_zero_scale_key_stays_trs() {
        let r = read_text(
            r#"def Xform "Pop" {
                float3 xformOp:scale.timeSamples = {
                    0: (0, 0, 0),
                    10: (1, 1, 1),
                }
                uniform token[] xformOpOrder = ["xformOp:scale"]
                def Mesh "M" { int[] faceVertexCounts = [3]
                    int[] faceVertexIndices = [0, 1, 2]
                    point3f[] points = [(1, 0, 0), (2, 0, 0), (1, 1, 0)] }
            }"#,
            &[],
        );
        assert!(matches!(r.scene.nodes[0].transform, Transform::Trs { scale: [0.0, 0.0, 0.0], .. }));
        let glb = write_scene(&r.scene, MeshFormat::Glb, WriteOptions::default()).unwrap();
        assert!(!String::from_utf8_lossy(&glb.package.files[0].bytes).contains("\"matrix\""));
    }

    #[test]
    fn a_stack_animated_from_zero_scale_is_animation_not_shear() {
        // A part that pops in: its op stack (translate, rotate, scale) is
        // sampled as one matrix, which is singular where the scale is 0. That
        // is a degenerate scale, not shear, and the animation must survive.
        let r = read_text(
            r#"def Xform "Pop" {
                double3 xformOp:translate = (1, 0, 0)
                float3 xformOp:rotateXYZ = (0, 0, 90)
                float3 xformOp:scale.timeSamples = {
                    0: (0, 0, 0),
                    24: (2, 2, 2),
                }
                uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"]
                def Mesh "M" { int[] faceVertexCounts = [3]
                    int[] faceVertexIndices = [0, 1, 2]
                    point3f[] points = [(1, 0, 0), (2, 0, 0), (1, 1, 0)] }
            }"#,
            &[],
        );
        assert!(r.warnings.iter().all(|w| !w.contains("shear")), "{:?}", r.warnings);
        assert_eq!(r.scene.animations.len(), 1, "the pop-in animation was dropped");
        // Halfway (code 12 at 24 tcps): scale 1, so (1,0,0) turns to (0,1,0)
        // and moves to (1,1,0).
        let p = drawn(&r.scene, Some(0.5))[0].positions[0];
        assert!((p[0] - 1.0).abs() < 1e-4 && (p[1] - 1.0).abs() < 1e-4 && p[2].abs() < 1e-4, "{p:?}");
    }

    #[test]
    fn skins_meshes_and_morphs_survive_together() {
        // A skinned mesh that also has morph targets: one skeleton drives both.
        let mut s = fixtures::skinned();
        let n = s.meshes[0].primitives[0].positions.len();
        s.meshes[0].primitives[0].targets.push(super::super::scene::MorphTarget { positions: vec![[0.1, 0.0, 0.0]; n], normals: None });
        s.meshes[0].weights = vec![0.5];
        s.meshes[0].target_names = vec!["fat".into()];
        let want = MeshFormat::Usdz.prepare(&s, WriteOptions::default()).unwrap();
        let (r, _) = round(&s, MeshFormat::Usdz, true);
        assert_eq!(r.scene.meshes[0].target_names, vec!["fat".to_string()]);
        assert_draws_alike(&want, &r.scene, &fixtures::sample_times(&s), Check { tol: 2e-5, ..FULL }, "skinned+morph");
    }

    #[test]
    fn an_animated_group_between_joints_is_folded_into_the_joint() {
        // A USD skeleton has no place for a non-joint node between two joints:
        // prepare folds it, resampling its motion into the child joint.
        let mut s = fixtures::skinned();
        let group = s.nodes.len();
        let q10 = math::quat_from_axis_angle([0.0, 0.0, 1.0], 10f64.to_radians());
        s.nodes.push(Node {
            name: Some("shin_group".into()),
            transform: Transform::Trs { translation: [0.0, 0.2, 0.0], rotation: q10, scale: [1.0; 3] },
            children: vec![2],
            ..Default::default()
        });
        s.nodes[1].children = vec![group];
        let q30 = math::quat_from_axis_angle([0.0, 0.0, 1.0], 30f64.to_radians());
        let values: Vec<f32> = q10.iter().chain(q30.iter()).map(|v| *v as f32).collect();
        s.animations[0].channels.push(Channel { node: group, path: AnimPath::Rotation, interpolation: Interpolation::Linear, times: vec![0.0, 2.0], values });
        // Without the hip's scale keys, which USD stores as halves: what is
        // left to differ is the folding alone.
        s.animations[0].channels.retain(|c| !(c.node == 0 && c.path == AnimPath::Scale));
        s.validate().unwrap();
        let want = MeshFormat::Usdz.prepare(&s, WriteOptions::default()).unwrap();
        assert_eq!(want.parents()[2], Some(1), "the ankle hangs from the knee");
        // Exact on the resampling grid.
        let grid = [0.0, 0.5, 1.0, 1.5, 2.0];
        assert_draws_alike(&s, &want, &grid, Check { tol: 1e-5, ..FULL }, "folded vs original");
        let (r, _) = round(&s, MeshFormat::Usdz, true);
        assert_draws_alike(&want, &r.scene, &grid, Check { tol: 2e-5, ..FULL }, "folded round trip");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn unused_skins_become_plain_nodes() {
        let mut s = fixtures::textured();
        s.skins.push(Skin { name: None, joints: vec![1, 2], inverse_bind_matrices: vec![super::super::math::IDENTITY; 2], skeleton: None });
        let p = MeshFormat::Usdz.prepare(&s, WriteOptions::default()).unwrap();
        assert!(p.skins.is_empty());
        let (r, _) = round(&s, MeshFormat::Usdz, true);
        assert_eq!(names(&r.scene), names(&p));
    }
}
