//! Normalised delivery (PixelGenie 3D doc §2 rule 3, §3D).
//!
//! The one step both observers of a 3D job's completion — the client's
//! `GET /v1/models3d/{id}` and the background poller — run on the files a
//! vendor returned, before re-hosting them, so whichever sees completion first
//! delivers the same thing:
//!
//! 1. Read the vendor's mesh with litegen's own reader. One litegen cannot read
//!    (Draco, meshopt) fails the delivery: a mesh litegen cannot describe is
//!    one whose conventions it cannot promise.
//! 2. Normalise it: turn it to face +Z when its model's proven orientation
//!    says it does not; make node names unique; rename joints and clips as the
//!    request asked; hold an in-place clip's root still where the vendor did
//!    not; transcode WebP textures to PNG (and drop KTX2 ones), so no delivered
//!    glTF requires an extension.
//! 3. Deliver every requested container — from the vendor's own files when
//!    nothing changed, from the normalised scene when something did, with the
//!    vendor's files then kept as `original` assets.
//! 4. Measure every delivered mesh into its manifest.

use serde::{Deserialize, Serialize};

use crate::mesh::normalize::{self, Axis};
use crate::mesh::{MeshFormat, WriteOptions};
use crate::providers::Model3dFile;
use crate::types::{Model3dAssetKind, SceneAnimation, SceneBounds, SceneManifest, SceneMaterial, ScenePart, SceneSkin};

/// A model's proven native orientation: the axes its vendor builds up and
/// front along.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NativeOrientationPlan {
    pub up: String,
    pub front: String,
}

/// One clip the request named, in request order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClipPlan {
    pub name: String,
    /// Whether the clip must stay in place. One that arrives in place was
    /// made so by the vendor; one that does not, litegen holds.
    #[serde(default)]
    pub in_place: Option<bool>,
    #[serde(default)]
    pub loop_requested: Option<bool>,
}

/// What a generation's delivery must do, fixed at submit and recorded on its
/// row (`metadata.delivery`), so the poller delivers exactly what the client's
/// poll would.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeliveryPlan {
    /// The containers promised to the caller.
    pub requested_formats: Vec<String>,
    /// `None` when the model's orientation is unproven: nothing is turned and
    /// the manifest says `front: "unverified"`.
    #[serde(default)]
    pub orientation: Option<NativeOrientationPlan>,
    /// Joint renames `(from, to)` litegen applies.
    #[serde(default)]
    pub joint_renames: Vec<(String, String)>,
    /// Clips the request named, in order: they are named so.
    #[serde(default)]
    pub clips: Vec<ClipPlan>,
    /// The skeleton the request asked the rig to be (`humanoid`, `quadruped`, …).
    #[serde(default)]
    pub skeleton: Option<String>,
    /// The frame rate every clip is keyed at.
    #[serde(default)]
    pub fps: Option<u32>,
    /// A segment model's mesh (3D doc §3F): every part named, and listed in
    /// the manifest.
    #[serde(default)]
    pub parts: bool,
    /// Opt-in placement (3D doc §3D), applied after orientation.
    #[serde(default)]
    pub placement: Option<PlacementPlan>,
    /// The joint-naming scheme asked for (3D doc §3B), when not `vendor`:
    /// litegen renames a humanoid rig it can read into it.
    #[serde(default)]
    pub joint_naming: Option<String>,
}

/// Where the request asked its mesh to be (3D doc §3D).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlacementPlan {
    #[serde(default)]
    pub ground: bool,
    #[serde(default)]
    pub center: bool,
    #[serde(default)]
    pub height_m: Option<f64>,
}

