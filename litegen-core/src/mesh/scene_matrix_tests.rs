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
//! where the capabilities say they do.

use super::fixtures;
use super::package::PackageFile;
use super::pose;
use super::scene::{MaterialSupport, Scene};
use super::*;

struct Shape {
    triangles: usize,
    area: f64,
    volume: f64,
    lo: [f64; 3],
    hi: [f64; 3],
    centroid: [f64; 3],
}

fn shape(scene: &Scene, anim_time: Option<f32>) -> Shape {
    let pose = match anim_time {
        Some(t) if !scene.animations.is_empty() => pose::sample_pose(scene, 0, t),
        _ => pose::default_pose(scene),
    };
    let mut s = Shape { triangles: 0, area: 0.0, volume: 0.0, lo: [f64::INFINITY; 3], hi: [f64::NEG_INFINITY; 3], centroid: [0.0; 3] };
    for wp in pose::world_primitives(scene, &pose).expect("pose") {
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

/// Write, then read back through the package's own files.
fn round(scene: &Scene, fmt: MeshFormat, opts: WriteOptions) -> Scene {
    let w = write_scene(scene, fmt, opts).unwrap_or_else(|e| panic!("write {fmt}: {e}"));
    let files: Vec<PackageFile> = w.package.files.clone();
    let side = super::package::MemorySidecars { files: &files };
    read_scene(&files[0].bytes, fmt, &side).unwrap_or_else(|e| panic!("read {fmt}: {e}")).scene
}

fn check_pair(name: &str, fixture: &Scene, from: MeshFormat, to: MeshFormat, opts: WriteOptions) {
    let ctx = format!("{name}: {from} -> {to}{}", if opts.self_contained { " (single file)" } else { "" });
    let expected = from
        .prepare(fixture, opts)
        .and_then(|s| to.prepare(&s, opts))
        .unwrap_or_else(|e| panic!("{ctx}: prepare: {e}"));
    let source = round(fixture, from, opts);
    let got = round(&source, to, opts);

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
}

#[test]
fn every_fixture_survives_every_direction_as_a_package() {
    let opts = WriteOptions { self_contained: false };
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
    let opts = WriteOptions { self_contained: true };
    for (name, fixture) in fixtures::all() {
        for from in MeshFormat::ALL {
            for to in MeshFormat::ALL {
                check_pair(name, &fixture, from, to, opts);
            }
        }
    }
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
