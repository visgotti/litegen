//! The scene-level conversion matrix: every fixture, every direction.
//!
//! `matrix_tests.rs` checks the flat geometry path. This checks the whole
//! scene — hierarchy, materials, textures, skins, morph targets, animation —
//! through every pair of containers in `MeshFormat::ALL`, so a new format gets
//! a full row and column of coverage the moment it is added there.
//!
//! What a conversion must produce is not "the source": it is the source as the
//! source container encodes it, re-encoded by the target —
//! `to.prepare(from.prepare(fixture))`, the same expectation the render
//! harness (`scripts/mesh-verify`) draws. Pixel comparison lives there; here,
//! in plain `cargo test`, the scenes are compared through invariants that any
//! geometric mistake disturbs:
//!
//! - triangle count, surface area and bounds of the posed, world-space model;
//! - its signed volume `Σ v0·(v1×v2)/6`, which changes sign under a mirror or a
//!   reversed winding even for open surfaces;
//! - its area-weighted centroid, which moves under any misplaced part;
//!
//! evaluated in the default pose AND at every animation sample time, so a
//! skin bound wrongly, a morph weight scaled wrongly or a keyframe misread
//! shows up as a moved centroid or a changed area. Then the feature counts:
//! animation, skins, morph targets, textures and materials must survive exactly
//! where the capabilities say they do; so must each drawn primitive's uv-set
//! count and its texture bindings (slot, set), and the multiuv fixtures' base
//! colour, which samples their set 3, must survive wherever textures do. Each
//! material layer (`fixtures::layered` has them all) survives exactly where
//! both containers' `LayerSupport` keeps it, and the writes' losses name it
//! once wherever it does not; and every drawn material's layers equal the
//! expected scene's, field by field and f32-exact, their maps compared by
//! image bytes, uv set and transform (Ruling A46). Lights and cameras
//! (`fixtures::staged` has both) are counted alike: all the fixture's survive
//! where both containers keep them and none elsewhere, and the writes name
//! each kind once wherever they go. Where they survive, each one's world
//! frame and values equal the expected scene's at rest and at every sample
//! time, within 1e-5 (`fixtures::lightcam`). Points and lines (`fixtures::wire`
//! has both) are counted alike, by primitive, and where they survive their
//! world-space bounds and centroids equal the expected scene's at rest and at
//! every sample time; the triangle metrics above are of the triangles alone.
//!
//! Ruling IDs ("Ruling A46") are mesh-gaps item 4's rulings, listed with
//! their costs in the status block of
//! `docs/superpowers/plans/2026-11-04-mesh-material-layers.md`.

use super::fixtures;
use super::package::PackageFile;
use super::pose;
use super::scene::{AnimPath, LightKind, Material, MaterialLayer, MaterialSupport, Scene, Topology};
use super::*;

struct Shape {
    triangles: usize,
    area: f64,
    volume: f64,
    lo: [f64; 3],
    hi: [f64; 3],
    centroid: [f64; 3],
}

/// The pose a comparison is made in: at rest, or `anim_time` into the first clip.
fn pose_at(scene: &Scene, anim_time: Option<f32>) -> pose::Pose {
    match anim_time {
        Some(t) if !scene.animations.is_empty() => pose::sample_pose(scene, 0, t),
        _ => pose::default_pose(scene),
    }
}