impl DeliveryPlan {
    /// What delivering a request for `schema` must do, fixed at submit.
    pub fn for_request(schema: &crate::capabilities::ModelSchema, extras: &crate::providers::Model3dExtras) -> DeliveryPlan {
        DeliveryPlan {
            requested_formats: if extras.output_formats.is_empty() {
                vec![crate::types::DEFAULT_MODEL3D_FORMAT.to_string()]
            } else {
                extras.output_formats.clone()
            },
            orientation: match schema.native_orientation.as_ref() {
                // An operation that keeps its source's axes faces where the
                // source's own manifest says it did — proven, or unverified.
                Some(o) if o.up == "source" => extras
                    .source
                    .as_ref()
                    .and_then(|s| s.front.as_deref())
                    .filter(|f| *f == "+z")
                    .map(|_| NativeOrientationPlan { up: "+y".into(), front: "+z".into() }),
                Some(o) => Some(NativeOrientationPlan { up: o.up.clone(), front: o.front.clone() }),
                None => None,
            },
            skeleton: extras.skeleton.clone(),
            clips: extras
                .animations
                .iter()
                .map(|c| ClipPlan { name: c.name.clone(), in_place: c.in_place, loop_requested: c.loop_requested })
                .collect(),
            fps: extras.fps,
            parts: schema.task == crate::capabilities::ModelTask::Segment,
            joint_naming: extras.joint_naming.clone().filter(|n| n != "vendor"),
            placement: {
                let p = PlacementPlan {
                    ground: extras.ground == Some(true),
                    center: extras.center == Some(true),
                    height_m: extras.height_m,
                };
                (p.ground || p.center || p.height_m.is_some()).then_some(p)
            },
            ..Default::default()
        }
    }

    /// The plan a row recorded at submit, else just its promised formats.
    pub fn from_row(gen: &crate::types::Generation) -> DeliveryPlan {
        let recorded = gen
            .metadata
            .as_ref()
            .and_then(|m| m.get("delivery"))
            .and_then(|d| serde_json::from_value::<DeliveryPlan>(d.clone()).ok());
        let mut plan = recorded.unwrap_or_default();
        if plan.requested_formats.is_empty() {
            plan.requested_formats = crate::api::handlers::row_requested_formats(gen);
        }
        plan
    }
}

/// What a delivery produced.
#[derive(Debug, Default)]
pub struct Delivered {
    /// Meshes (each with its manifest), originals, textures and previews.
    pub files: Vec<Model3dFile>,
    /// Set when the delivery failed: the generation fails with this error.
    pub fatal: Option<String>,
    /// Requested containers that could not be produced, and why.
    pub failures: Vec<(String, String)>,
}

/// `extensionsUsed` and `extensionsRequired` of a glTF or GLB.
fn gltf_extensions(bytes: &[u8], format: &str) -> (Vec<String>, Vec<String>) {
    let json: Option<serde_json::Value> = match format {
        "glb" if bytes.len() >= 20 && &bytes[0..4] == b"glTF" => {
            let len = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
            bytes.get(20..20 + len).and_then(|j| serde_json::from_slice(j).ok())
        }
        "gltf" => serde_json::from_slice(bytes).ok(),
        _ => None,
    };
    let list = |doc: &serde_json::Value, key: &str| -> Vec<String> {
        doc.get(key)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    match json {
        Some(doc) => (list(&doc, "extensionsUsed"), list(&doc, "extensionsRequired")),
        None => (Vec::new(), Vec::new()),
    }
}

/// Measure one delivered mesh. `None` when it cannot be read back.
fn manifest(
    bytes: &[u8],
    format: &str,
    plan: &DeliveryPlan,
    oriented_by: &str,
    litegen_in_place: &[bool],
) -> Option<SceneManifest> {
    let scene = crate::mesh::read_scene(bytes, MeshFormat::parse(format)?, &crate::mesh::package::NoSidecars).ok()?.scene;
    let facts = crate::mesh::inspect::inspect(&scene).ok()?;
    let skin = facts.skin.map(|s| SceneSkin {
        skeleton: if s.humanoid.is_some() { Some("humanoid".into()) } else { plan.skeleton.clone() },
        joint_count: s.joints.len() as u32,
        root: s.root,
        bind_pose: s.bind_pose.as_str().into(),
        joints: s.joints,
        humanoid: s.humanoid.map(|(_, map)| map.into_iter().collect()),
    });
    let animations = facts
        .animations
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let in_place = c.in_place();
            let by_litegen = litegen_in_place.get(i).copied().unwrap_or(false);
            SceneAnimation {
                // As the file names it: delivery renamed the clips already,
                // and the manifest vouches only for what it read.
                name: c.name.clone().unwrap_or_else(|| format!("clip_{i}")),
                duration_s: c.duration_s,
                in_place,
                in_place_by: in_place.then(|| if by_litegen { "litegen" } else { "vendor" }.to_string()),
                root_travel_m: c.root_travel_m,
                loop_requested: plan.clips.get(i).and_then(|p| p.loop_requested),
            }
        })
        .collect();
    Some(SceneManifest {
        units: "m".into(),
        up: "+y".into(),
        front: if plan.orientation.is_some() { "+z" } else { "unverified" }.into(),
        oriented_by: oriented_by.into(),
        bounds: facts.bounds.map(|(min, max)| SceneBounds { min, max }),
        vertex_count: facts.vertex_count as u64,
        triangle_count: facts.triangle_count as u64,
        uv_sets: facts.uv_sets as u32,
        vertex_colors: facts.vertex_colors,
        materials: facts
            .materials
            .into_iter()
            .map(|m| SceneMaterial { name: m.name, maps: m.maps.into_iter().map(str::to_string).collect() })
            .collect(),
        skin,
        animations,
        extensions_used: gltf_extensions(bytes, format).0,
        parts: if plan.parts {
            let parts = crate::mesh::inspect::parts(&scene).ok()?;
            Some(
                parts
                    .into_iter()
                    .enumerate()
                    .map(|(n, p)| ScenePart {
                        name: p.name.unwrap_or_else(|| format!("part_{n}")),
                        node: p.node as u32,
                        triangle_count: p.triangle_count as u64,
                        bounds: p.bounds.map(|(min, max)| SceneBounds { min, max }),
                    })
                    .collect(),
            )
        } else {
            None
        },
    })
}

/// Deliver the files of the poll that saw a job complete — what both
/// observers run, so whichever sees completion first delivers the same thing.
/// The work is CPU-bound and runs on a blocking thread; a failed delivery
/// fails the poll.
pub async fn deliver_poll(poll: &mut crate::providers::Model3dGenerationPollResult, plan: DeliveryPlan, id: &str) {
    if poll.status != crate::types::GenerationStatus::Completed || poll.files.is_empty() {
        return;
    }
    let files = std::mem::take(&mut poll.files);
    let delivered = tokio::task::spawn_blocking(move || deliver(files, &plan))
        .await
        .unwrap_or_else(|e| Delivered { fatal: Some(format!("the delivery step failed: {e}")), ..Default::default() });
    for (what, why) in &delivered.failures {
        tracing::warn!(generation_id = %id, what = %what, reason = %why, "3d delivery could not produce something");
    }
    match delivered.fatal {
        Some(e) => {
            tracing::warn!(generation_id = %id, error = %e, "3d delivery failed");
            poll.status = crate::types::GenerationStatus::Failed;
            poll.error = Some(e);
        }
        None => poll.files = delivered.files,
    }
}