/// The metrics of the triangles alone: lines and points have no area or
/// volume, and their positions are [`extent`]'s.
fn shape(scene: &Scene, anim_time: Option<f32>) -> Shape {
    let pose = pose_at(scene, anim_time);
    let mut s = Shape { triangles: 0, area: 0.0, volume: 0.0, lo: [f64::INFINITY; 3], hi: [f64::NEG_INFINITY; 3], centroid: [0.0; 3] };
    for wp in pose::world_primitives(scene, &pose).expect("pose").iter().filter(|wp| wp.topology == Topology::Triangles) {
        for t in wp.indices.chunks_exact(3) {
            let p = |i: u32| {
                let v = wp.positions[i as usize];
                [v[0] as f64, v[1] as f64, v[2] as f64]
            };
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
            let area = 0.5 * (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            s.triangles += 1;
            s.area += area;
            s.volume += (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0;
            for k in 0..3 {
                s.centroid[k] += area * (a[k] + b[k] + c[k]) / 3.0;
                for v in [a, b, c] {
                    s.lo[k] = s.lo[k].min(v[k]);
                    s.hi[k] = s.hi[k].max(v[k]);
                }
            }
        }
    }
    if s.area > 0.0 {
        for k in 0..3 {
            s.centroid[k] /= s.area;
        }
    }
    s
}

fn close(a: f64, b: f64, scale: f64) -> bool {
    // Text formats round-trip floats exactly here (shortest-repr writers), and
    // binary ones are exact; what remains is f32 transform composition and the
    // resampling some containers impose on animation.
    (a - b).abs() <= 2e-3 * scale.max(1e-6)
}

fn assert_same_shape(want: &Shape, got: &Shape, ctx: &str) {
    assert_eq!(got.triangles, want.triangles, "{ctx}: triangle count");
    let scale = (0..3).map(|k| want.hi[k] - want.lo[k]).fold(0.0f64, f64::max);
    assert!(close(got.area, want.area, scale * scale), "{ctx}: area {} vs {}", got.area, want.area);
    assert!(
        close(got.volume, want.volume, scale * scale * scale),
        "{ctx}: signed volume {} vs {} (a mirror or reversed winding flips it)",
        got.volume,
        want.volume
    );
    for k in 0..3 {
        assert!(close(got.lo[k], want.lo[k], scale) && close(got.hi[k], want.hi[k], scale), "{ctx}: bounds on axis {k}");
        assert!(close(got.centroid[k], want.centroid[k], scale), "{ctx}: centroid on axis {k}");
    }
}

/// Where the vertices a topology draws stand: their bounds, and their mean,
/// each index counted once (a vertex a loop's segments share counts for
/// each of them, so the same segments give the same mean however a
/// container numbers them).
struct Extent {
    count: usize,
    lo: [f64; 3],
    hi: [f64; 3],
    centroid: [f64; 3],
}

/// `topology`'s [`Extent`] in `scene` at `anim_time`, `None` where it draws none.
fn extent(scene: &Scene, anim_time: Option<f32>, topology: Topology) -> Option<Extent> {
    let mut e = Extent { count: 0, lo: [f64::INFINITY; 3], hi: [f64::NEG_INFINITY; 3], centroid: [0.0; 3] };
    for wp in pose::world_primitives(scene, &pose_at(scene, anim_time)).expect("pose").iter().filter(|wp| wp.topology == topology) {
        for &i in &wp.indices {
            let v = wp.positions[i as usize];
            e.count += 1;
            for k in 0..3 {
                e.lo[k] = e.lo[k].min(f64::from(v[k]));
                e.hi[k] = e.hi[k].max(f64::from(v[k]));
                e.centroid[k] += f64::from(v[k]);
            }
        }
    }
    (e.count > 0).then(|| {
        e.centroid = e.centroid.map(|c| c / e.count as f64);
        e
    })
}

fn assert_same_extent(want: &Extent, got: &Extent, ctx: &str) {
    assert_eq!(got.count, want.count, "{ctx}: vertices drawn");
    let scale = (0..3).map(|k| want.hi[k] - want.lo[k]).fold(0.0f64, f64::max);
    for k in 0..3 {
        assert!(close(got.lo[k], want.lo[k], scale) && close(got.hi[k], want.hi[k], scale), "{ctx}: bounds on axis {k}");
        assert!(close(got.centroid[k], want.centroid[k], scale), "{ctx}: centroid on axis {k}");
    }
}

/// Each drawn primitive's uv-set count, and its material's texture bindings
/// as (slot, uv set), sorted. Counted per drawn primitive
/// (`pose::world_primitives`), so a container that writes every instance
/// inline counts the same as one that references a mesh; sorted, so neither
/// primitive nor material order matters.
fn uv_layout(s: &Scene) -> Vec<(usize, Vec<(&'static str, u32)>)> {
    let mut out: Vec<(usize, Vec<(&'static str, u32)>)> = pose::world_primitives(s, &pose::default_pose(s))
        .expect("pose")
        .iter()
        .map(|wp| {
            let mut bindings: Vec<(&'static str, u32)> = wp
                .material
                .and_then(|m| s.materials.get(m))
                .map(|m| m.texture_refs().map(|(slot, r)| (slot.as_str(), r.uv_set)).collect())
                .unwrap_or_default();
            bindings.sort_unstable();
            (wp.uvs.len(), bindings)
        })
        .collect();
    out.sort_unstable();
    out
}

/// Each drawn primitive's material layers, the nine of them field by field:
/// a material holding only its layers, every map a layer samples named by
/// its image's bytes (FNV-1a), not by a texture index a writer may renumber,
/// and keeping its uv set and transform. Sorted, so neither primitive nor
/// material order matters.
fn layer_values(s: &Scene) -> Vec<Material> {
    let image_of = |t: usize| -> usize {
        let data = s.textures.get(t).and_then(|t| s.images.get(t.image)).map_or(&[][..], |i| i.data.as_slice());
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for b in data {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h as usize
    };
    let mut out: Vec<Material> = pose::world_primitives(s, &pose::default_pose(s))
        .expect("pose")
        .iter()
        .map(|wp| {
            let m = wp.material.and_then(|m| s.materials.get(m)).cloned().unwrap_or_default();
            let mut layers = Material {
                clearcoat: m.clearcoat.clone(),
                transmission: m.transmission.clone(),
                volume: m.volume.clone(),
                ior: m.ior,
                specular: m.specular.clone(),
                sheen: m.sheen.clone(),
                iridescence: m.iridescence.clone(),
                anisotropy: m.anisotropy.clone(),
                dispersion: m.dispersion,
                ..Default::default()
            };
            assert!(MaterialLayer::ALL.into_iter().all(|l| layers.has_layer(l) == m.has_layer(l)), "a layer is missing from this copy");
            for (_, r) in layers.texture_refs_mut() {
                r.texture = image_of(r.texture);
            }
            layers
        })
        .collect();
    out.sort_by_cached_key(|m| format!("{m:?}"));
    out
}

/// A set's distinct coordinates, sorted: the same whatever vertex order or
/// welding a reader chooses.
fn uv_values(set: &[[f32; 2]]) -> Vec<[i64; 2]> {
    let mut v: Vec<[i64; 2]> = set.iter().map(|uv| uv.map(|x| (f64::from(x) * 65536.0).round() as i64)).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Write, then read back through the package's own files; with the losses
/// the write reported.
fn written(scene: &Scene, fmt: MeshFormat, opts: WriteOptions) -> (Scene, Vec<Loss>) {
    let w = write_scene(scene, fmt, opts).unwrap_or_else(|e| panic!("write {fmt}: {e}"));
    let files: Vec<PackageFile> = w.package.files.clone();
    let side = super::package::MemorySidecars { files: &files };
    (read_scene(&files[0].bytes, fmt, &side).unwrap_or_else(|e| panic!("read {fmt}: {e}")).scene, w.losses)
}

/// [`written`]'s scene alone.
fn round(scene: &Scene, fmt: MeshFormat, opts: WriteOptions) -> Scene {
    written(scene, fmt, opts).0
}

/// Whether a loss's `what` names every light or camera of a scene, as
/// `scene::losses` words it: a count, then `noun` (" light(s) [" or
/// " camera(s) [") and their names.
fn names_them_all(what: &str, noun: &str) -> bool {
    what.split_once(noun).is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn check_pair(name: &str, fixture: &Scene, from: MeshFormat, to: MeshFormat, opts: WriteOptions) {
    let ctx = format!("{name}: {from} -> {to}{}", if opts.self_contained { " (single file)" } else { "" });
    let expected = from
        .prepare(fixture, opts)
        .and_then(|s| to.prepare(&s, opts))
        .unwrap_or_else(|e| panic!("{ctx}: prepare: {e}"));
    let (source, mut losses) = written(fixture, from, opts);
    let (got, more) = written(&source, to, opts);
    losses.extend(more);

    let mut times: Vec<Option<f32>> = vec![None];
    if !expected.animations.is_empty() {
        times.extend(fixtures::sample_times(fixture).into_iter().map(Some));
    }
    for t in &times {
        assert_same_shape(&shape(&expected, *t), &shape(&got, *t), &format!("{ctx} @ {t:?}"));
    }

    let caps = from.capabilities(opts).intersect(to.capabilities(opts));
    assert_eq!(got.animations.is_empty(), expected.animations.is_empty(), "{ctx}: animation presence");
    if caps.animation {
        for (a, b) in got.animations.iter().zip(&expected.animations) {
            let (ra, rb) = (a.time_range(), b.time_range());
            // The end exactly; the start too, unless the writer keyed the
            // clip from 0, holding its first pose (`pose::resample_times`).
            assert!(
                ra.zip(rb).is_some_and(|(x, y)| (x.1 - y.1).abs() < 1e-3 && ((x.0 - y.0).abs() < 1e-3 || (x.0 == 0.0 && y.0 > 0.0))),
                "{ctx}: clip duration {ra:?} vs {rb:?}"
            );
        }
    }
    assert_eq!(got.skins.is_empty(), expected.skins.is_empty(), "{ctx}: skin presence");
    let morphs = |s: &Scene| s.meshes.iter().map(|m| m.target_count()).sum::<usize>();
    assert_eq!(morphs(&got) > 0, morphs(&expected) > 0, "{ctx}: morph target presence");
    let tex = |s: &Scene| s.materials.iter().filter(|m| m.base_color_texture.is_some()).count() > 0;
    assert_eq!(tex(&got), tex(&expected), "{ctx}: base-colour texture presence");
    if caps.materials >= MaterialSupport::ColorOnly {
        assert!(!got.materials.is_empty() || expected.materials.is_empty(), "{ctx}: materials lost");
    }
    let colors = |s: &Scene| s.meshes.iter().flat_map(|m| &m.primitives).any(|p| p.colors.is_some());
    assert_eq!(colors(&got), colors(&expected), "{ctx}: vertex colour presence");

    // UV sets and texture bindings: each drawn primitive keeps as many sets as
    // expected, never more than both containers hold, and its material samples
    // the same slots from the same sets.
    let (want, have) = (uv_layout(&expected), uv_layout(&got));
    let counts = |l: &[(usize, Vec<(&str, u32)>)]| {
        let mut c: Vec<usize> = l.iter().map(|(n, _)| *n).collect();
        c.sort_unstable();
        c
    };
    assert_eq!(counts(&have), counts(&want), "{ctx}: uv sets per drawn primitive");
    assert!(have.iter().chain(&want).all(|(n, _)| *n <= caps.uv_sets), "{ctx}: more than {} uv set(s): {have:?}", caps.uv_sets);
    let bindings = |l: &[(usize, Vec<(&'static str, u32)>)]| {
        let mut b: Vec<Vec<(&'static str, u32)>> = l.iter().map(|(_, b)| b.clone()).collect();
        b.sort();
        b
    };
    assert_eq!(bindings(&have), bindings(&want), "{ctx}: texture bindings (slot, uv set) per drawn primitive");
    // The multiuv fixtures' base colour samples set 3, first in its
    // material's order, so every pair that carries textures keeps it, on the
    // fixture's set-3 coordinates.
    if name.starts_with("multiuv") && caps.textures {
        let set3 = uv_values(&fixture.meshes[0].primitives[0].uvs[3]);
        for (label, s) in [("expected", &expected), ("got", &got)] {
            for wp in pose::world_primitives(s, &pose::default_pose(s)).expect("pose") {
                let r = wp.material.and_then(|m| s.materials.get(m)).and_then(|m| m.base_color_texture.as_ref());
                let r = r.unwrap_or_else(|| panic!("{ctx}: the {label} scene lost the base-colour binding"));
                let set = wp.uvs.get(r.uv_set as usize).unwrap_or_else(|| panic!("{ctx}: {label}: no uv set {}", r.uv_set));
                assert_eq!(uv_values(set), set3, "{ctx}: the {label} base colour samples the fixture's set 3");
            }
        }
    }

    // Material layers: each survives exactly where both containers keep it,
    // and the two writes name each of the others once, as a materials loss.
    if name == "layered" {
        let missing: Vec<&str> =
            MaterialLayer::ALL.into_iter().filter(|&l| !fixture.materials.iter().any(|m| m.has_layer(l))).map(MaterialLayer::as_str).collect();
        assert!(missing.is_empty(), "{ctx}: the fixture has no {missing:?} layer to test");
    }
    for layer in MaterialLayer::ALL {
        let has = |s: &Scene| s.materials.iter().any(|m| m.has_layer(layer));
        let (kept, lost) = (has(fixture) && caps.layers.keeps(layer), has(fixture) && !caps.layers.keeps(layer));
        assert_eq!(has(&expected), kept, "{ctx}: the expected scene's {} layer", layer.as_str());
        assert_eq!(has(&got), kept, "{ctx}: the {} layer", layer.as_str());
        let prefix = format!("{} on ", layer.as_str());
        let named = losses.iter().filter(|l| l.kind == LossKind::Materials && l.what.starts_with(&prefix)).count();
        assert_eq!(named, usize::from(lost), "{ctx}: the {} layer named {named} time(s): {losses:?}", layer.as_str());
    }
    // And what each kept layer holds: every codec's `prepare` models what it
    // reads back, so the values come through as the expected scene has them,
    // in every pair (Ruling A46).
    assert_eq!(layer_values(&got), layer_values(&expected), "{ctx}: the layers' values per drawn primitive");

    // Lights and cameras: all the fixture's survive where both containers
    // keep them and none elsewhere, and the two writes name each kind once
    // wherever they go, as "N light(s) […]" or "N camera(s) […]".
    let counts: [(LossKind, bool, fn(&Scene) -> usize, &str); 2] = [
        (LossKind::Lights, caps.lights, |s| s.lights.len(), " light(s) ["),
        (LossKind::Cameras, caps.cameras, |s| s.cameras.len(), " camera(s) ["),
    ];
    for (kind, keeps, count, noun) in counts {
        let kept = if keeps { count(fixture) } else { 0 };
        assert_eq!(count(&expected), kept, "{ctx}: the expected scene's {kind}");
        assert_eq!(count(&got), kept, "{ctx}: {kind}");
        let named = losses.iter().filter(|l| l.kind == kind && names_them_all(&l.what, noun)).count();
        assert_eq!(named, usize::from(count(fixture) > 0 && !keeps), "{ctx}: {kind} named {named} time(s): {losses:?}");
    }
    // Ruling R27: a write to Collada or USD names a point or spot light's
    // range, which only litegen's own annotation keeps there: the first
    // write's, and the second's when the first kept the lights (and so the
    // range). Nothing else about a light or camera of the fixture is named.
    let annotated = |f: MeshFormat| matches!(f, MeshFormat::Dae | MeshFormat::Usdz | MeshFormat::Usda | MeshFormat::Usdc);
    let ranged = fixture.lights.iter().any(|l| l.kind != LightKind::Directional && l.range.is_some());
    let ranges = usize::from(ranged && annotated(from)) + usize::from(ranged && annotated(to) && from.capabilities(opts).lights);
    let named = losses.iter().filter(|l| l.kind == LossKind::Lights && l.what.starts_with("the range of ")).count();
    assert_eq!(named, ranges, "{ctx}: light ranges named {named} time(s): {losses:?}");
    let other: Vec<&Loss> = losses
        .iter()
        .filter(|l| match l.kind {
            LossKind::Lights => !names_them_all(&l.what, " light(s) [") && !l.what.starts_with("the range of "),
            LossKind::Cameras => !names_them_all(&l.what, " camera(s) ["),
            _ => false,
        })
        .collect();
    assert!(other.is_empty(), "{ctx}: more named of the lights and cameras: {other:?}");
    // And what each kept one holds: where it stands, faces and projects, at
    // rest and at every sample time, as in the expected scene.
    if !expected.lights.is_empty() || !expected.cameras.is_empty() {
        fixtures::lightcam::assert_same_lights_and_cameras(&expected, &got, 1e-5, &ctx);
    }

    // Points and lines: the primitives of each survive where both containers
    // hold them and none elsewhere, and the two writes name each topology
    // once wherever it goes, as "N point primitive(s)" or "N line
    // primitive(s)". Where they survive, they stand where the expected
    // scene's do at rest and at every sample time, as the triangles do
    // (the shapes above are of the triangles alone).
    //
    // PLY holds points, but only in a file with no triangles and no lines
    // (`ply::prepare`), which no flag says: the points a pair's PLY write is
    // given are dropped, and named once, whenever that scene (the fixture when
    // PLY is the source, else what the source read back as) draws anything else.
    let ply_scene = if from == MeshFormat::Ply {
        Some(fixture)
    } else if to == MeshFormat::Ply {
        Some(&source)
    } else {
        None
    };
    let ply_drops_points = ply_scene.is_some_and(|s| {
        let draws = |t: Topology| s.meshes.iter().flat_map(|m| &m.primitives).any(|p| p.topology == t);
        draws(Topology::Points) && (draws(Topology::Triangles) || draws(Topology::Lines))
    });
    let topologies: [(LossKind, Topology, bool, &str); 2] = [
        (LossKind::Points, Topology::Points, caps.points && !ply_drops_points, " point primitive(s)"),
        (LossKind::Lines, Topology::Lines, caps.lines, " line primitive(s)"),
    ];
    for (kind, topology, keeps, noun) in topologies {
        let primitives = |s: &Scene| s.meshes.iter().flat_map(|m| &m.primitives).filter(|p| p.topology == topology).count();
        let kept = if keeps { primitives(fixture) } else { 0 };
        assert_eq!(primitives(&expected), kept, "{ctx}: the expected scene's {topology:?} primitives");
        assert_eq!(primitives(&got), kept, "{ctx}: {topology:?} primitives");
        let named: Vec<&Loss> = losses.iter().filter(|l| l.kind == kind).collect();
        assert_eq!(named.len(), usize::from(primitives(fixture) > 0 && !keeps), "{ctx}: {kind} named {} time(s): {losses:?}", named.len());
        let counted = format!("{}{noun}", primitives(fixture));
        assert!(named.iter().all(|l| l.what == counted), "{ctx}: {kind} named as {named:?}, not {counted:?}");
        if keeps {
            for t in &times {
                let (want, have) = (extent(&expected, *t, topology), extent(&got, *t, topology));
                assert_eq!(have.is_some(), want.is_some(), "{ctx} @ {t:?}: {topology:?} drawn");
                if let (Some(want), Some(have)) = (want, have) {
                    assert_same_extent(&want, &have, &format!("{ctx} @ {t:?}: {topology:?}"));
                }
            }
        }
    }
}

#[test]
fn every_fixture_survives_every_direction_as_a_package() {
    let opts = WriteOptions { self_contained: false, ..Default::default() };
    for (name, fixture) in fixtures::all() {
        for from in MeshFormat::ALL {
            for to in MeshFormat::ALL {
                check_pair(name, &fixture, from, to, opts);
            }
        }
    }
}

#[test]
fn every_fixture_survives_every_direction_as_a_single_file() {
    // What the proxy delivers: one file per format.
    let opts = WriteOptions { self_contained: true, ..Default::default() };
    for (name, fixture) in fixtures::all() {
        for from in MeshFormat::ALL {
            for to in MeshFormat::ALL {
                check_pair(name, &fixture, from, to, opts);
            }
        }
    }
}

/// The `dae:keep` write mode: Collada written with `DaeMorphs::Keep`, every
/// fixture through every direction into and out of it. Only Collada reads the
/// option, so the pairs that never touch it are the default mode's. Under
/// `Keep` a Collada file holds the morph targets and their weight animation,
/// so a pair keeps them wherever its other container does too, and nowhere
/// else: the `morph` fixture's weight channel survives exactly where the
/// capabilities say it does.
fn dae_keep_mode(self_contained: bool) {
    let opts = WriteOptions { self_contained, dae_morphs: DaeMorphs::Keep, ..Default::default() };
    let weights = |s: &Scene| s.animations.iter().flat_map(|a| &a.channels).any(|c| c.path == AnimPath::Weights);
    for (name, fixture) in fixtures::all() {
        for from in MeshFormat::ALL {
            for to in MeshFormat::ALL {
                if from != MeshFormat::Dae && to != MeshFormat::Dae {
                    continue;
                }
                check_pair(name, &fixture, from, to, opts);
                if name == "morph" {
                    let caps = from.capabilities(opts).intersect(to.capabilities(opts));
                    let got = round(&round(&fixture, from, opts), to, opts);
                    assert_eq!(
                        weights(&got),
                        caps.morph_targets && caps.animation,
                        "{name}: {from} -> {to}{}: morph-weight animation survives where the capabilities keep it",
                        if self_contained { " (single file)" } else { "" }
                    );
                }
            }
        }
    }
}

#[test]
fn every_fixture_survives_collada_written_with_dae_keep_as_a_package() {
    dae_keep_mode(false);
}

#[test]
fn every_fixture_survives_collada_written_with_dae_keep_as_a_single_file() {
    dae_keep_mode(true);
}

#[test]
fn a_clip_that_starts_late_holds_its_first_pose_from_zero_in_every_container() {
    // CesiumMan's keys begin at 1/24 s. glTF holds the first key until then;
    // every container that resamples must say so explicitly, or readers that
    // start a clip at its first key play the opening early.
    let mut late = fixtures::skinned();
    for ch in late.animations[0].channels.iter_mut() {
        for t in ch.times.iter_mut() {
            *t += 0.5;
        }
    }
    let opts = WriteOptions::default();
    for fmt in MeshFormat::ALL.into_iter().filter(|f| f.capabilities(opts).animation) {
        let back = round(&late, fmt, opts);
        let (lo, _) = back.animations[0].time_range().expect("a clip");
        assert!(lo <= 0.5 + 1e-3, "{fmt}: the clip starts at {lo}");
        let expected = fmt.prepare(&late, opts).unwrap();
        for t in [0.0f32, 0.25, 0.5, 0.75] {
            assert_same_shape(&shape(&expected, Some(t)), &shape(&back, Some(t)), &format!("{fmt} @ {t}"));
        }
        if !matches!(fmt, MeshFormat::Glb | MeshFormat::Gltf) {
            assert_eq!(lo, 0.0, "{fmt}: a resampled clip is keyed from 0");
        }
    }
}

/// Mesh-gaps item 6: the `clips` fixture's three clips come back from every
/// container that holds clips (glTF, FBX, Collada and USD), and none from the
/// static ones (OBJ, STL, PLY, 3MF). A USDZ keeps what its default says
/// (`USDZ_DEFAULT_CLIPS`) unless asked for the other word, and a USDA or USDC
/// keeps all of them unless asked for the first.
#[test]
fn the_clips_fixture_keeps_its_three_clips_where_the_container_holds_them() {
    let fixture = fixtures::clips();
    assert_eq!(fixture.animations.len(), 3, "the subject");
    let usd = |f: MeshFormat| matches!(f, MeshFormat::Usdz | MeshFormat::Usda | MeshFormat::Usdc);
    for fmt in MeshFormat::ALL {
        for self_contained in [false, true] {
            let ask = |usd_clips| WriteOptions { self_contained, usd_clips, ..Default::default() };
            let ctx = format!("{fmt}{}", if self_contained { " (single file)" } else { "" });
            let default = ask(None);
            let held = if !fmt.capabilities(default).animation {
                0
            } else if usd(fmt) && default.clips_for(fmt) == UsdClips::First {
                1
            } else {
                3
            };
            assert_eq!(round(&fixture, fmt, default).animations.len(), held, "{ctx}: the clips it holds");
            if usd(fmt) {
                for (word, clips) in [(UsdClips::All, 3), (UsdClips::First, 1)] {
                    assert_eq!(round(&fixture, fmt, ask(Some(word))).animations.len(), clips, "{ctx}: usd_clips={}", word.as_str());
                }
            }
        }
    }
}

/// Mesh-gaps item 6: the `unlit` fixture's unlit box stays unlit in the
/// containers that hold the flag (glTF's `KHR_materials_unlit`, and USD's
/// recipe with litegen's marker), as a package and as a single file, with no
/// loss named for it, and is lit in the ones that do not (each words its own
/// loss). The wedge and the panel are the lit control, and stay lit
/// everywhere.
#[test]
fn the_unlit_fixture_keeps_its_unlit_box_where_the_container_holds_it() {
    let fixture = fixtures::unlit();
    let unlit_names = |s: &Scene| s.materials.iter().filter(|m| m.unlit).map(|m| m.name.clone().unwrap_or_default()).collect::<Vec<_>>();
    assert_eq!(unlit_names(&fixture), ["box_unlit"], "the subject");
    for fmt in MeshFormat::ALL {
        for self_contained in [false, true] {
            let opts = WriteOptions { self_contained, ..Default::default() };
            let ctx = format!("{fmt}{}", if self_contained { " (single file)" } else { "" });
            let holds = matches!(fmt, MeshFormat::Glb | MeshFormat::Gltf | MeshFormat::Usdz | MeshFormat::Usda | MeshFormat::Usdc);
            let want: &[&str] = if holds { &["box_unlit"] } else { &[] };
            let (got, losses) = written(&fixture, fmt, opts);
            assert_eq!(unlit_names(&got), want, "{ctx}: the unlit materials it holds");
            // Where it is held it is not reported. Elsewhere each container
            // words its own loss (a static one's is the materials as a whole).
            if holds {
                assert!(losses.iter().all(|l| !l.what.contains("unlit")), "{ctx}: unlit is held, so not reported: {losses:#?}");
            }
        }
    }
}

#[test]
fn the_invariants_notice_a_mirror_and_a_moved_part() {
    // The comparison above is only worth something if it fails when it
    // should. A mirrored copy keeps the area and the bounds' extent but flips
    // the signed volume; a moved node keeps everything but the centroid.
    let base = fixtures::textured();
    let a = shape(&base, None);

    let mut mirrored = base.clone();
    if let super::scene::Transform::Trs { scale, .. } = &mut mirrored.nodes[0].transform {
        scale[0] = -scale[0];
    }
    let m = shape(&mirrored, None);
    assert!(!close(m.volume, a.volume, 10.0) || !close(m.centroid[0], a.centroid[0], 1.0), "mirror unnoticed");

    let mut moved = base.clone();
    if let super::scene::Transform::Trs { translation, .. } = &mut moved.nodes[2].transform {
        translation[1] += 0.5;
    }
    let b = shape(&moved, None);
    assert!(!close(b.centroid[1], a.centroid[1], 1.0), "moved part unnoticed");
}

#[test]
fn losses_carry_stable_machine_readable_kinds() {
    // The spellings are an API contract (response headers, dashboards); a
    // rename here is a breaking change for every caller.
    let all = [
        (super::scene::LossKind::Animation, "animation"),
        (super::scene::LossKind::Skins, "skins"),
        (super::scene::LossKind::MorphTargets, "morph_targets"),
        (super::scene::LossKind::Normals, "normals"),
        (super::scene::LossKind::Uvs, "uvs"),
        (super::scene::LossKind::VertexColors, "vertex_colors"),
        (super::scene::LossKind::Textures, "textures"),
        (super::scene::LossKind::ImageFormats, "image_formats"),
        (super::scene::LossKind::Materials, "materials"),
        (super::scene::LossKind::Alpha, "alpha"),
        (super::scene::LossKind::Hierarchy, "hierarchy"),
        (super::scene::LossKind::Meshes, "meshes"),
        (super::scene::LossKind::TextureSampling, "texture_sampling"),
        (super::scene::LossKind::TextureTransforms, "texture_transforms"),
        (super::scene::LossKind::Geometry, "geometry"),
        (super::scene::LossKind::Lights, "lights"),
        (super::scene::LossKind::Cameras, "cameras"),
        (super::scene::LossKind::Points, "points"),
        (super::scene::LossKind::Lines, "lines"),
    ];
    for (k, s) in all {
        assert_eq!(k.as_str(), s);
    }
    // A skinned, animated, textured character into STL loses exactly these.
    let w = write_scene(&fixtures::skinned(), MeshFormat::Stl, WriteOptions::default()).unwrap();
    let mut kinds: Vec<&str> = w.losses.iter().map(|l| l.kind.as_str()).collect();
    kinds.sort_unstable();
    kinds.dedup();
    for want in ["animation", "skins", "normals", "uvs", "textures", "materials", "hierarchy"] {
        assert!(kinds.contains(&want), "{want} missing from {kinds:?}");
    }
    // And the same character into GLB loses nothing.
    let w = write_scene(&fixtures::skinned(), MeshFormat::Glb, WriteOptions::default()).unwrap();
    assert!(w.losses.is_empty(), "{:?}", w.losses);
}

/// `wire` less the primitives of every topology but triangles, by hand: the
/// nodes that drew them stay, drawing nothing.
fn wire_triangles() -> Scene {
    let mut s = fixtures::wire();
    for m in s.meshes.iter_mut() {
        m.primitives.retain(|p| p.topology == Topology::Triangles);
    }
    s
}

/// Area, signed volume, bounds and centroid are of the triangles, and a
/// loop and a grid beside them add to none, at rest and while the clip moves
/// the loop.
#[test]
fn metrics_ignore_points_and_lines() {
    let (wire, cube) = (fixtures::wire(), wire_triangles());
    for t in std::iter::once(None).chain(fixtures::sample_times(&wire).into_iter().map(Some)) {
        let (got, want) = (shape(&wire, t), shape(&cube, t));
        assert_eq!(got.triangles, 12, "@ {t:?}");
        assert_eq!((got.area, got.volume, got.lo, got.hi, got.centroid), (want.area, want.volume, want.lo, want.hi, want.centroid), "@ {t:?}");
    }
    assert!(shape(&wire, None).area > 0.0 && shape(&wire, None).volume > 0.0);
}

/// What the comparison above does with points and lines is only worth
/// something if it fails when it should: `wire` against itself passes, and a
/// loop moved a quarter metre, a grid with one point out of place and a loop
/// of fewer segments each fail, naming what differs.
#[test]
fn the_extents_notice_what_moves() {
    let wire = fixtures::wire();
    let failure = |got: &Scene, topology: Topology| -> String {
        let (want, have) = (extent(&wire, None, topology).unwrap(), extent(got, None, topology).unwrap());
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| assert_same_extent(&want, &have, "case")));
        r.err().map(|e| e.downcast_ref::<String>().cloned().unwrap_or_default()).unwrap_or_default()
    };
    assert_eq!(failure(&wire, Topology::Lines), "");
    assert_eq!(failure(&wire, Topology::Points), "");
    assert!(extent(&wire, None, Topology::Triangles).is_some() && extent(&wire_triangles(), None, Topology::Points).is_none());

    let mut shifted = wire.clone();
    shifted.nodes[1].transform = super::scene::Transform::Trs { translation: [0.25, 0.0, 0.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] };
    assert!(failure(&shifted, Topology::Lines).contains("case: bounds on axis 0"), "{}", failure(&shifted, Topology::Lines));
    let mut off = wire.clone();
    off.meshes[2].primitives[0].positions[4] = [0.0, -1.0, 0.5];
    assert!(failure(&off, Topology::Points).contains("case: centroid on axis 2"), "{}", failure(&off, Topology::Points));
    let mut fewer = wire.clone();
    fewer.meshes[1].primitives[0].indices.truncate(6);
    assert!(failure(&fewer, Topology::Lines).contains("case: vertices drawn"), "{}", failure(&fewer, Topology::Lines));
}

/// `check_pair` compares the points and lines of a pair only where both of
/// its containers hold them, so a glTF whose capabilities lost them would
/// drop out of the comparison without a word: every pair of GLB and glTF is
/// compared, both ways and each with itself.
#[test]
fn the_matrix_compares_the_points_and_lines_glb_and_gltf_hold() {
    for opts in [WriteOptions::default(), WriteOptions { self_contained: false, ..Default::default() }] {
        for from in [MeshFormat::Glb, MeshFormat::Gltf] {
            for to in [MeshFormat::Glb, MeshFormat::Gltf] {
                let caps = from.capabilities(opts).intersect(to.capabilities(opts));
                assert!(caps.points && caps.lines, "{from} to {to}: {caps:?}");
            }
        }
    }
}

/// The same for OBJ and PLY (mesh-gaps item 10, Task 3): each pair of them
/// with GLB, glTF, OBJ and PLY is compared, both ways, in both packagings.
#[test]
fn the_matrix_compares_the_points_and_lines_obj_and_ply_hold() {
    for opts in [WriteOptions::default(), WriteOptions { self_contained: false, ..Default::default() }] {
        for from in [MeshFormat::Obj, MeshFormat::Ply] {
            for to in [MeshFormat::Glb, MeshFormat::Gltf, MeshFormat::Obj, MeshFormat::Ply] {
                for (a, b) in [(from, to), (to, from)] {
                    let caps = a.capabilities(opts).intersect(b.capabilities(opts));
                    assert!(caps.points && caps.lines, "{a} to {b}: {caps:?}");
                }
            }
        }
    }
}

/// The same for USD and Collada (mesh-gaps item 10, Task 4). USDA and USDZ
/// hold both, so each pair of them with GLB, glTF, OBJ, PLY and each other is
/// compared in both directions and both packagings. Collada holds lines and
/// not points (1.4.1 has no point primitive), so it is compared with each of
/// those on lines alone, and it names the points it drops.
#[test]
fn the_matrix_compares_the_points_and_lines_usd_and_collada_hold() {
    let both = [MeshFormat::Glb, MeshFormat::Gltf, MeshFormat::Obj, MeshFormat::Ply, MeshFormat::Usda, MeshFormat::Usdc, MeshFormat::Usdz];
    for opts in [WriteOptions::default(), WriteOptions { self_contained: false, ..Default::default() }] {
        for usd in [MeshFormat::Usda, MeshFormat::Usdc, MeshFormat::Usdz] {
            for to in both {
                for (a, b) in [(usd, to), (to, usd)] {
                    let caps = a.capabilities(opts).intersect(b.capabilities(opts));
                    assert!(caps.points && caps.lines, "{a} to {b}: {caps:?}");
                }
            }
        }
        for to in both.into_iter().chain([MeshFormat::Dae]) {
            for (a, b) in [(MeshFormat::Dae, to), (to, MeshFormat::Dae)] {
                let caps = a.capabilities(opts).intersect(b.capabilities(opts));
                assert!(caps.lines && !caps.points, "{a} to {b}: {caps:?}");
            }
        }
    }
}

/// A container that cannot hold points or lines (its capabilities say so)
/// drops them from what it writes and names each once, by its count of
/// primitives; and that is all it says of them: what else it reports of
/// `wire` is what it reports of the same scene with them stripped (a loop's
/// eight indices are no triangles that 3MF could call degenerate, the cube is
/// not three primitives to merge, vertex colours only the loop and the grid
/// have are not lost twice). One that holds them keeps them, and names
/// nothing.
#[test]
fn every_format_names_the_points_and_lines_it_cannot_hold() {
    let mut wire = fixtures::wire();
    // The loop's normals: a container that does not hold lines drops them
    // with the loop and says nothing more of them, and one that holds lines
    // but not their normals (OBJ's `l`, USD's `BasisCurves`) names them by
    // itself, so the comparison below sees the same report with the lines
    // stripped as with them.
    wire.meshes[1].primitives[0].normals = Some(vec![[0.0, 1.0, 0.0]; 4]);
    for opts in [WriteOptions::default(), WriteOptions { self_contained: false, ..Default::default() }] {
        for fmt in MeshFormat::ALL {
            let caps = fmt.capabilities(opts);
            let ctx = format!("{fmt}{}", if opts.self_contained { " (single file)" } else { "" });
            // PLY holds points only in a file with no triangles and no lines
            // (`ply::prepare`): `wire` has both, so they go, and are named. Its
            // own rule says so (`ply::losses`), for PLY alone.
            let content_rule: Vec<Loss> = if fmt == MeshFormat::Ply { ply::losses(&wire) } else { Vec::new() };
            let holds_points = caps.points && content_rule.is_empty();
            // The scene less the topologies this container cannot hold.
            let mut kept = wire.clone();
            pose::strip_topologies(&mut kept, &caps);
            let prepared = fmt.prepare(&wire, opts).unwrap_or_else(|e| panic!("{ctx}: prepare: {e}"));
            prepared.validate().unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let kinds: Vec<Topology> = prepared.meshes.iter().flat_map(|m| &m.primitives).map(|p| p.topology).collect();
            assert!(kinds.contains(&Topology::Triangles) && kinds.iter().all(|t| caps.holds(*t)), "{ctx}: {kinds:?}");
            assert_eq!(kinds.contains(&Topology::Lines), caps.lines, "{ctx}: the lines it holds are kept: {kinds:?}");
            assert_eq!(kinds.contains(&Topology::Points), holds_points, "{ctx}: the points it holds are kept: {kinds:?}");

            let with = write_scene(&wire, fmt, opts).unwrap_or_else(|e| panic!("{ctx}: write: {e}")).losses;
            // PLY still holds the points of `kept`, and names them as it does
            // `wire`'s (its content rule): that kind is compared above, and is
            // left out here for PLY alone. Every other format's report of
            // `kept` is compared whole. 3MF is told no more of `kept`'s nodes
            // that the strip emptied than of `wire`'s: `kept` is stripped by
            // hand, so its writer cannot know those nodes drew anything, and
            // counts them as nodes without geometry; asserted of `wire` below.
            let without: Vec<Loss> = write_scene(&kept, fmt, opts)
                .unwrap_or_else(|e| panic!("{ctx}: write what it holds: {e}"))
                .losses
                .into_iter()
                .filter(|l| !content_rule.iter().any(|r| r.kind == l.kind))
                .filter(|l| fmt != MeshFormat::ThreeMf || !l.what.contains("node(s) without geometry"))
                .collect();
            let named = |kind: LossKind| -> Vec<String> { with.iter().filter(|l| l.kind == kind).map(|l| l.what.clone()).collect() };
            let says = |holds: bool, what: &str| -> Vec<String> { if holds { Vec::new() } else { vec![what.to_string()] } };
            assert_eq!(named(LossKind::Points), says(holds_points, "1 point primitive(s)"), "{ctx}");
            assert_eq!(named(LossKind::Lines), says(caps.lines, "1 line primitive(s)"), "{ctx}");
            if matches!(fmt, MeshFormat::Usda | MeshFormat::Usdc | MeshFormat::Usdz) {
                assert_eq!(named(LossKind::Normals), ["normals of 1 line primitive(s)"], "{ctx}: the loop's normals are left out of its BasisCurves");
            }
            if fmt == MeshFormat::ThreeMf {
                // The loop's and the grid's nodes draw nothing once their
                // lines and points are stripped, and those are named above:
                // a third line about the same nodes tells the caller nothing.
                // A node that drew nothing to begin with is still named.
                let without_geometry = |l: &[Loss]| -> Vec<String> { l.iter().filter(|l| l.what.contains("node(s) without geometry")).map(|l| l.what.clone()).collect() };
                assert_eq!(without_geometry(&with), Vec::<String>::new(), "{ctx}: the nodes the strip emptied are not nodes without geometry: {with:?}");
                let mut bare = wire.clone();
                bare.add_root(super::scene::Node { name: Some("bare".into()), ..Default::default() });
                let l = write_scene(&bare, fmt, opts).unwrap_or_else(|e| panic!("{ctx}: write: {e}")).losses;
                assert_eq!(without_geometry(&l), ["1 node(s) without geometry"], "{ctx}: a node that drew nothing before is named: {l:?}");
            }
            let rest: Vec<&Loss> = with.iter().filter(|l| !matches!(l.kind, LossKind::Points | LossKind::Lines)).collect();
            assert_eq!(rest, without.iter().collect::<Vec<_>>(), "{ctx}: the rest of what it reports");
        }
    }
}

/// `wire` with only the primitives of `kinds` left. A mesh that loses all its
/// primitives stays, empty, under its node.
fn wire_keeping(kinds: &[Topology]) -> Scene {
    let mut s = fixtures::wire();
    for m in &mut s.meshes {
        m.primitives.retain(|p| kinds.contains(&p.topology));
    }
    s
}

/// STL, 3MF and FBX hold neither points nor lines, and their writers are
/// never given one: `prepare` removes them, so what a writer gets is the
/// triangles alone (3MF's own `threemf::write_scene` prepares again, so even a
/// direct call keeps them from its encoder). A writer that is given one
/// anyway panics in a debug build: `..._3mf` in threemf.rs for 3MF's
/// encoder, and the test below for FBX's writer. STL's writer takes the flat
/// triangle mesh, which has no topology to assert.
#[test]
fn writers_never_see_a_topology_they_cannot_hold() {
    let wire = fixtures::wire();
    for fmt in [MeshFormat::Stl, MeshFormat::ThreeMf, MeshFormat::Fbx] {
        let caps = fmt.capabilities(WriteOptions::default());
        assert!(!caps.points && !caps.lines, "{fmt} holds neither");
        let prepared = fmt.prepare(&wire, WriteOptions::default()).unwrap_or_else(|e| panic!("{fmt}: {e}"));
        let kinds: Vec<Topology> = prepared.meshes.iter().flat_map(|m| &m.primitives).map(|p| p.topology).collect();
        assert!(!kinds.is_empty() && kinds.iter().all(|k| *k == Topology::Triangles), "{fmt}: {kinds:?}");
        write_scene(&wire, fmt, WriteOptions::default()).unwrap_or_else(|e| panic!("{fmt}: write: {e}"));
    }
    // 3MF's writer, given the unprepared scene: the cube, and nothing else.
    let direct = threemf::write_scene(&wire).unwrap_or_else(|e| panic!("3mf: {e}"));
    let back = threemf::read_scene(&direct.files[0].bytes, &NoSidecars).unwrap_or_else(|e| panic!("3mf: {e}"));
    assert_eq!((back.scene.triangle_count(), back.scene.drawable_count()), (12, 12));
    // Their own gates keep counting triangles, as these containers hold
    // nothing else: a scene of points alone is nothing to either writer,
    // before or after `prepare` (`write_scene` names why, first:
    // `points_to_stl_is_named`).
    let points = wire_keeping(&[Topology::Points]);
    let prepared = |fmt: MeshFormat| fmt.prepare(&points, WriteOptions::default()).unwrap_or_else(|e| panic!("{fmt}: prepare: {e}"));
    assert_eq!(threemf::write_scene(&points).err(), Some(MeshError::Empty));
    assert_eq!(threemf::write_scene(&prepared(MeshFormat::ThreeMf)).err(), Some(MeshError::Empty));
    assert_eq!(fbx::write_scene(&prepared(MeshFormat::Fbx), 30.0, FbxColorSpace::default(), LightUnits::default()).err(), Some(MeshError::Empty));
}

/// FBX's writer, given a scene that `prepare` did not reduce.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "fbx was given a lines primitive; prepare strips what it cannot hold")]
fn writers_never_see_a_topology_they_cannot_hold_fbx() {
    let _ = fbx::write_scene(&fixtures::wire(), 30.0, FbxColorSpace::default(), LightUnits::default());
}

/// A scene with nothing left for the target is refused by name, as
/// `Unsupported`, not as an empty mesh (`Empty`): STL's sentence is
/// the spec's. A target that holds the scene's elements writes it, and a
/// scene with triangles beside them is written, with its losses named.
#[test]
fn points_to_stl_is_named() {
    let refused = |scene: &Scene, to: MeshFormat| write_scene(scene, to, WriteOptions::default()).err();
    let named = |m: &str| Some(MeshError::Unsupported(m.to_string()));
    let points = wire_keeping(&[Topology::Points]);
    let lines = wire_keeping(&[Topology::Lines]);
    let both = wire_keeping(&[Topology::Lines, Topology::Points]);
    assert_eq!(refused(&points, MeshFormat::Stl), named("stl holds triangles only; this scene has 9 points and no triangles"));
    assert_eq!(refused(&lines, MeshFormat::Stl), named("stl holds triangles only; this scene has 4 line segments and no triangles"));
    assert_eq!(
        refused(&both, MeshFormat::ThreeMf),
        named("3mf holds triangles only; this scene has 4 line segments and 9 points and no triangles")
    );
    assert_eq!(refused(&points, MeshFormat::Fbx), named("fbx holds triangles only; this scene has 9 points and no triangles"));
    // Collada holds lines, not points.
    assert_eq!(
        refused(&points, MeshFormat::Dae),
        named("dae holds triangles and lines; this scene has 9 points and no triangles or lines")
    );
    assert_eq!(refused(&lines, MeshFormat::Dae), None);
    // One of each is singular.
    let mut one = points.clone();
    let p = &mut one.meshes[2].primitives[0];
    p.positions.truncate(1);
    p.colors = p.colors.take().map(|mut c| {
        c.truncate(1);
        c
    });
    p.indices = vec![0];
    assert_eq!(refused(&one, MeshFormat::Stl), named("stl holds triangles only; this scene has 1 point and no triangles"));
    let mut one = lines.clone();
    one.meshes[1].primitives[0].indices = vec![0, 1];
    assert_eq!(refused(&one, MeshFormat::Stl), named("stl holds triangles only; this scene has 1 line segment and no triangles"));
    // Triangles beside them: written, the points and lines named as lost.
    let written = write_scene(&fixtures::wire(), MeshFormat::Stl, WriteOptions::default()).unwrap_or_else(|e| panic!("stl: {e}"));
    assert!(written.losses.iter().any(|l| l.kind == LossKind::Points) && written.losses.iter().any(|l| l.kind == LossKind::Lines));
    // Every container that holds points writes a scene of points alone.
    for fmt in MeshFormat::ALL {
        let held = fmt.capabilities(WriteOptions::default()).points;
        match refused(&points, fmt) {
            None => assert!(held, "{fmt} wrote points it cannot hold"),
            Some(MeshError::Unsupported(m)) => assert!(!held && m.starts_with(&format!("{fmt} holds ")), "{fmt}: {m}"),
            Some(e) => panic!("{fmt}: {e}"),
        }
    }
}

// ─── Review Focus 1–3 (mesh-gaps item 10, Task 6b) ──────────────────────────

/// Where `topology`'s elements are drawn: the world position of every index
/// of every primitive of that topology, in the order drawn (a segment's two
/// ends, a triangle's three corners), at `anim_time`.
fn drawn(scene: &Scene, anim_time: Option<f32>, topology: Topology) -> Vec<[f32; 3]> {
    pose::world_primitives(scene, &pose_at(scene, anim_time))
        .expect("pose")
        .iter()
        .filter(|wp| wp.topology == topology)
        .flat_map(|wp| wp.indices.iter().map(|&i| wp.positions[i as usize]))
        .collect()
}

/// The largest difference on any axis between two lists of positions, and
/// infinity where their lengths differ.
fn farthest(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
    if a.len() != b.len() {
        return f32::INFINITY;
    }
    a.iter().zip(b).flat_map(|(p, q)| (0..3).map(move |k| (p[k] - q[k]).abs())).fold(0.0, f32::max)
}

/// `fixtures::skinned()`'s tube, given two morph targets (a bulge out along
/// its radius, and a lift that grows with height) and a weights clip beside
/// the joints', and, in the same mesh and under the same skin and the same
/// targets, a primitive of its edges as lines and one of its vertices as
/// points. The three primitives hold the same vertices, so a pose that moves
/// one moves the others the same way.
fn skinned_and_morphed_edges() -> Scene {
    use super::scene::{AnimPath, Channel, Interpolation, MorphTarget, Primitive};
    let mut s = fixtures::skinned();
    let node = s.nodes.iter().position(|n| n.mesh.is_some()).expect("the tube's node");
    let tube = &mut s.meshes[0].primitives[0];
    let bulge = tube.positions.iter().map(|p| [p[0] * 0.8, 0.0, p[2] * 0.8]).collect();
    let lift = tube.positions.iter().map(|p| [0.0, 0.2 * p[1], 0.0]).collect();
    tube.targets = vec![MorphTarget { positions: bulge, normals: None }, MorphTarget { positions: lift, normals: None }];
    let tube = tube.clone();
    // 25 rings of 17 vertices: the ring edges, then the edges between rings.
    let (rings, row) = (24u32, 17u32);
    let mut edges = Vec::new();
    for r in 0..=rings {
        edges.extend((0..row - 1).flat_map(|k| [r * row + k, r * row + k + 1]));
    }
    for r in 0..rings {
        edges.extend((0..row).flat_map(|k| [r * row + k, (r + 1) * row + k]));
    }
    let like = |topology: Topology, indices: Vec<u32>| Primitive {
        positions: tube.positions.clone(),
        skin: tube.skin.clone(),
        targets: tube.targets.clone(),
        indices,
        topology,
        ..Default::default()
    };
    let (lines, points) = (like(Topology::Lines, edges), like(Topology::Points, (0..tube.positions.len() as u32).collect()));
    s.meshes[0].primitives.extend([lines, points]);
    s.meshes[0].weights = vec![0.25, 0.0];
    s.meshes[0].target_names = vec!["bulge".into(), "lift".into()];
    s.animations[0].channels.push(Channel {
        node,
        path: AnimPath::Weights,
        interpolation: Interpolation::Linear,
        times: vec![0.0, 1.0, 2.0],
        values: vec![0.0, 0.0, 1.0, 0.5, 0.2, 1.0],
    });
    s
}

/// Review Focus 1. A line primitive and a points primitive under the skin and
/// the morph targets of a triangle primitive are posed as it is, by the
/// shared evaluator and by every container that holds them and a skin or a
/// morph. The three primitives hold the same vertices, so at rest and at each
/// sample time of the clip the world positions of the segments' and the
/// points' vertices must equal the triangles' to the bit; the pose must move
/// them (by the joints alone, by the weights alone, and by both); and each
/// container's read-back must draw them where the pose of what it was given
/// draws them, the triangles held to the same bar. Collada's morphs are
/// applied at their weights and OBJ's and PLY's whole pose is baked at rest
/// (`pose::bake` is how they receive it), so the baking is held to the same
/// source at the sample times as well.
#[test]
fn skinned_and_morphed_lines_pose_like_triangles() {
    use super::scene::AnimPath;
    let full = skinned_and_morphed_edges();
    full.validate().expect("valid");
    let mut skin_only = full.clone();
    skin_only.animations[0].channels.retain(|c| c.path != AnimPath::Weights);
    skin_only.meshes[0].weights = vec![0.0; 2];
    let mut morph_only = full.clone();
    morph_only.animations[0].channels.retain(|c| c.path == AnimPath::Weights);
    let times = |s: &Scene| -> Vec<Option<f32>> { std::iter::once(None).chain(fixtures::sample_times(s).into_iter().map(Some)).collect() };

    // The evaluator.
    for (what, s) in [("skin and morph", &full), ("skin alone", &skin_only), ("morph alone", &morph_only)] {
        let rest = pose::world_primitives(s, &pose_at(s, None)).expect("pose");
        let rest = &rest.iter().find(|wp| wp.topology == Topology::Triangles).expect("triangles").positions;
        let mut moved = 0.0f32;
        for t in times(s) {
            let posed = pose::world_primitives(s, &pose_at(s, t)).expect("pose");
            assert_eq!(posed.len(), 3, "{what} @ {t:?}: one posed primitive of each topology");
            let at = |topology: Topology| &posed.iter().find(|wp| wp.topology == topology).expect("a primitive of each").positions;
            let tri = at(Topology::Triangles);
            assert_eq!(at(Topology::Lines), tri, "{what} @ {t:?}: the segments' vertices are posed as the triangles'");
            assert_eq!(at(Topology::Points), tri, "{what} @ {t:?}: the points are posed as the triangles' vertices");
            moved = moved.max(farthest(tri, rest));
        }
        assert!(moved > 0.1, "{what}: the pose moves the tube by {moved} m at most, which would test nothing");
    }

    // The comparisons bite: a line primitive posed without its skin, or
    // without its morph targets, is not where the triangles are.
    for (what, wrong) in [
        ("skin", { let mut w = full.clone(); w.meshes[0].primitives[1].skin = None; w }),
        ("morph targets", { let mut w = full.clone(); w.meshes[0].primitives[1].targets.clear(); w }),
    ] {
        let at = |s: &Scene, topology: Topology| -> Vec<[f32; 3]> { drawn(s, Some(1.0), topology) };
        assert!(farthest(&at(&wrong, Topology::Lines), &at(&full, Topology::Lines)) > 0.05, "a line primitive without its {what} must be posed differently");
        let glb = MeshFormat::Glb.prepare(&wrong, WriteOptions::default()).expect("prepare");
        let (got, _) = written(&full, MeshFormat::Glb, WriteOptions::default());
        assert!(farthest(&drawn(&glb, Some(1.0), Topology::Lines), &drawn(&got, Some(1.0), Topology::Lines)) > 0.05, "the round-trip comparison must see a line primitive without its {what}");
    }

    // Every container that holds points or lines and a skin or a morph, in
    // both packagings: what it reads back is drawn where the pose of what it
    // was given draws it (`prepare`'s scene, as in `check_pair`), triangles
    // included; where it keeps the whole clip (skins, morph targets and
    // animation: glTF and USD) the source's own pose is the reference; and
    // the clip still moves the lines.
    let holders: Vec<MeshFormat> = MeshFormat::ALL
        .into_iter()
        .filter(|f| {
            let c = f.capabilities(WriteOptions::default());
            (c.points || c.lines) && (c.skinning || c.morph_targets)
        })
        .collect();
    assert_eq!(holders, [MeshFormat::Glb, MeshFormat::Gltf, MeshFormat::Dae, MeshFormat::Usdz, MeshFormat::Usda, MeshFormat::Usdc]);
    for fmt in holders {
        for opts in [WriteOptions::default(), WriteOptions { self_contained: false, ..Default::default() }] {
            let ctx = format!("{fmt}{}", if opts.self_contained { " (single file)" } else { "" });
            let caps = fmt.capabilities(opts);
            let expected = fmt.prepare(&full, opts).unwrap_or_else(|e| panic!("{ctx}: prepare: {e}"));
            let (got, _) = written(&full, fmt, opts);
            let reference: &Scene = if caps.skinning && caps.morph_targets && caps.animation { &full } else { &expected };
            // The 2e-3 m is UsdSkel's `half3[] scales` (usd.rs, "joint scales
            // at half precision"), which USD rounds an animated joint scale
            // to: at the fixture's hip scale of
            // 1.25 (in [1, 2), where a half steps by 2^-10) the rounding is at
            // most 2^-11 = 4.88e-4, and the tube is 3 m, so a vertex at its far
            // end is off by at most 4.88e-4 x 3 = 1.46e-3 m. The 1.41e-3 m
            // measured is about 70 % of the 2e-3 m limit, which is the least
            // round figure above that bound.
            let mut worst = 0.0f32;
            for t in times(&full) {
                for topology in [Topology::Triangles, Topology::Lines, Topology::Points].into_iter().filter(|k| caps.holds(*k)) {
                    for (label, from) in [("prepare", &expected), ("the source", reference)] {
                        let (want, have) = (drawn(from, t, topology), drawn(&got, t, topology));
                        assert!(!want.is_empty(), "{ctx} @ {t:?}: {label} draws {topology:?}");
                        let off = farthest(&want, &have);
                        worst = worst.max(off);
                        assert!(off < 2e-3, "{ctx} @ {t:?}: {topology:?} drawn {off} m from where the pose of {label} puts them");
                    }
                }
            }
            eprintln!("{ctx}: read back at most {worst} m from where the pose draws it");
            if caps.lines {
                let (rest, mid) = (drawn(&got, None, Topology::Lines), drawn(&got, Some(1.0), Topology::Lines));
                assert!(farthest(&rest, &mid) > 0.1, "{ctx}: the clip moves the lines");
            }
        }
    }

    // The baking OBJ and PLY receive their scene by: the pose at a time,
    // baked, draws what the evaluator draws at that time.
    for t in times(&full) {
        let baked = pose::bake(&full, &pose_at(&full, t)).expect("bake");
        for topology in [Topology::Triangles, Topology::Lines, Topology::Points] {
            let (want, have) = (drawn(&full, t, topology), drawn(&baked, None, topology));
            assert!(!want.is_empty() && farthest(&want, &have) < 1e-6, "bake @ {t:?}: {topology:?} {} from the evaluator's", farthest(&want, &have));
        }
    }
    for fmt in [MeshFormat::Obj, MeshFormat::Ply] {
        for opts in [WriteOptions::default(), WriteOptions { self_contained: false, ..Default::default() }] {
            let ctx = format!("{fmt}{}", if opts.self_contained { " (single file)" } else { "" });
            let (got, _) = written(&full, fmt, opts);
            // PLY holds the points of a file with no faces or edges alone.
            let kept = if fmt == MeshFormat::Ply { vec![Topology::Triangles, Topology::Lines] } else { vec![Topology::Triangles, Topology::Lines, Topology::Points] };
            for topology in kept {
                let (want, have) = (drawn(&full, None, topology), drawn(&got, None, topology));
                assert!(!want.is_empty() && farthest(&want, &have) < 1e-5, "{ctx}: {topology:?} baked {} from the evaluator's", farthest(&want, &have));
            }
        }
    }
}

/// Review Focus 3. A loop and a grid of points hung under a parent that
/// translates, rotates and scales (non-uniformly) are written to OBJ where the
/// parent puts them: the text's `l` and `p` statements name vertices at the
/// world positions worked out here by hand (scale, then turn about +Y, then
/// move), and the reader gives the same positions back and draws them where
/// the engine's own pose draws the source, within f32 rounding. The local
/// positions are far from those, so a writer that left them in the node's
/// space would fail.
#[test]
fn obj_bakes_lines_to_world_space() {
    use super::scene::{Node, Transform};
    let mut s = fixtures::wire();
    s.animations.clear();
    let angle = 30.0f64.to_radians();
    let (translation, scale, child) = ([2.0, -1.0, 3.0], [2.0, 0.5, 1.5], [0.5, 0.25, -0.75]);
    let parent = s.nodes.len();
    s.nodes.push(Node {
        name: Some("parent".into()),
        transform: Transform::Trs { translation, rotation: [0.0, (angle / 2.0).sin(), 0.0, (angle / 2.0).cos()], scale },
        children: vec![1, 2],
        ..Default::default()
    });
    s.roots = vec![0, parent];
    // The loop sits off its parent's origin; the grid at it.
    s.nodes[1].transform = Transform::Trs { translation: child, rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] };
    s.validate().expect("valid");
    let world = |offset: [f64; 3], v: [f32; 3]| -> [f64; 3] {
        let p = [(f64::from(v[0]) + offset[0]) * scale[0], (f64::from(v[1]) + offset[1]) * scale[1], (f64::from(v[2]) + offset[2]) * scale[2]];
        let (sn, cs) = angle.sin_cos();
        [translation[0] + p[0] * cs + p[2] * sn, translation[1] + p[1], translation[2] - p[0] * sn + p[2] * cs]
    };
    let (ring, grid) = (&s.meshes[1].primitives[0], &s.meshes[2].primitives[0]);
    let want_ends: Vec<[f64; 3]> = ring.indices.iter().map(|&i| world(child, ring.positions[i as usize])).collect();
    let want_points: Vec<[f64; 3]> = grid.indices.iter().map(|&i| world([0.0; 3], grid.positions[i as usize])).collect();
    assert_eq!((want_ends.len(), want_points.len()), (8, 9));
    let apart = |a: &[f64; 3], b: [f32; 3]| (0..3).map(|k| (a[k] - f64::from(b[k])).abs()).fold(0.0, f64::max);
    // The local positions are not these, by metres.
    assert!(ring.indices.iter().zip(&want_ends).all(|(&i, w)| apart(w, ring.positions[i as usize]) > 0.5));

    for self_contained in [true, false] {
        let ctx = if self_contained { "one file" } else { "a package" };
        let opts = WriteOptions { self_contained, ..Default::default() };
        let w = write_scene(&s, MeshFormat::Obj, opts).unwrap_or_else(|e| panic!("{ctx}: write: {e}"));
        let text = String::from_utf8(w.package.files[0].bytes.clone()).expect("utf8");
        // The file's own words: the `v` lines, and what `l` and `p` name.
        let (mut vertices, mut ends, mut points) = (Vec::<[f32; 3]>::new(), Vec::<[f32; 3]>::new(), Vec::<[f32; 3]>::new());
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let number = |w: Option<&str>| w.expect("a number").parse::<f32>().expect("a float");
            let named = |tok: &str| tok.split('/').next().unwrap().parse::<usize>().expect("a vertex") - 1;
            match words.next() {
                Some("v") => vertices.push([number(words.next()), number(words.next()), number(words.next())]),
                Some("l") => ends.extend(words.map(|t| vertices[named(t)])),
                Some("p") => points.extend(words.map(|t| vertices[named(t)])),
                _ => {}
            }
        }
        assert_eq!((ends.len(), points.len()), (8, 9), "{ctx}: what `l` and `p` name");
        for (i, (w, got)) in want_ends.iter().zip(&ends).enumerate() {
            assert!(apart(w, *got) < 1e-5, "{ctx}: segment end {i} is written at {got:?}, the parent puts it at {w:?}");
        }
        for (i, (w, got)) in want_points.iter().zip(&points).enumerate() {
            assert!(apart(w, *got) < 1e-5, "{ctx}: point {i} is written at {got:?}, the parent puts it at {w:?}");
        }
        // The reader, at identity roots, and the engine's own pose of the source.
        let (back, _) = written(&s, MeshFormat::Obj, opts);
        for topology in [Topology::Lines, Topology::Points] {
            let (source, read) = (drawn(&s, None, topology), drawn(&back, None, topology));
            assert!(!source.is_empty() && farthest(&source, &read) < 1e-5, "{ctx}: {topology:?} read back {} from the source's world positions", farthest(&source, &read));
        }
    }
}

/// A points primitive with texture coordinates, drawn with a material that
/// has a base-colour map: four points, one in each quadrant of the image.
fn textured_points() -> Scene {
    use super::scene::{Image, ImageFormat, Node, Primitive, SceneMesh, Texture, TextureRef};
    let mut s = Scene { name: Some("textured_points".into()), ..Default::default() };
    s.images.push(Image { name: Some("quadrants".into()), format: ImageFormat::Png, data: fixtures::quadrant_png() });
    s.textures.push(Texture { name: None, image: 0, sampler: Default::default() });
    s.materials.push(Material {
        name: Some("quadrants".into()),
        base_color_texture: Some(TextureRef { texture: 0, uv_set: 0, transform: None }),
        metallic: 0.0,
        roughness: 1.0,
        ..Default::default()
    });
    let points = Primitive {
        positions: vec![[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [-1.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
        uvs: vec![vec![[0.25, 0.25], [0.75, 0.25], [0.25, 0.75], [0.75, 0.75]]],
        indices: vec![0, 1, 2, 3],
        topology: Topology::Points,
        material: Some(0),
        ..Default::default()
    };
    s.meshes.push(SceneMesh { name: Some("texture_dots".into()), primitives: vec![points], ..Default::default() });
    s.add_root(Node { name: Some("texture_dots_node".into()), mesh: Some(0), ..Default::default() });
    s
}

/// What the Khronos validator refuses of a map with no coordinate to sample
/// it by (MESH_PRIMITIVE_TOO_FEW_TEXCOORDS), asked of the scene a written
/// GLB reads back as: each primitive whose material has a texture on uv set
/// n and has no set n. The GLB's `TEXCOORD_n` attributes are the scene's uv
/// sets, and a texture's `texCoord` its `uv_set`, so the reading says what
/// the document does.
fn texcoord_gaps(scene: &Scene) -> Vec<String> {
    let mut out = Vec::new();
    for (mi, mesh) in scene.meshes.iter().enumerate() {
        for (pi, p) in mesh.primitives.iter().enumerate() {
            let Some(m) = p.material.and_then(|m| scene.materials.get(m)) else { continue };
            for (slot, r) in m.texture_refs().filter(|(_, r)| r.uv_set as usize >= p.uvs.len()) {
                out.push(format!("mesh {mi} primitive {pi}: the {} map samples TEXCOORD_{}, which the primitive does not have", slot.as_str(), r.uv_set));
            }
        }
    }
    out
}

/// `scene` written as a GLB and read back.
fn through_glb(scene: &Scene) -> Scene {
    let glb = write_scene(scene, MeshFormat::Glb, WriteOptions::default()).expect("glb").package.files[0].bytes.clone();
    read_scene(&glb, MeshFormat::Glb, &NoSidecars).expect("read the glb").scene
}

/// Review Focus 2. A points primitive with texture coordinates and a material
/// with a base-colour map keeps both through glTF and USD: the coordinates
/// point by point, the map as its image's bytes, and nothing named lost. OBJ's
/// `p` names a vertex alone, so it drops the coordinates and the material's
/// map, each by name (the `uvs` and `textures` kinds), in a package and in
/// one file; and what an OBJ package then gives back goes on to glTF as a
/// valid document: no map on a primitive that has no `TEXCOORD` for it.
#[test]
fn textured_points_keep_their_texture() {
    let s = textured_points();
    let package = WriteOptions { self_contained: false, ..Default::default() };
    let points_of = |scene: &Scene| -> super::scene::Primitive {
        scene.meshes.iter().flat_map(|m| &m.primitives).find(|p| p.topology == Topology::Points).expect("a points primitive").clone()
    };
    let want = points_of(&s);
    // Point by point (a position and its coordinate), whatever order a
    // container keeps the points in.
    let pairs = |p: &super::scene::Primitive| -> Vec<([i64; 3], [i64; 2])> {
        let fixed = |x: f32| (f64::from(x) * 65536.0).round() as i64;
        let mut v: Vec<_> = p
            .indices
            .iter()
            .map(|&i| {
                let (pos, uv) = (p.positions[i as usize], p.uvs[0][i as usize]);
                ([fixed(pos[0]), fixed(pos[1]), fixed(pos[2])], [fixed(uv[0]), fixed(uv[1])])
            })
            .collect();
        v.sort_unstable();
        v
    };
    let mut swapped = want.clone();
    swapped.uvs[0].swap(0, 1);
    assert_ne!(pairs(&swapped), pairs(&want), "the comparison sees two points' coordinates swapped");
    for fmt in [MeshFormat::Glb, MeshFormat::Gltf, MeshFormat::Usdz, MeshFormat::Usda, MeshFormat::Usdc] {
        let (got, losses) = written(&s, fmt, package);
        assert!(!losses.iter().any(|l| matches!(l.kind, LossKind::Uvs | LossKind::Textures | LossKind::Points)), "{fmt}: {losses:?}");
        let p = points_of(&got);
        assert_eq!(p.uvs.len(), 1, "{fmt}: one uv set");
        assert_eq!(pairs(&p), pairs(&want), "{fmt}: each point keeps its own texture coordinate");
        let m = &got.materials[p.material.unwrap_or_else(|| panic!("{fmt}: the points lost their material"))];
        let map = m.base_color_texture.as_ref().unwrap_or_else(|| panic!("{fmt}: the material lost its map: {m:?}"));
        assert_eq!(got.images[got.textures[map.texture].image].data, s.images[0].data, "{fmt}: the map's image bytes");
    }
    for self_contained in [false, true] {
        let ctx = if self_contained { "obj (one file)" } else { "obj (a package)" };
        let (got, losses) = written(&s, MeshFormat::Obj, WriteOptions { self_contained, ..Default::default() });
        let p = points_of(&got);
        assert!(p.uvs.is_empty(), "{ctx}: `p` has no place for a texture coordinate");
        assert!(got.materials.iter().all(|m| m.texture_refs().next().is_none()), "{ctx}: no map on any material");
        assert!(got.textures.is_empty() && got.images.is_empty(), "{ctx}: no image shipped for a map nothing uses");
        for kind in [LossKind::Uvs, LossKind::Textures] {
            assert!(losses.iter().any(|l| l.kind == kind), "{ctx}: {kind} not named: {losses:?}");
        }
    }
    // The OBJ package's points go on to glTF as a valid document...
    let (obj, _) = written(&s, MeshFormat::Obj, package);
    assert_eq!(texcoord_gaps(&through_glb(&obj)), Vec::<String>::new());
    // ...and the check is of what it says: the same points with their
    // coordinates gone and the map kept (what the OBJ package gave before
    // the points had a material of their own) fail it.
    let mut gapped = s.clone();
    gapped.meshes[0].primitives[0].uvs.clear();
    let gaps = texcoord_gaps(&through_glb(&gapped));
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert!(gaps[0].contains("TEXCOORD_0"), "{gaps:?}");
}

/// A crate file made by a tool that is not ours (`usdcat`, Apple USD Tools
/// 0.25.2, from the USDA this module writes for the `wire` fixture) reads as
/// the fixture does: a `Points` prim and a `BasisCurves` prim among the
/// triangles, at every time of the clip, with the points' colours. The
/// reader meets crate-encoded points, widths and curve vertex counts here,
/// bytes our writer never produced.
#[test]
fn a_usdcat_crate_of_the_wire_fixture_reads_with_its_points_and_lines() {
    const WIRE_USDC: &[u8] = include_bytes!("testdata/usd/wire.usdc");
    let fixture = fixtures::wire();
    let package = WriteOptions { self_contained: false, ..Default::default() };
    let want = MeshFormat::Usda.prepare(&fixture, package).expect("prepare");
    let r = read_scene(WIRE_USDC, MeshFormat::Usda, &NoSidecars).expect("read the crate");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let got = r.scene;
    let count = |s: &Scene, t: Topology| -> usize { s.meshes.iter().flat_map(|m| &m.primitives).filter(|p| p.topology == t).map(|p| p.indices.len() / t.indices_per_element()).sum() };
    assert_eq!(count(&got, Topology::Points), 9, "the 3 x 3 grid");
    assert_eq!(count(&got, Topology::Lines), 4, "the ring's four segments");
    assert_eq!(count(&got, Topology::Triangles), 12, "the cube");
    for t in [Topology::Points, Topology::Lines, Topology::Triangles] {
        assert_eq!(count(&got, t), count(&want, t), "{t:?} as prepared");
    }
    for time in [None, Some(0.0), Some(0.5), Some(1.0), Some(2.0)] {
        for t in [Topology::Points, Topology::Lines, Topology::Triangles] {
            let (a, b) = (drawn(&want, time, t), drawn(&got, time, t));
            assert!(!a.is_empty());
            let off = farthest(&a, &b);
            assert!(off < 1e-5, "{t:?} at {time:?}: {off} m from the fixture");
        }
    }
    // Each point keeps the colour it was written with.
    let colors_of = |s: &Scene| -> Vec<[f32; 4]> {
        let p = s.meshes.iter().flat_map(|m| &m.primitives).find(|p| p.topology == Topology::Points).expect("points");
        p.indices.iter().map(|&i| p.colors.as_ref().expect("point colours")[i as usize]).collect()
    };
    let (a, b) = (colors_of(&want), colors_of(&got));
    assert_eq!(a.len(), 9);
    assert!(a.iter().zip(&b).all(|(p, q)| (0..4).all(|k| (p[k] - q[k]).abs() < 1e-6)), "{a:?} vs {b:?}");
    assert_ne!(a[0], a[8], "the colours differ from point to point, so the comparison tells points apart");
}