/// Normalise and measure a vendor's files. CPU work: run on a blocking thread.
pub fn deliver(files: Vec<Model3dFile>, plan: &DeliveryPlan) -> Delivered {
    let requested: Vec<String> = if plan.requested_formats.is_empty() {
        vec![crate::types::DEFAULT_MODEL3D_FORMAT.to_string()]
    } else {
        plan.requested_formats.iter().map(|f| f.to_ascii_lowercase()).collect()
    };
    let (vendor_meshes, others): (Vec<Model3dFile>, Vec<Model3dFile>) =
        files.into_iter().partition(|f| f.kind == Model3dAssetKind::Mesh);
    if vendor_meshes.is_empty() {
        // The completion guard names this ("success without a mesh asset").
        return Delivered { files: others, ..Default::default() };
    }
    let primary_ix = vendor_meshes
        .iter()
        .position(|f| f.format.eq_ignore_ascii_case("glb"))
        .unwrap_or(0);
    let primary = &vendor_meshes[primary_ix];
    let primary_format = primary.format.to_ascii_lowercase();
    let read = MeshFormat::parse(&primary_format)
        .ok_or_else(|| format!("'{primary_format}' is not a container litegen reads"))
        .and_then(|f| crate::mesh::read_scene(&primary.bytes, f, &crate::mesh::package::NoSidecars).map_err(|e| e.to_string()));
    let mut scene = match read {
        Ok(r) => r.scene,
        Err(e) => {
            let have: Vec<String> = vendor_meshes.iter().map(|m| m.format.to_ascii_lowercase()).collect();
            let missing = crate::types::missing_model3d_formats(&requested, &have);
            let fatal = if missing.is_empty() {
                format!("the provider's {primary_format} could not be read ({e}); litegen delivers only meshes it can read")
            } else {
                format!(
                    "provider did not return the requested format(s): {}; its {primary_format} could not be read ({e}), so nothing can be derived",
                    missing.join(", ")
                )
            };
            return Delivered { fatal: Some(fatal), ..Default::default() };
        }
    };

    // ── Normalise ──
    let mut changed = !gltf_extensions(&primary.bytes, &primary_format).1.is_empty();
    let mut oriented_by = "vendor";
    if let Some(o) = &plan.orientation {
        match (Axis::parse(&o.up), Axis::parse(&o.front)) {
            (Some(up), Some(front)) => match normalize::orient(&mut scene, up, front) {
                Ok(true) => {
                    changed = true;
                    oriented_by = "litegen";
                }
                Ok(false) => {}
                Err(e) => return Delivered { fatal: Some(format!("the model's declared orientation is unusable: {e}")), ..Default::default() },
            },
            _ => return Delivered { fatal: Some(format!("the model's declared orientation {}/{} is not a pair of axes", o.up, o.front)), ..Default::default() },
        }
    }
    if let Some(p) = &plan.placement {
        match normalize::place(&mut scene, p.ground, p.center, p.height_m) {
            Ok(placed) => changed |= placed,
            Err(e) => return Delivered { fatal: Some(format!("the mesh could not be placed: {e}")), ..Default::default() },
        }
    }
    if plan.parts {
        changed |= normalize::name_parts(&mut scene);
    }
    changed |= normalize::unique_node_names(&mut scene);
    changed |= normalize::rename_nodes(&mut scene, &plan.joint_renames);
    // 3D doc §3B: a scheme the vendor could not name in, from the table
    // litegen reads the rig with. Already so named (the vendor did it), it is
    // a no-op; a scheme litegen has no table for was the vendor's to apply.
    let mut naming_note = None;
    if let Some(scheme) = plan.joint_naming.as_deref().filter(|s| crate::mesh::humanoid::RENAMES_TO.contains(s)) {
        let joints: Vec<String> = scene
            .skins
            .first()
            .map(|s| s.joints.iter().map(|&j| scene.nodes[j].name.clone().unwrap_or_default()).collect())
            .unwrap_or_default();
        match crate::mesh::humanoid::renames_to(scheme, &joints) {
            Some(renames) => changed |= normalize::rename_nodes(&mut scene, &renames),
            None => naming_note = Some(format!("litegen cannot read this rig's joints as a humanoid, so they keep the vendor's names, not {scheme} ones")),
        }
    }
    // One clip per requested name, in order (3D doc §3C): a promise like the
    // containers. A pose clip a vendor adds beside them is not one of them.
    if !plan.clips.is_empty() {
        if scene.animations.len() > plan.clips.len() && normalize::drop_static_animations(&mut scene) > 0 {
            changed = true;
        }
        if scene.animations.len() != plan.clips.len() {
            return Delivered {
                fatal: Some(format!(
                    "the provider returned {} clip(s) for the {} requested ({})",
                    scene.animations.len(),
                    plan.clips.len(),
                    plan.clips.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
                )),
                ..Default::default()
            };
        }
    }
    let clip_names: Vec<String> = plan.clips.iter().map(|c| c.name.clone()).collect();
    changed |= normalize::name_animations(&mut scene, &clip_names);
    if let Some(fps) = plan.fps {
        for i in 0..scene.animations.len() {
            changed |= normalize::resample(&mut scene, i, fps);
        }
    }
    let mut litegen_in_place = vec![false; scene.animations.len()];
    if let Ok(facts) = crate::mesh::inspect::inspect(&scene) {
        for (i, clip) in plan.clips.iter().enumerate() {
            let measured = facts.animations.get(i).map(|c| c.in_place()).unwrap_or(true);
            if clip.in_place == Some(true) && !measured && normalize::remove_root_motion(&mut scene, i) {
                litegen_in_place[i] = true;
                changed = true;
            }
        }
    }
    let transcode = normalize::transcode_textures(&mut scene);
    changed |= transcode.changed;
    let mut failures: Vec<(String, String)> =
        transcode.dropped.into_iter().map(|t| ("texture".to_string(), format!("{t} could not be decoded and was left out (KTX2)"))).collect();
    failures.extend(naming_note.map(|n| ("joint_naming".to_string(), n)));

    // ── Deliver ──
    let mut out: Vec<Model3dFile> = Vec::new();
    if !changed {
        let (files, report) = crate::proxy::model3d_convert::synthesize_missing_formats(vendor_meshes, &requested);
        failures.extend(report.failures);
        out.extend(files);
    } else {
        for fmt in &requested {
            let Some(target) = MeshFormat::parse(fmt) else {
                failures.push((fmt.clone(), "not a container litegen writes".into()));
                continue;
            };
            match crate::mesh::write_scene(&scene, target, WriteOptions::default()) {
                Ok(w) => match w.package.files.into_iter().next() {
                    Some(file) => out.push(Model3dFile {
                        kind: Model3dAssetKind::Mesh,
                        format: target.as_str().to_string(),
                        content_type: crate::proxy::storage::model3d_content_type(target.as_str(), "application/octet-stream").to_string(),
                        bytes: file.bytes,
                        polycount: Some(scene.triangle_count() as u32),
                        width: None,
                        height: None,
                        manifest: None,
                    }),
                    None => failures.push((fmt.clone(), "the writer produced no file".into())),
                },
                Err(e) => failures.push((fmt.clone(), e.to_string())),
            }
        }
        for mut m in vendor_meshes {
            m.kind = Model3dAssetKind::Original;
            out.push(m);
        }
    }
    for f in out.iter_mut().filter(|f| f.kind == Model3dAssetKind::Mesh) {
        f.manifest = manifest(&f.bytes, &f.format.to_ascii_lowercase(), plan, oriented_by, &litegen_in_place);
    }
    out.extend(others);
    Delivered { files: out, fatal: None, failures }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Model3dFile;
    use crate::types::Model3dAssetKind;

    fn mesh(format: &str, bytes: Vec<u8>) -> Model3dFile {
        Model3dFile {
            kind: Model3dAssetKind::Mesh,
            format: format.into(),
            content_type: "model/gltf-binary".into(),
            bytes,
            polycount: None,
            width: None,
            height: None,
            manifest: None,
        }
    }

    fn plan(formats: &[&str], orientation: Option<(&str, &str)>) -> DeliveryPlan {
        DeliveryPlan {
            requested_formats: formats.iter().map(|f| f.to_string()).collect(),
            orientation: orientation.map(|(up, front)| NativeOrientationPlan { up: up.into(), front: front.into() }),
            ..Default::default()
        }
    }

    fn meshes(d: &Delivered) -> Vec<&Model3dFile> {
        d.files.iter().filter(|f| f.kind == Model3dAssetKind::Mesh).collect()
    }

    fn scene_of(bytes: &[u8], format: &str) -> crate::mesh::scene::Scene {
        crate::mesh::read_scene(bytes, crate::mesh::MeshFormat::parse(format).unwrap(), &crate::mesh::package::NoSidecars).unwrap().scene
    }

    /// The mock humanoid turned to face +X, as a vendor building +X-forward does.
    fn turned_humanoid() -> Vec<u8> {
        crate::providers::model3d::humanoid::turned_humanoid_glb()
    }

    #[test]
    fn a_conforming_mesh_passes_through_with_a_measured_manifest() {
        let cube = crate::providers::model3d::glb::generate_cube_glb("a crate");
        let d = deliver(vec![mesh("glb", cube.clone())], &plan(&["glb"], Some(("+y", "+z"))));
        assert!(d.fatal.is_none());
        let m = meshes(&d);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].bytes, cube, "nothing to change, nothing changed");
        assert!(!d.files.iter().any(|f| f.kind == Model3dAssetKind::Original));
        let s = m[0].manifest.as_ref().unwrap();
        assert_eq!((s.units.as_str(), s.up.as_str(), s.front.as_str(), s.oriented_by.as_str()), ("m", "+y", "+z", "vendor"));
        // The mock cube exactly: 8 shared corners at ±its prompt-seeded scale,
        // 12 triangles, no UVs, one untextured material.
        let half = crate::providers::model3d::glb::cube_scale("a crate");
        let b = s.bounds.as_ref().unwrap();
        for axis in 0..3 {
            assert!((b.min[axis] + half).abs() < 1e-6 && (b.max[axis] - half).abs() < 1e-6, "{b:?} vs ±{half}");
        }
        assert_eq!((s.vertex_count, s.triangle_count, s.uv_sets, s.vertex_colors), (8, 12, 0, false));
        assert_eq!(s.materials, [crate::types::SceneMaterial { name: Some("generated".into()), maps: vec![] }]);
        assert!(s.extensions_used.is_empty());
        assert!(s.skin.is_none());
        assert!(s.animations.is_empty());
    }

    #[test]
    fn duplicate_joint_names_are_delivered_with_numeric_suffixes() {
        let vendor = crate::providers::model3d::humanoid::rigged_humanoid_glb(&["hips", "spine", "head", "arm", "arm", "leg", "leg"]);
        let d = deliver(vec![mesh("glb", vendor.clone())], &plan(&["glb"], Some(("+y", "+z"))));
        let skin = meshes(&d)[0].manifest.clone().unwrap().skin.unwrap();
        assert_eq!(skin.joints, ["hips", "spine", "head", "arm", "arm_1", "leg", "leg_1"]);
        assert_eq!(d.files.iter().find(|f| f.kind == Model3dAssetKind::Original).unwrap().bytes, vendor);
    }

    #[test]
    fn an_unproven_orientation_is_reported_never_guessed() {
        let cube = crate::providers::model3d::glb::generate_cube_glb("a crate");
        let d = deliver(vec![mesh("glb", cube.clone())], &plan(&["glb"], None));
        let s = meshes(&d)[0].manifest.clone().unwrap();
        assert_eq!(s.front, "unverified");
        assert_eq!(s.oriented_by, "vendor");
        assert_eq!(meshes(&d)[0].bytes, cube);
    }

    #[test]
    fn a_turned_mesh_is_rotated_to_face_z_and_the_vendors_file_kept() {
        let vendor = turned_humanoid();
        let d = deliver(vec![mesh("glb", vendor.clone())], &plan(&["glb", "obj"], Some(("+y", "+x"))));
        assert!(d.fatal.is_none(), "{:?}", d.fatal);
        let original = d.files.iter().find(|f| f.kind == Model3dAssetKind::Original).expect("the vendor's own file");
        assert_eq!(original.bytes, vendor);
        for m in meshes(&d) {
            let s = m.manifest.as_ref().unwrap();
            assert_eq!((s.front.as_str(), s.oriented_by.as_str()), ("+z", "litegen"), "{}", m.format);
            // The nose is on the front: +Z after delivery.
            let b = s.bounds.as_ref().unwrap();
            assert!(b.max[2] > 0.15 && b.min[2] > -0.12, "{}: {:?}", m.format, b);
            assert!(b.max[0] > 0.7 && b.min[0] < -0.7, "arms along ±X: {:?}", b);
        }
        assert_eq!(meshes(&d).iter().map(|m| m.format.as_str()).collect::<Vec<_>>(), ["glb", "obj"]);
    }

    #[test]
    fn a_webp_textured_mesh_needs_no_extension_once_delivered() {
        let mut scene = crate::mesh::fixtures::textured();
        let img = image::load_from_memory_with_format(&scene.images[0].data, image::ImageFormat::Png).unwrap();
        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
            .encode(img.as_bytes(), img.width(), img.height(), img.color().into())
            .unwrap();
        scene.images[0].format = crate::mesh::scene::ImageFormat::Webp;
        scene.images[0].data = webp;
        let vendor = crate::mesh::glb::write_scene(&scene).unwrap();
        let d = deliver(vec![mesh("glb", vendor.clone())], &plan(&["glb"], Some(("+y", "+z"))));
        let m = meshes(&d)[0];
        assert!(m.manifest.as_ref().unwrap().extensions_used.iter().all(|e| e != "EXT_texture_webp"), "{:?}", m.manifest);
        assert_eq!(scene_of(&m.bytes, "glb").images[0].format, crate::mesh::scene::ImageFormat::Png);
        assert_eq!(d.files.iter().find(|f| f.kind == Model3dAssetKind::Original).unwrap().bytes, vendor);
    }

    #[test]
    fn a_mesh_litegen_cannot_read_fails_the_delivery_and_names_what_is_missing() {
        let draco = crate::providers::model3d::mock::generate_undecodable_glb("a fox");
        let only_glb = deliver(vec![mesh("glb", draco.clone())], &plan(&["glb"], Some(("+y", "+z"))));
        assert!(only_glb.fatal.as_deref().is_some_and(|e| e.contains("could not be read")), "{:?}", only_glb.fatal);
        let with_fbx = deliver(vec![mesh("glb", draco)], &plan(&["glb", "fbx"], Some(("+y", "+z"))));
        assert!(with_fbx.fatal.as_deref().is_some_and(|e| e.contains("fbx")), "{:?}", with_fbx.fatal);
    }

    #[test]
    fn a_rig_no_table_knows_reports_the_requested_skeleton_and_no_humanoid_map() {
        let mut models = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        models.pop();
        models.push("models");
        let schema = crate::capabilities::CapabilityRegistry::from_dir(&models).unwrap().get("mock/rig-3d").unwrap().clone();
        let extras = crate::providers::Model3dExtras {
            output_formats: vec!["glb".into()],
            texture: None, pbr: None, target_polycount: None, symmetry: None, topology: None, rig: None,
            skeleton: Some("quadruped".into()), joint_naming: None, height_m: None,
            extra: None, source: None, preflight: None,
            animations: Vec::new(), fps: None,
            texture_resolution: None, keep_uvs: None,
            ground: None, center: None,
        };
        let plan = DeliveryPlan::for_request(&schema, &extras);
        assert_eq!(plan.skeleton.as_deref(), Some("quadruped"));
        let rig = crate::providers::model3d::humanoid::rigged_humanoid_glb(&["b0", "b1", "b2", "b3", "b4", "b5", "b6"]);
        let skin = meshes(&deliver(vec![mesh("glb", rig)], &plan))[0].manifest.clone().unwrap().skin.unwrap();
        assert_eq!(skin.skeleton.as_deref(), Some("quadruped"));
        assert!(skin.humanoid.is_none(), "never a guess: {:?}", skin.humanoid);
    }

    /// The mock humanoid carrying `clips` (name, key times) on its hips.
    fn animated(clips: &[(&str, &[f32])]) -> Vec<u8> {
        use crate::mesh::scene::{AnimPath, Animation, Channel, Interpolation};
        let mut s = crate::providers::model3d::humanoid::rigged_humanoid(&crate::providers::model3d::humanoid::MOCK_JOINTS);
        for (name, times) in clips {
            s.animations.push(Animation {
                name: Some(name.to_string()),
                channels: vec![Channel {
                    node: 0,
                    path: AnimPath::Translation,
                    interpolation: Interpolation::Linear,
                    times: times.to_vec(),
                    values: times.iter().flat_map(|t| [0.0, 0.95, *t]).collect(),
                }],
            });
        }
        crate::mesh::glb::write_scene(&s).unwrap()
    }

    fn clip_plan(names: &[&str]) -> DeliveryPlan {
        DeliveryPlan {
            clips: names.iter().map(|n| ClipPlan { name: n.to_string(), in_place: Some(false), loop_requested: None }).collect(),
            ..plan(&["glb"], Some(("+y", "+z")))
        }
    }

    #[test]
    fn the_manifest_names_each_clip_as_the_file_does() {
        // Measured, never taken from the request: a manifest that echoed the
        // asked-for names would vouch for a file that does not carry them.
        let file = animated(&[("Armature|walk", &[0.0, 1.0])]);
        let m = manifest(&file, "glb", &clip_plan(&["walk"]), "vendor", &[]).unwrap();
        assert_eq!(m.animations[0].name, "Armature|walk");
    }

    #[test]
    fn a_vendor_returning_fewer_clips_than_asked_fails_the_delivery() {
        let d = deliver(vec![mesh("glb", animated(&[("Armature|walk", &[0.0, 1.0])]))], &clip_plan(&["walk", "idle"]));
        assert!(d.fatal.as_deref().is_some_and(|e| e.contains("1 clip(s) for the 2 requested")), "{:?}", d.fatal);
    }

    #[test]
    fn a_pose_clip_beside_the_asked_ones_is_dropped_and_the_rest_named_in_order() {
        let vendor = animated(&[("Armature|clip0|baselayer", &[0.0]), ("Armature|walk", &[0.0, 1.0]), ("Armature|run", &[0.0, 0.5])]);
        let d = deliver(vec![mesh("glb", vendor)], &clip_plan(&["walk", "run"]));
        assert!(d.fatal.is_none(), "{:?}", d.fatal);
        let names: Vec<String> = meshes(&d)[0].manifest.as_ref().unwrap().animations.iter().map(|a| a.name.clone()).collect();
        assert_eq!(names, ["walk", "run"]);
    }

    #[test]
    fn a_rig_reports_its_skin_and_humanoid_map() {
        let rig = crate::providers::model3d::humanoid::rigged_humanoid_glb(&crate::providers::model3d::humanoid::MOCK_JOINTS);
        let d = deliver(vec![mesh("glb", rig)], &plan(&["glb"], Some(("+y", "+z"))));
        let skin = meshes(&d)[0].manifest.clone().unwrap().skin.unwrap();
        assert_eq!(skin.joint_count, 7);
        assert_eq!(skin.root.as_deref(), Some("hips"));
        assert_eq!(skin.skeleton.as_deref(), Some("humanoid"));
        assert_eq!(skin.bind_pose, "t");
        let h = skin.humanoid.unwrap();
        assert_eq!(h.get("spine").map(String::as_str), Some("torso"));
        assert_eq!(h.get("leftUpperArm").map(String::as_str), Some("arm_l"));
    }
}
