//! Normalised delivery (PixelGenie 3D doc §2 rule 3, §3D).
//!
//! The one step both observers of a 3D job's completion — the client's
//! `GET /v1/models3d/{id}` and the background poller — run on the files a
//! vendor returned, before re-hosting them, so whichever sees completion first
//! delivers the same thing:
//!
//! 1. Read the vendor's mesh with litegen's own reader, which decodes Draco
//!    and meshopt compression. One it cannot read (one that requires an
//!    extension litegen does not implement, say) fails the delivery: a mesh
//!    litegen cannot describe is one whose conventions it cannot promise.
//! 2. Normalise it: turn it to face +Z when its model's proven orientation
//!    says it does not; make node names unique; rename joints and clips as the
//!    request asked; hold an in-place clip's root still where the vendor did
//!    not; transcode WebP and KTX2 (Basis Universal) textures to PNG, so no
//!    delivered glTF requires an extension. With `ktx2_textures: keep` the
//!    glb and gltf keep their KTX2 textures instead (WebP still becomes PNG),
//!    and every other container gets PNG.
//! 3. Deliver every requested container — from the vendor's own files when
//!    nothing changed, from the normalised scene when something did, with the
//!    vendor's files then kept as `original` assets. A vendor glTF that
//!    requires an extension, or uses a compression one, counts as changed:
//!    every glTF litegen delivers is uncompressed.
//! 4. Measure every delivered mesh into its manifest.
//!
//! Only one observer runs it per job. Both can see the same finish, and a
//! delivery is the costly part of a poll (1.2 s and ~205 MB for a
//! 216k-triangle model, 2026-10-12 review #30), so each claims it first
//! ([`claim_delivery`](crate::proxy::delivery_lease::claim_delivery)): a slot of this process's
//! [`delivery_slots`](crate::proxy::delivery_lease::delivery_slots), then a
//! lease on the row that every process respects. An observer that gets
//! neither reports the job still processing ([`stand_aside`]), and the
//! holder's terminal write answers its next poll.

use serde::{Deserialize, Serialize};

use crate::mesh::normalize::{self, Axis};
use crate::mesh::{pose::MAX_RESAMPLED_VALUES, scene::MAX_ANIMATION_KEYS, MeshFormat, WriteOptions};
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
    /// The joint-naming scheme asked for (3D doc §3B), when not `vendor`, or,
    /// for an animation that asks none, the one its rig was delivered in:
    /// litegen renames a humanoid rig it can read into it.
    #[serde(default)]
    pub joint_naming: Option<String>,
    /// How FBX material and light colours are stored, for the vendor's FBX when it is
    /// the source and for every FBX delivered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fbx_color_space: Option<crate::types::Model3dFbxColorSpace>,
    /// The units FBX light strengths are stored in, for the vendor's FBX when
    /// it is the source and for every FBX litegen writes (a vendor FBX
    /// delivered as it is keeps its own). Absent when the request asked none,
    /// as from every plan recorded before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fbx_light_units: Option<crate::types::Model3dLightUnits>,
    /// The units USD light strengths are stored in, for the vendor's USD when
    /// it is the source and for every usdz, usda and usdc litegen writes (a vendor
    /// USD delivered as it is keeps its own). Absent when the request asked
    /// none, as from every plan recorded before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd_light_units: Option<crate::types::Model3dLightUnits>,
    /// How the Collada litegen writes stores a mesh's morph targets (a vendor
    /// Collada delivered as it is keeps its own). Absent when the request
    /// asked none, as from every plan recorded before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dae_morphs: Option<crate::types::Model3dDaeMorphs>,
    /// What the root layer of a usdz litegen writes is (a vendor USDZ
    /// delivered as it is keeps its own). Absent when the request asked none,
    /// as from every plan recorded before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usdz_root: Option<crate::types::Model3dUsdzRoot>,
    /// Which clips a usda, usdc or usdz litegen writes holds (a vendor USD
    /// delivered as it is keeps its own). Absent when the request asked none,
    /// as from every plan recorded before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd_clips: Option<crate::types::Model3dUsdClips>,
    /// The generation keeps nothing (`keep_outputs: false`): the vendor's files
    /// are handed out as they came, so nothing is read, turned or converted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep_nothing: bool,
    /// `output_packages`: also deliver each requested container's package as
    /// a zip, when it is more than one file. Absent from a plan that asks
    /// none, as from every plan recorded before it existed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub packages: bool,
    /// `ktx2_textures`: `keep` writes `glb` and `gltf` with the KTX2
    /// textures the mesh came with; absent for `png`, the default, as from
    /// every plan recorded before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ktx2_textures: Option<crate::types::Model3dKtx2Textures>,
    /// Set when the plan recorded on a row could not be read: the delivery
    /// goes ahead with what is known and says so on every mesh, instead of
    /// quietly dropping the options the client's own poll applies.
    #[serde(skip)]
    pub unreadable: Option<String>,
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

/// How delivery reads a vendor file: an FBX's colours in the requested space,
/// and an FBX's and a USD's lights in the requested units.
fn read_options(plan: &DeliveryPlan) -> crate::mesh::ReadOptions {
    crate::mesh::ReadOptions {
        fbx_color_space: crate::proxy::model3d_convert::fbx_color_space(plan.fbx_color_space),
        fbx_light_units: crate::proxy::model3d_convert::light_units(plan.fbx_light_units),
        usd_light_units: crate::proxy::model3d_convert::light_units(plan.usd_light_units),
    }
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
            // 3D doc §3B: the naming the request names, `vendor` meaning none.
            // One that names none, animating a rig, keeps the rig's: the vendor
            // animates the rig it stored, in its own names. Only an animation:
            // a texture, remesh or segment of a rig, or a re-rig, can come back
            // unskinned or with a new skeleton.
            joint_naming: match extras.joint_naming.as_deref() {
                Some(n) => (n != "vendor").then(|| n.to_string()),
                None if schema.task == crate::capabilities::ModelTask::Animate => {
                    extras.source.as_ref().and_then(|s| s.joint_naming.clone())
                }
                None => None,
            },
            fbx_color_space: extras.fbx_color_space,
            fbx_light_units: extras.fbx_light_units,
            usd_light_units: extras.usd_light_units,
            dae_morphs: extras.dae_morphs,
            usdz_root: extras.usdz_root,
            usd_clips: extras.usd_clips,
            packages: extras.output_packages == Some(true),
            ktx2_textures: extras.ktx2_textures.filter(|k| *k != crate::types::Model3dKtx2Textures::Png),
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
        let mut plan = match gen.metadata.as_ref().and_then(|m| m.get("delivery")) {
            // A row from before plans were recorded: its formats are all.
            None => DeliveryPlan::default(),
            Some(d) => serde_json::from_value::<DeliveryPlan>(d.clone()).unwrap_or_else(|e| {
                tracing::error!(generation_id = %gen.id, error = %e, "the recorded 3D delivery plan does not parse");
                DeliveryPlan {
                    unreadable: Some(format!(
                        "the delivery options recorded at submit could not be read ({e}); this mesh was delivered without them"
                    )),
                    ..Default::default()
                }
            }),
        };
        if plan.requested_formats.is_empty() {
            plan.requested_formats = crate::types::generation::row_requested_formats(gen);
        }
        plan
    }
}

/// What a delivery produced.
#[derive(Debug, Default)]
pub struct Delivered {
    /// Meshes (each with its manifest), originals, textures and previews,
    /// then packages (`output_packages`), each with its own manifest.
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

/// Measure one delivered mesh, its sidecars read through `sidecars` (a
/// package's own files; none for one file). `None` when it cannot be read
/// back.
fn manifest(
    bytes: &[u8],
    format: &str,
    plan: &DeliveryPlan,
    oriented_by: &str,
    litegen_in_place: &[bool],
    sidecars: &dyn crate::mesh::Sidecars,
) -> Option<SceneManifest> {
    let scene = crate::mesh::read_scene_with(bytes, MeshFormat::parse(format)?, sidecars, read_options(plan)).ok()?.scene;
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
        notes: Vec::new(),
    })
}

/// What an observer without the claim reports for a finished job: still
/// processing, short of done, with nothing delivered and no error.
pub fn stand_aside(poll: &mut crate::providers::Model3dGenerationPollResult) {
    poll.status = crate::types::GenerationStatus::Processing;
    poll.progress = poll.progress.min(99);
    poll.files.clear();
    poll.error = None;
}

/// Whether `poll` is a finished job with files: the observation a delivery
/// claim is for.
pub fn needs_delivery(poll: &crate::providers::Model3dGenerationPollResult) -> bool {
    poll.status == crate::types::GenerationStatus::Completed && !poll.files.is_empty()
}

/// Deliver the files of the poll that saw a job complete — what both
/// observers run, so whichever sees completion first delivers the same thing.
/// The work is CPU-bound and runs on a blocking thread; a failed delivery
/// fails the poll.
pub async fn deliver_poll(poll: &mut crate::providers::Model3dGenerationPollResult, plan: DeliveryPlan, id: &str) {
    if poll.status != crate::types::GenerationStatus::Completed || poll.files.is_empty() || plan.keep_nothing {
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
        .and_then(|f| {
            crate::mesh::read_scene_with(&primary.bytes, f, &crate::mesh::package::NoSidecars, read_options(plan)).map_err(|e| e.to_string())
        });
    let (mut scene, read_warnings) = match read {
        Ok(r) => (r.scene, r.warnings),
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
    // A vendor glTF that requires an extension is rewritten, and so is one
    // that only uses a compression extension, beside an uncompressed fallback
    // (gltfpack `-cf`): litegen decodes it, and delivers no compressed glTF.
    // Every vendor mesh that is a glb or gltf is checked, not the primary
    // alone (Task 3 minor 2): a compressed gltf beside a plain glb would
    // otherwise go out as it came.
    let (mut vendor_used, mut vendor_required) = (Vec::new(), Vec::new());
    for m in vendor_meshes.iter().filter(|m| m.format.eq_ignore_ascii_case("glb") || m.format.eq_ignore_ascii_case("gltf")) {
        let (used, required) = gltf_extensions(&m.bytes, &m.format.to_ascii_lowercase());
        vendor_used.extend(used);
        vendor_required.extend(required);
    }
    let mut changed = !vendor_required.is_empty()
        || vendor_used.iter().any(|e| crate::mesh::glb::COMPRESSION_EXTENSIONS.contains(&e.as_str()));
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
    // What delivery could not do, named on every mesh it delivers (and logged):
    // first what reading the vendor's file itself dropped.
    let mut notes: Vec<String> = plan.unreadable.iter().cloned().collect();
    notes.extend(read_warnings.into_iter().map(|w| format!("reading the provider's {primary_format}: {w}")));
    if let Some(p) = &plan.placement {
        match normalize::place(&mut scene, p.ground, p.center, p.height_m) {
            Ok(placed) => {
                changed |= placed.moved;
                if placed.height_skipped {
                    notes.push(format!(
                        "height_m {} was not applied: the mesh has no height to scale",
                        p.height_m.unwrap_or_default()
                    ));
                }
            }
            Err(e) => return Delivered { fatal: Some(format!("the mesh could not be placed: {e}")), ..Default::default() },
        }
    }
    if plan.parts {
        changed |= normalize::name_parts(&mut scene);
    }
    changed |= normalize::unique_node_names(&mut scene);
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
            Some(renames) => {
                changed |= normalize::rename_nodes(&mut scene, &renames);
                // A joint renamed onto another node's name ("head" to
                // "Head" beside a mesh node "Head"): unique again, or a
                // reader binding by name (three.js) picks the wrong one.
                changed |= normalize::unique_node_names(&mut scene);
            }
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
    // Which clips were rekeyed at the requested rate: an in-place hold keeps
    // them on it.
    let mut rekeyed = vec![false; scene.animations.len()];
    if let Some(fps) = plan.fps {
        // Counted once and moved by each rekey, so the budget costs a clip
        // its own channels, not a recount of the scene.
        let mut size = normalize::animation_size(&scene);
        for (i, done) in rekeyed.iter_mut().enumerate() {
            match normalize::resample_with_size(&mut scene, &mut size, i, fps) {
                Ok(keyed) => {
                    changed |= keyed;
                    *done = true;
                }
                Err(limit) => {
                    let why = match limit {
                        normalize::RekeyLimit::Grid => "it is too long to key at that rate".to_string(),
                        normalize::RekeyLimit::Keys(keys) => format!("it would hold {keys} keys, over the {MAX_ANIMATION_KEYS}-key limit"),
                        normalize::RekeyLimit::Values(values) => format!("it would hold {values} values, over the {MAX_RESAMPLED_VALUES}-value limit"),
                    };
                    notes.push(format!("clip {} was not rekeyed at {fps} fps: {why}", scene.animations[i].name.as_deref().unwrap_or("?")));
                }
            }
        }
    }
    let mut litegen_in_place = vec![false; scene.animations.len()];
    if let Ok(facts) = crate::mesh::inspect::inspect(&scene) {
        for (i, clip) in plan.clips.iter().enumerate() {
            let measured = facts.animations.get(i).map(|c| c.in_place()).unwrap_or(true);
            if clip.in_place != Some(true) || measured {
                continue;
            }
            if normalize::remove_root_motion(&mut scene, i, rekeyed.get(i).copied().unwrap_or(false)) {
                litegen_in_place[i] = true;
                changed = true;
            }
            // Measured again, so the asset never claims a hold that did not take.
            let held = crate::mesh::inspect::inspect(&scene).ok().and_then(|f| f.animations.get(i).map(|c| c.in_place())).unwrap_or(false);
            if !held {
                litegen_in_place[i] = false;
                notes.push(format!("clip {} was asked to stay in place, and litegen could not hold it", clip.name));
            }
        }
    }
    // `ktx2_textures: keep`: the glb and gltf are written from a scene whose
    // KTX2 images are still KTX2. WebP becomes PNG first, once, and the scene
    // is copied there; the copy is the one that keeps its KTX2, and the
    // original then has them turned into PNG for every other container. A
    // scene is copied only when it holds a KTX2 image and a glb or gltf is
    // asked for.
    let asks_glb_or_gltf = requested.iter().filter_map(|f| MeshFormat::parse(f)).any(|t| matches!(t, MeshFormat::Glb | MeshFormat::Gltf));
    let texture_notes_at = notes.len();
    let mut keeping: Option<(crate::mesh::scene::Scene, Vec<String>)> = None;
    if plan.ktx2_textures == Some(crate::types::Model3dKtx2Textures::Keep)
        && asks_glb_or_gltf
        && scene.images.iter().any(|i| i.format == crate::mesh::scene::ImageFormat::Ktx2)
    {
        let webp = normalize::transcode_textures_keeping_ktx2(&mut scene);
        changed |= webp.changed;
        let webp_notes = webp.notes();
        notes.extend(webp_notes.iter().cloned());
        keeping = Some((scene.clone(), webp_notes));
    }
    let transcode = normalize::transcode_textures(&mut scene);
    changed |= transcode.changed;
    notes.extend(transcode.notes());
    let texture_notes_end = notes.len();
    notes.extend(naming_note);
    // The notes of a glb or gltf written from the keeping scene: the same,
    // with that scene's texture notes (a KTX2 image is among them only if it
    // was a WebP).
    let kept_notes: Option<Vec<String>> =
        keeping.as_ref().map(|(_, texture_notes)| [&notes[..texture_notes_at], texture_notes.as_slice(), &notes[texture_notes_end..]].concat());
    // What is logged as not delivered: a KTX2 kept in every container asked
    // for was delivered, so it is no failure.
    let only_keeping = requested.iter().filter_map(|f| MeshFormat::parse(f)).all(|t| matches!(t, MeshFormat::Glb | MeshFormat::Gltf));
    let logged: &[String] = match &kept_notes {
        Some(kept) if only_keeping => kept,
        _ => &notes,
    };
    let mut failures: Vec<(String, String)> = logged.iter().map(|n| ("delivery".to_string(), n.clone())).collect();

    // ── Deliver ──
    // The rate the containers that resample (FBX, Collada, USD) key and
    // declare: the requested one, when every clip was rekeyed at it.
    let key_fps = plan.fps.filter(|_| rekeyed.iter().all(|&done| done));
    // A container written from the scene, with what it could not hold.
    let mut losses: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    // The scene a container is written from: the glb and gltf take the one
    // that kept its KTX2 images, when `ktx2_textures` is `keep`.
    let scene_for = |target: MeshFormat| -> &crate::mesh::scene::Scene {
        match &keeping {
            Some((kept, _)) if matches!(target, MeshFormat::Glb | MeshFormat::Gltf) => kept,
            _ => &scene,
        }
    };
    let mut write = |fmt: &str, failures: &mut Vec<(String, String)>| -> Option<Model3dFile> {
        let Some(target) = MeshFormat::parse(fmt) else {
            failures.push((fmt.to_string(), "not a container litegen writes".into()));
            return None;
        };
        match write_keyed(scene_for(target), target, true, key_fps, plan) {
            Ok(w) => {
                losses.insert(target.as_str().to_string(), loss_notes(&w.losses));
                match w.package.files.into_iter().next() {
                    Some(file) => Some(Model3dFile {
                        kind: Model3dAssetKind::Mesh,
                        format: target.as_str().to_string(),
                        content_type: crate::store::model3d_content_type(target.as_str(), "application/octet-stream").to_string(),
                        bytes: file.bytes,
                        polycount: Some(scene.triangle_count() as u32),
                        width: None,
                        height: None,
                        manifest: None,
                        source_url: None,
                    }),
                    None => {
                        failures.push((fmt.to_string(), "the writer produced no file".into()));
                        None
                    }
                }
            }
            Err(e) => {
                failures.push((fmt.to_string(), e.to_string()));
                None
            }
        }
    };
    let mut out: Vec<Model3dFile> = Vec::new();
    if !changed {
        // The vendor's files as they came; a promised container it did not
        // send is written from the scene read from them.
        let have: Vec<String> = vendor_meshes.iter().map(|m| m.format.to_ascii_lowercase()).collect();
        for fmt in crate::types::missing_model3d_formats(&requested, &have) {
            out.extend(write(&fmt, &mut failures));
        }
        out.splice(0..0, vendor_meshes);
    } else {
        for fmt in &requested {
            out.extend(write(fmt, &mut failures));
        }
        for mut m in vendor_meshes {
            m.kind = Model3dAssetKind::Original;
            out.push(m);
        }
    }
    let requested_clips: Vec<&str> = plan.clips.iter().map(|c| c.name.as_str()).collect();
    // What a delivered file's manifest says beside what was measured: what
    // delivery could not do, what this write could not hold (`losses`), and
    // the clips as this container holds them against the request (one keeps
    // only the first (USD), one renames to its rules (Collada ids)).
    // Measured, so a file never contradicts its notes.
    let annotate = |mut m: SceneManifest, format: &str, losses: &[String]| -> SceneManifest {
        let notes = match (&kept_notes, format) {
            (Some(kept), "glb" | "gltf") => kept,
            _ => &notes,
        };
        m.notes = notes.iter().cloned().chain(losses.iter().cloned()).collect();
        let held: Vec<&str> = m.animations.iter().map(|a| a.name.as_str()).collect();
        if !requested_clips.is_empty() && held != requested_clips {
            m.notes.push(format!(
                "this {format} holds the clips [{}] for the [{}] requested",
                held.join(", "),
                requested_clips.join(", ")
            ));
        }
        m
    };
    for f in out.iter_mut().filter(|f| f.kind == Model3dAssetKind::Mesh) {
        let format = f.format.to_ascii_lowercase();
        let lost = losses.get(&format).cloned().unwrap_or_default();
        f.manifest = manifest(&f.bytes, &format, plan, oriented_by, &litegen_in_place, &crate::mesh::package::NoSidecars)
            .map(|m| annotate(m, &format, &lost));
    }
    out.extend(others);
    // `output_packages`: every requested container written as a package too,
    // delivered as its zip when it is more than one file. Last, so no file
    // above moves from the place it has without packages.
    if plan.packages {
        // A container that is one file either way (glb, fbx, …) is not
        // written again.
        for target in requested.iter().filter_map(|f| MeshFormat::parse(f)).filter(|t| t.writes_sidecars()) {
            match package_zip(scene_for(target), target, key_fps, plan) {
                Ok(None) => {}
                Ok(Some(PackageZip { zip, files, losses })) => {
                    let sidecars = crate::mesh::package::MemorySidecars { files: &files };
                    let measured = manifest(&files[0].bytes, target.as_str(), plan, oriented_by, &litegen_in_place, &sidecars)
                        .map(|m| annotate(m, target.as_str(), &losses));
                    out.push(Model3dFile {
                        kind: Model3dAssetKind::Package,
                        format: target.as_str().to_string(),
                        content_type: crate::store::model3d_content_type("zip", "application/zip").to_string(),
                        bytes: zip,
                        polycount: Some(scene.triangle_count() as u32),
                        width: None,
                        height: None,
                        manifest: measured,
                        source_url: None,
                    });
                }
                Err(e) => {
                    // Named on the container's mesh, so a package asked for is
                    // never missing without a word.
                    let note = format!("the {} package could not be made: {e}", target.as_str());
                    if let Some(m) = out.iter_mut().find(|f| f.kind == Model3dAssetKind::Mesh && f.format.eq_ignore_ascii_case(target.as_str())).and_then(|f| f.manifest.as_mut()) {
                        m.notes.push(note.clone());
                    }
                    failures.push((format!("{} package", target.as_str()), note));
                }
            }
        }
    }
    Delivered { files: out, fatal: None, failures }
}

/// A delivery's losses as the notes a manifest lists.
fn loss_notes(losses: &[crate::mesh::Loss]) -> Vec<String> {
    losses.iter().map(|l| format!("{}: {} ({})", l.kind, l.what, l.why)).collect()
}

/// `scene` written as `target`, as one file (`self_contained`) or a package,
/// keyed at `key_fps`; past a writer's budget at that rate, at its own 30 fps,
/// and said so.
fn write_keyed(
    scene: &crate::mesh::scene::Scene,
    target: MeshFormat,
    self_contained: bool,
    key_fps: Option<u32>,
    plan: &DeliveryPlan,
) -> Result<crate::mesh::SceneWrite, crate::mesh::MeshError> {
    let fbx_color_space = crate::proxy::model3d_convert::fbx_color_space(plan.fbx_color_space);
    let fbx_light_units = crate::proxy::model3d_convert::light_units(plan.fbx_light_units);
    let usd_light_units = crate::proxy::model3d_convert::light_units(plan.usd_light_units);
    let dae_morphs = crate::proxy::model3d_convert::dae_morphs(plan.dae_morphs);
    let usdz_root = crate::proxy::model3d_convert::usdz_root(plan.usdz_root);
    let usd_clips = crate::proxy::model3d_convert::usd_clips(plan.usd_clips);
    let opts = |fps: Option<u32>| WriteOptions { self_contained, fps, fbx_color_space, fbx_light_units, usd_light_units, dae_morphs, usdz_root, usd_clips };
    match crate::mesh::write_scene(scene, target, opts(key_fps)) {
        Err(crate::mesh::MeshError::Unsupported(why)) if key_fps.is_some() => crate::mesh::write_scene(scene, target, opts(None)).map(|mut w| {
            w.losses.push(crate::mesh::Loss::new(
                crate::mesh::LossKind::Animation,
                format!("keying at {} fps", key_fps.unwrap_or_default()),
                format!("{target} keys at 30 fps instead: {why}"),
            ));
            w
        }),
        other => other,
    }
}

/// A container's package, zipped: the zip, the files in it, and what the
/// package could not hold.
struct PackageZip {
    zip: Vec<u8>,
    files: Vec<crate::mesh::PackageFile>,
    losses: Vec<String>,
}

/// `scene` as `target`'s package, zipped; `None` when that package is one
/// file (an obj, dae, usda or usdc with nothing to put beside it).
fn package_zip(scene: &crate::mesh::scene::Scene, target: MeshFormat, key_fps: Option<u32>, plan: &DeliveryPlan) -> Result<Option<PackageZip>, String> {
    let w = write_keyed(scene, target, false, key_fps, plan).map_err(|e| e.to_string())?;
    if w.package.files.len() < 2 {
        return Ok(None);
    }
    let zip = crate::mesh::zip::write_package(&w.package).map_err(|e| e.to_string())?;
    Ok(Some(PackageZip { zip, losses: loss_notes(&w.losses), files: w.package.files }))
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
            source_url: None,
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

    /// The scene read from USDA text, brought to the precision a crate holds
    /// its transforms at, to be compared with `==` against the same layer's
    /// crate reading, `crate_read`. A copy of `transforms_at_f32` in
    /// `mesh/usd.rs`'s tests, which this module cannot see; see it for why
    /// (rulings U16, U18 and U18b): translation and scale are rounded through
    /// f32 (exact), and rotation is held to within 2^-24 per component of the
    /// crate's (half an f32 ulp at 1, the declared precision of `quatf`),
    /// asserted, then taken from the crate, because the reader normalises the
    /// quaternion after reading, from the decimal on one side and the f32 on
    /// the other, so no input rounding can make them equal.
    fn transforms_at_f32(mut text: crate::mesh::scene::Scene, crate_read: &crate::mesh::scene::Scene) -> crate::mesh::scene::Scene {
        use crate::mesh::scene::Transform;
        const HALF_F32_ULP_AT_ONE: f64 = 1.0 / 16_777_216.0; // 2^-24
        let f32d = |x: f64| f64::from(x as f32);
        for (i, (node, other)) in text.nodes.iter_mut().zip(&crate_read.nodes).enumerate() {
            if let (Transform::Trs { translation, rotation, scale }, Transform::Trs { rotation: crate_rotation, .. }) = (&mut node.transform, &other.transform) {
                *translation = translation.map(f32d);
                *scale = scale.map(f32d);
                for (c, axis) in ["x", "y", "z", "w"].into_iter().enumerate() {
                    let apart = (rotation[c] - crate_rotation[c]).abs();
                    assert!(
                        apart <= HALF_F32_ULP_AT_ONE,
                        "node {i} {:?}: rotation {axis} is {} in the text and {} in the crate, {apart:e} apart, more than 2^-24",
                        node.name,
                        rotation[c],
                        crate_rotation[c]
                    );
                }
                *rotation = *crate_rotation;
            }
        }
        text
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

    /// Every container is derived from a GLB-only vendor, and each reads
    /// back: its manifest is measured from litegen's own read of it.
    #[test]
    fn every_container_derives_from_a_glb_and_reads_back() {
        let cube = crate::providers::model3d::glb::generate_cube_glb("a crate");
        let all: Vec<&str> = crate::mesh::MeshFormat::ALL.iter().map(|f| f.as_str()).collect();
        let d = deliver(vec![mesh("glb", cube.clone())], &plan(&all, Some(("+y", "+z"))));
        assert!(d.fatal.is_none() && d.failures.is_empty(), "{:?} {:?}", d.fatal, d.failures);
        let got: Vec<&str> = meshes(&d).iter().map(|m| m.format.as_str()).collect();
        let mut want = all.clone();
        want.retain(|f| *f != "glb");
        want.insert(0, "glb");
        assert_eq!(got, want, "the vendor's glb first, then each derived container");
        for m in meshes(&d) {
            let s = m.manifest.as_ref().unwrap_or_else(|| panic!("{} did not read back", m.format));
            assert_eq!(s.triangle_count, 12, "{}", m.format);
        }
        assert_eq!(meshes(&d)[0].bytes, cube, "the vendor's own file, untouched");
    }

    /// A vendor's spelling of a container is matched without case.
    #[test]
    fn a_vendor_format_spelled_in_capitals_is_its_container() {
        let cube = crate::providers::model3d::glb::generate_cube_glb("a crate");
        let d = deliver(vec![mesh("GLB", cube.clone())], &plan(&["glb"], Some(("+y", "+z"))));
        assert!(d.fatal.is_none(), "{:?}", d.fatal);
        assert_eq!(meshes(&d).len(), 1, "nothing derived for a container the vendor sent");
        assert_eq!(meshes(&d)[0].bytes, cube);
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
        let unreadable = crate::providers::model3d::mock::generate_undecodable_glb("a fox");
        let only_glb = deliver(vec![mesh("glb", unreadable.clone())], &plan(&["glb"], Some(("+y", "+z"))));
        assert!(only_glb.fatal.as_deref().is_some_and(|e| e.contains("could not be read")), "{:?}", only_glb.fatal);
        let with_fbx = deliver(vec![mesh("glb", unreadable)], &plan(&["glb", "fbx"], Some(("+y", "+z"))));
        assert!(with_fbx.fatal.as_deref().is_some_and(|e| e.contains("fbx")), "{:?}", with_fbx.fatal);
    }

    /// Ruling B23 (mesh-gaps item 2): a vendor GLB that only *uses* meshopt,
    /// with an uncompressed fallback and nothing required, as gltfpack
    /// `-cf -noq` writes it, and the same file under KHR's name. Its fallback
    /// is an external .bin nobody sent, and none is needed: litegen decodes
    /// the streams. Every glTF litegen delivers is uncompressed, so the file
    /// is rewritten, never handed on, and kept as the original, as any
    /// changed file is. The same for a used-only Draco GLB (Task 4 minors 5
    /// and 8, F-T4m8), which pins Draco's `COMPRESSION_EXTENSIONS` entry at
    /// delivery.
    #[test]
    fn a_vendor_glb_that_only_uses_meshopt_is_delivered_uncompressed() {
        const MESHOPT: [&str; 2] = ["EXT_meshopt_compression", "KHR_meshopt_compression"];
        const COMPRESSED: &[&str] = crate::mesh::glb::COMPRESSION_EXTENSIONS;
        let ext = include_bytes!("../providers/model3d/mock_data/skinned.cf.noq.glb").to_vec();
        // KHR's v0 streams are EXT's, so the renamed file is one too.
        let khr = with_json(&ext, |doc| {
            *doc = serde_json::from_str(&doc.to_string().replace(MESHOPT[0], MESHOPT[1])).unwrap();
        });
        // mock_data's skinned.draco.glb, the same skinned fixture, with its
        // `extensionsRequired` dropped so that only `extensionsUsed` names
        // Draco: read from the stream either way (Ruling B27).
        let draco = with_json(include_bytes!("../providers/model3d/mock_data/skinned.draco.glb"), |doc| {
            doc.as_object_mut().unwrap().remove("extensionsRequired");
        });
        for (name, vendor) in [(MESHOPT[0], ext), (MESHOPT[1], khr), ("KHR_draco_mesh_compression", draco)] {
            let (used, required) = gltf_extensions(&vendor, "glb");
            assert_eq!((used, required), (vec![name.to_string()], Vec::<String>::new()), "the subject");
            // Buffer 1 names `skinned.cf.noq.fallback.bin`, which nobody sent.
            let read = crate::mesh::read_scene(&vendor, MeshFormat::Glb, &crate::mesh::package::NoSidecars)
                .unwrap_or_else(|e| panic!("{name}: it reads without its fallback: {e}"));
            assert_eq!(read.scene.triangle_count(), 768, "{name}: its one primitive's 2,304 indices");

            let d = deliver(vec![mesh("glb", vendor.clone())], &plan(&["glb"], None));
            assert!(d.fatal.is_none(), "{name}: {:?}", d.fatal);
            let m = meshes(&d);
            assert_eq!(m.iter().map(|f| f.format.as_str()).collect::<Vec<_>>(), ["glb"], "{name}");
            let (used, required) = gltf_extensions(&m[0].bytes, "glb");
            assert!(
                !used.iter().chain(&required).any(|e| COMPRESSED.contains(&e.as_str())),
                "{name}: the delivered glb names a compression extension: used {used:?}, required {required:?}"
            );
            assert!(required.is_empty(), "{name}: {required:?}");
            let json_len = u32::from_le_bytes(m[0].bytes[12..16].try_into().unwrap()) as usize;
            let doc: serde_json::Value = serde_json::from_slice(&m[0].bytes[20..20 + json_len]).unwrap();
            assert!(doc["buffers"].as_array().unwrap().iter().all(|b| b.get("uri").is_none()), "{name}: names no file it lacks: {}", doc["buffers"]);
            let s = m[0].manifest.as_ref().expect("measured");
            assert!(!s.extensions_used.iter().any(|e| COMPRESSED.contains(&e.as_str())), "{name}: {:?}", s.extensions_used);
            assert_eq!(s.triangle_count, 768, "{name}");
            assert_eq!(s.skin.as_ref().map(|k| k.joints.clone()), Some(vec!["hip".to_string(), "knee".into(), "ankle".into()]), "{name}");
            assert_eq!(s.animations.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Bend"], "{name}");
            let originals: Vec<&Model3dFile> = d.files.iter().filter(|f| f.kind == Model3dAssetKind::Original).collect();
            assert_eq!(originals.len(), 1, "{name}: the vendor's file kept beside it");
            assert_eq!(originals[0].bytes, vendor, "{name}");
        }
    }

    /// Task 3 minor 2 (F-T3m2): the compression and required-extension check
    /// covers every vendor mesh that is glb or gltf, not the primary alone.
    /// A vendor that returns an uncompressed glb beside a meshopt gltf, for
    /// a request of both, has both rewritten: the delivered gltf names no
    /// compression extension and requires nothing, and both vendor files
    /// are kept as originals. Before this the primary alone was checked, and
    /// the vendor's compressed gltf went out as it came, against the module
    /// doc's "every glTF litegen delivers is uncompressed".
    #[test]
    fn a_compressed_secondary_vendor_gltf_is_rewritten_too() {
        let glb = crate::providers::model3d::glb::generate_cube_glb("a crate");
        // skinned.cf.noq.glb's document as a .gltf: a used-only meshopt file
        // whose buffers are a BIN chunk it no longer has and a fallback
        // nobody sent. The check reads its extension lists alone.
        let packed = include_bytes!("../providers/model3d/mock_data/skinned.cf.noq.glb");
        let json_len = u32::from_le_bytes(packed[12..16].try_into().unwrap()) as usize;
        let gltf = packed[20..20 + json_len].to_vec();
        let (used, required) = gltf_extensions(&gltf, "gltf");
        assert_eq!((used, required), (vec!["EXT_meshopt_compression".to_string()], Vec::<String>::new()), "the subject: a used-only meshopt gltf");
        let d = deliver(vec![mesh("glb", glb.clone()), mesh("gltf", gltf.clone())], &plan(&["glb", "gltf"], None));
        assert!(d.fatal.is_none(), "{:?}", d.fatal);
        let m = meshes(&d);
        assert_eq!(m.iter().map(|f| f.format.as_str()).collect::<Vec<_>>(), ["glb", "gltf"]);
        let delivered = m.iter().find(|f| f.format == "gltf").unwrap();
        let (used, required) = gltf_extensions(&delivered.bytes, "gltf");
        assert!(
            !used.iter().any(|e| crate::mesh::glb::COMPRESSION_EXTENSIONS.contains(&e.as_str())),
            "the delivered gltf is uncompressed: used {used:?}"
        );
        assert!(required.is_empty(), "{required:?}");
        let originals: Vec<&Model3dFile> = d.files.iter().filter(|f| f.kind == Model3dAssetKind::Original).collect();
        assert_eq!(originals.iter().map(|f| f.format.as_str()).collect::<Vec<_>>(), ["glb", "gltf"], "both vendor files kept as originals");
        assert!(originals[0].bytes == glb && originals[1].bytes == gltf, "as they came");
    }

    /// Ruling B24's precision (mesh-gaps item 2, Task 3's review, Minor 1):
    /// only a compression extension makes a vendor GLB that requires nothing
    /// count as changed. The cube using KHR_texture_transform, which litegen
    /// carries and reads without a warning, is handed on untouched, with no
    /// original beside it.
    #[test]
    fn a_vendor_glb_that_only_uses_a_carried_extension_is_handed_on_untouched() {
        let cube = crate::providers::model3d::glb::generate_cube_glb("a crate");
        let vendor = with_json(&cube, |doc| doc["extensionsUsed"] = serde_json::json!(["KHR_texture_transform"]));
        let (used, required) = gltf_extensions(&vendor, "glb");
        assert_eq!((used, required), (vec!["KHR_texture_transform".to_string()], Vec::<String>::new()), "the subject");
        let d = deliver(vec![mesh("glb", vendor.clone())], &plan(&["glb"], None));
        assert!(d.fatal.is_none(), "{:?}", d.fatal);
        let m = meshes(&d);
        assert_eq!(m.len(), 1, "one glb");
        assert!(m[0].bytes == vendor, "nothing to change, nothing changed: the vendor's own bytes");
        assert!(!d.files.iter().any(|f| f.kind == Model3dAssetKind::Original), "no original: the vendor's file is the one delivered");
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
            extra: None, source: None, preflight: None, fbx_color_space: None, fbx_light_units: None, usd_light_units: None, dae_morphs: None, usdz_root: None, usd_clips: None, format_source: None,output_packages: None, ktx2_textures: None,
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

    /// 3D doc §3B: a request's own joint naming wins, `vendor` meaning none.
    /// One that names none keeps its source rig's only when it animates
    /// the rig: a texture, remesh or segment of a rig, or a re-rig, can
    /// come back unskinned or with a new skeleton.
    #[test]
    fn the_requested_naming_wins_and_only_an_animation_inherits_its_rigs() {
        let mut models = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        models.pop();
        models.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&models).unwrap();
        let naming = |model: &str, asked: Option<&str>, rig: Option<&str>| {
            let extras = crate::providers::Model3dExtras {
                output_formats: vec!["glb".into()],
                texture: None, pbr: None, target_polycount: None, symmetry: None, topology: None, rig: None,
                skeleton: None, joint_naming: asked.map(str::to_string), height_m: None,
                extra: None, preflight: None, fbx_color_space: None, fbx_light_units: None, usd_light_units: None, dae_morphs: None, usdz_root: None, usd_clips: None, format_source: None,output_packages: None, ktx2_textures: None,
                source: Some(crate::providers::SourceInput {
                    bytes: Vec::new(),
                    format: "glb".into(),
                    sha256: "00".into(),
                    facts: Default::default(),
                    vendor_task_id: None,
                    front: None,
                    joint_naming: rig.map(str::to_string),
                    delivered: Default::default(),
                }),
                animations: Vec::new(), fps: None,
                texture_resolution: None, keep_uvs: None,
                ground: None, center: None,
            };
            DeliveryPlan::for_request(catalog.get(model).unwrap(), &extras).joint_naming
        };
        assert_eq!(naming("mock/animate-3d", None, Some("unity")).as_deref(), Some("unity"), "an animation is delivered in its rig's naming");
        assert_eq!(naming("mock/animate-3d", Some("vendor"), Some("unity")), None, "asked for the vendor's names, it gets them");
        assert_eq!(naming("mock/animate-3d", Some("mixamo"), Some("unity")).as_deref(), Some("mixamo"), "asked for another naming, it gets that");
        assert_eq!(naming("mock/animate-3d", None, None), None, "a rig in the vendor's names passes none on");
        assert_eq!(naming("mock/rig-3d", Some("unity"), None).as_deref(), Some("unity"), "a rig is delivered as asked");
        assert_eq!(naming("mock/rig-3d", None, Some("unity")), None, "a re-rig makes a skeleton of its own");
        assert_eq!(naming("mock/texture-3d", None, Some("unity")), None, "only an animation inherits");
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
        let m = manifest(&file, "glb", &clip_plan(&["walk"]), "vendor", &[], &crate::mesh::package::NoSidecars).unwrap();
        assert_eq!(m.animations[0].name, "Armature|walk");
    }

    /// 3D doc §3C: clips are keyed at the requested `fps` — litegen's own
    /// resampling when the vendor keyed them at another rate, the only way
    /// fps is honoured for a vendor without the setting.
    #[test]
    fn clips_a_vendor_keyed_at_another_rate_are_delivered_at_the_requested_fps() {
        let plan = DeliveryPlan { fps: Some(4), ..clip_plan(&["walk"]) };
        let d = deliver(vec![mesh("glb", animated(&[("walk", &[0.0, 0.5, 1.0])]))], &plan);
        assert!(d.fatal.is_none(), "{:?}", d.fatal);
        let glb = d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh && f.format == "glb").expect("a delivered glb");
        let scene = crate::mesh::read_scene(&glb.bytes, MeshFormat::Glb, &crate::mesh::package::NoSidecars).unwrap().scene;
        assert_eq!(scene.animations[0].channels[0].times, vec![0.0, 0.25, 0.5, 0.75, 1.0]);
    }

    /// Review leftover f (2026-11-04): a rekey past the scene's key or value
    /// budget is declined before a key is built, and the note names the
    /// limit that failed.
    #[test]
    fn the_notes_name_the_budget_that_failed() {
        use crate::mesh::scene::{AnimPath, Animation, Channel, Interpolation, MorphTarget, Node, Primitive, Scene, SceneMesh};
        let notes = |glb: Vec<u8>, fps: u32| {
            let d = deliver(vec![mesh("glb", glb)], &DeliveryPlan { fps: Some(fps), ..clip_plan(&["walk"]) });
            assert!(d.fatal.is_none(), "{:?}", d.fatal);
            let glb = d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh && f.format == "glb").expect("the glb is delivered");
            glb.manifest.as_ref().unwrap().notes.clone()
        };
        // Each clip is past a budget before delivery sees it: a regression
        // fails here, not after building gigabytes of keys.
        let over_budget = |s: &Scene| {
            assert!(!normalize::fits_fps(s, 0, 30), "{:?}", normalize::rekeyed_size(s, 0, 30));
            crate::mesh::glb::write_scene(s).unwrap()
        };

        // 3,000 nodes keyed twice from 0 to 3,000 s: at 30 fps, over both
        // limits, and keys are checked first.
        let mut many = crate::mesh::fixtures::cube();
        let first = many.nodes.len();
        many.nodes.extend((0..3_000).map(|i| Node { name: Some(format!("dot{i}")), ..Default::default() }));
        many.roots.extend(first..first + 3_000);
        many.animations.push(Animation {
            name: Some("walk".into()),
            channels: (first..first + 3_000)
                .map(|node| Channel { node, path: AnimPath::Translation, interpolation: Interpolation::Linear, times: vec![0.0, 3_000.0], values: vec![0.0; 6] })
                .collect(),
        });
        let n = notes(over_budget(&many), 30);
        assert!(n.iter().any(|n| n == "clip walk was not rekeyed at 30 fps: it would hold 270003000 keys, over the 20000000-key limit"), "{n:?}");

        // One triangle with 1,024 morph targets keyed from 0 to 2,000 s: past
        // the value limit alone.
        let mut morphs = Scene::default();
        morphs.meshes.push(SceneMesh {
            primitives: vec![Primitive {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                indices: vec![0, 1, 2],
                targets: vec![MorphTarget { positions: vec![[0.0, 0.0, 0.1]; 3], normals: None }; 1_024],
                ..Default::default()
            }],
            ..Default::default()
        });
        morphs.nodes.push(Node { name: Some("face".into()), mesh: Some(0), ..Default::default() });
        morphs.roots = vec![0];
        morphs.animations.push(Animation {
            name: Some("walk".into()),
            channels: vec![Channel { node: 0, path: AnimPath::Weights, interpolation: Interpolation::Linear, times: vec![0.0, 2_000.0], values: vec![0.0; 2 * 1_024] }],
        });
        let n = notes(over_budget(&morphs), 30);
        assert!(n.iter().any(|n| n == "clip walk was not rekeyed at 30 fps: it would hold 61441024 values, over the 32000000-value limit"), "{n:?}");

        // Past the grid cap, the wording is as before.
        let n = notes(animated(&[("walk", &[0.0, 1.0])]), u32::MAX);
        assert!(n.iter().any(|n| n == "clip walk was not rekeyed at 4294967295 fps: it is too long to key at that rate"), "{n:?}");
    }

    /// A Blender-style FBX: one triangle, a Phong material with DiffuseColor
    /// 0.8 and no Maya PBR colours.
    fn classic_fbx() -> Vec<u8> {
        "; FBX 7.4.0 project file\nFBXHeaderExtension:  {\n\tFBXVersion: 7400\n\tCreator: \"Blender (stable FBX IO) - 4.4.0\"\n}\nObjects:  {\n\
         \tModel: 10, \"Model::m\", \"Mesh\" {\n\t}\n\
         \tGeometry: 99, \"Geometry::tri\", \"Mesh\" {\n\t\tVertices: *9 {\n\t\t\ta: 0,0,0,1,0,0,0,1,0\n\t\t}\n\t\tPolygonVertexIndex: *3 {\n\t\t\ta: 0,1,-3\n\t\t}\n\
         \t\tLayerElementMaterial: 0 {\n\t\t\tMappingInformationType: \"AllSame\"\n\t\t\tReferenceInformationType: \"IndexToDirect\"\n\t\t\tMaterials: *1 {\n\t\t\t\ta: 0\n\t\t\t}\n\t\t}\n\t}\n\
         \tMaterial: 20, \"Material::m\", \"\" {\n\t\tShadingModel: \"phong\"\n\t\tProperties70:  {\n\t\t\tP: \"DiffuseColor\", \"Color\", \"\", \"A\",0.8,0.8,0.8\n\t\t}\n\t}\n\
         }\nConnections:  {\n\tC: \"OO\",10,0\n\tC: \"OO\",99,10\n\tC: \"OO\",20,10\n}\n"
            .as_bytes()
            .to_vec()
    }

    /// The client's `fbx_color_space` (review finding 21, left to the client):
    /// a vendor FBX whose colours are linear (Blender's) reads as such when
    /// the request says so, and every FBX delivered is written in it; the
    /// default stays sRGB.
    #[test]
    fn the_requested_fbx_colour_space_reads_the_vendor_fbx_and_writes_every_fbx() {
        use crate::types::Model3dFbxColorSpace;
        let glb_colour = |space: Option<Model3dFbxColorSpace>| {
            let p = DeliveryPlan { fbx_color_space: space, ..plan(&["glb"], Some(("+y", "+z"))) };
            let d = deliver(vec![mesh("fbx", classic_fbx())], &p);
            assert!(d.fatal.is_none(), "{:?}", d.fatal);
            let glb = d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh && f.format == "glb").expect("a derived glb");
            crate::mesh::read_scene(&glb.bytes, MeshFormat::Glb, &crate::mesh::package::NoSidecars).unwrap().scene.materials[0].base_color[0]
        };
        assert!((glb_colour(Some(Model3dFbxColorSpace::Linear)) - 0.8).abs() < 1e-4, "{}", glb_colour(Some(Model3dFbxColorSpace::Linear)));
        assert!((glb_colour(None) - crate::mesh::math::srgb_to_linear(0.8)).abs() < 1e-4, "the default is unchanged");
        // Written: the FBX made from a vendor glb differs only by its colours.
        let fbx_of = |space: Option<Model3dFbxColorSpace>| {
            let p = DeliveryPlan { fbx_color_space: space, ..plan(&["glb", "fbx"], Some(("+y", "+z"))) };
            let d = deliver(vec![mesh("glb", crate::mesh::glb::write_scene(&crate::mesh::fixtures::cube()).unwrap())], &p);
            d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh && f.format == "fbx").expect("a derived fbx").bytes.clone()
        };
        assert_ne!(fbx_of(Some(Model3dFbxColorSpace::Linear)), fbx_of(None));
        assert_eq!(fbx_of(Some(Model3dFbxColorSpace::Srgb)), fbx_of(None));
    }

    /// The client's `fbx_light_units` and `usd_light_units`: each is applied to
    /// its own format alone, in what delivery writes (every FBX and USD made
    /// from a vendor GLB) and in what it reads (a vendor FBX or USD whose
    /// lights are Blender's), and `photometric` is the default's bytes.
    #[test]
    fn the_requested_light_units_read_the_vendor_file_and_write_every_fbx_and_usd() {
        use crate::types::Model3dLightUnits::{Blender, Photometric};
        let staged = crate::mesh::fixtures::staged();
        // Written: the files made from a vendor glb with lights.
        let glb = crate::mesh::glb::write_scene(&staged).unwrap();
        let made = |fbx_light_units, usd_light_units, format: &str| {
            let p = DeliveryPlan { fbx_light_units, usd_light_units, ..plan(&["glb", "fbx", "usdz"], Some(("+y", "+z"))) };
            let d = deliver(vec![mesh("glb", glb.clone())], &p);
            assert!(d.fatal.is_none(), "{:?}", d.fatal);
            d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh && f.format == format).unwrap_or_else(|| panic!("a derived {format}")).bytes.clone()
        };
        for format in ["fbx", "usdz"] {
            let default = made(None, None, format);
            assert!(made(Some(Photometric), Some(Photometric), format) == default, "{format}: photometric is today's bytes");
            let (own, other) = if format == "fbx" { (made(Some(Blender), None, format), made(None, Some(Blender), format)) } else { (made(None, Some(Blender), format), made(Some(Blender), None, format)) };
            assert!(own != default, "{format}: its own option changes its lights");
            assert!(other == default, "{format}: the other format's option leaves it alone");
        }
        // Read: a vendor FBX written in Blender's units gives the scene's own
        // strengths when the request says so, and its energy otherwise.
        let blender_fbx = crate::mesh::write_scene(&staged, MeshFormat::Fbx, crate::mesh::WriteOptions { fbx_light_units: crate::mesh::LightUnits::Blender, ..Default::default() })
            .unwrap()
            .package
            .files
            .remove(0)
            .bytes;
        let sun_in_glb = |vendor: (&str, Vec<u8>), fbx_light_units, usd_light_units| {
            let p = DeliveryPlan { fbx_light_units, usd_light_units, ..plan(&["glb"], Some(("+y", "+z"))) };
            let d = deliver(vec![mesh(vendor.0, vendor.1)], &p);
            assert!(d.fatal.is_none(), "{:?}", d.fatal);
            let glb = d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh && f.format == "glb").expect("a derived glb");
            let sun = scene_of(&glb.bytes, "glb").lights.into_iter().find(|l| l.name.as_deref() == Some("sun")).expect("the sun");
            f64::from(sun.intensity)
        };
        let energy = crate::mesh::scene::blender_energy(&crate::mesh::scene::LightKind::Directional, 3.0);
        assert!((sun_in_glb(("fbx", blender_fbx.clone()), Some(Blender), None) - 3.0).abs() < 1e-5);
        assert!((sun_in_glb(("fbx", blender_fbx.clone()), None, Some(Blender)) - energy).abs() < 1e-6, "usd_light_units does not move an FBX read");
        assert!((sun_in_glb(("fbx", blender_fbx), None, None) - energy).abs() < 1e-6, "the default reads Intensity / 100");
        // A USD without litegen's customData (as Blender writes one): its sun
        // is 0.0010980966 of intensity, which Blender's importer takes for
        // 4 × that W/m².
        let usda = "#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n)\ndef Xform \"R\"\n{\n\
            def Mesh \"tri\" {\n int[] faceVertexCounts = [3]\n int[] faceVertexIndices = [0, 1, 2]\n point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]\n}\n\
            def DistantLight \"sun\" {\n float inputs:intensity = 0.0010980966\n bool inputs:normalize = 1\n float inputs:angle = 0\n}\n}\n";
        assert!((sun_in_glb(("usda", usda.as_bytes().to_vec()), None, Some(Blender)) - 3.0).abs() < 1e-4);
        assert!((sun_in_glb(("usda", usda.as_bytes().to_vec()), Some(Blender), None) - 0.0010980966).abs() < 1e-8, "fbx_light_units does not move a USD read");
    }

    /// A plan recorded before the light units existed parses, and one that
    /// asks none records none (so today's plans are today's bytes); the two
    /// are recorded as the lowercase word and read back.
    #[test]
    fn a_plan_without_light_units_parses() {
        use crate::types::Model3dLightUnits::{Blender, Photometric};
        let recorded = serde_json::json!({ "requested_formats": ["glb"], "fps": 30, "parts": false });
        let old = serde_json::from_value::<DeliveryPlan>(recorded).expect("an old plan parses");
        assert_eq!((old.fbx_light_units, old.usd_light_units), (None, None));
        let today = serde_json::to_value(plan(&["glb"], None)).unwrap();
        assert!(today.get("fbx_light_units").is_none() && today.get("usd_light_units").is_none(), "{today}");
        let asked = serde_json::to_value(DeliveryPlan { fbx_light_units: Some(Blender), usd_light_units: Some(Photometric), ..plan(&["glb"], None) }).unwrap();
        assert_eq!((&asked["fbx_light_units"], &asked["usd_light_units"]), (&serde_json::json!("blender"), &serde_json::json!("photometric")));
        let back = serde_json::from_value::<DeliveryPlan>(asked).unwrap();
        assert_eq!((back.fbx_light_units, back.usd_light_units), (Some(Blender), Some(Photometric)));
    }

    /// The client's `dae_morphs` is applied to the Collada delivery writes
    /// from a vendor GLB: `keep` writes the morph controllers, `bake` or
    /// nothing is today's bytes, and the GLB beside it is the same either way.
    #[test]
    fn the_requested_dae_morphs_write_every_derived_collada() {
        use crate::types::Model3dDaeMorphs::{Bake, Keep};
        let glb = crate::mesh::glb::write_scene(&crate::mesh::fixtures::morph()).unwrap();
        let made = |dae_morphs, format: &str| {
            let p = DeliveryPlan { dae_morphs, ..plan(&["glb", "dae"], Some(("+y", "+z"))) };
            let d = deliver(vec![mesh("glb", glb.clone())], &p);
            assert!(d.fatal.is_none(), "{:?}", d.fatal);
            d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh && f.format == format).unwrap_or_else(|| panic!("a {format}")).bytes.clone()
        };
        let default = made(None, "dae");
        assert!(!String::from_utf8_lossy(&default).contains("<morph"), "the default bakes the targets in");
        assert!(made(Some(Bake), "dae") == default, "bake is today's bytes");
        let kept = made(Some(Keep), "dae");
        assert!(String::from_utf8_lossy(&kept).contains("<morph "), "keep writes the controllers");
        assert!(made(Some(Keep), "glb") == made(None, "glb"), "the glb is the same either way");
    }

    /// A plan recorded before `dae_morphs` existed parses, and one that asks
    /// none records none (so today's plans are today's bytes); the choice is
    /// recorded as the lowercase word and read back.
    #[test]
    fn a_plan_without_dae_morphs_parses() {
        use crate::types::Model3dDaeMorphs::Keep;
        let recorded = serde_json::json!({ "requested_formats": ["glb"], "fps": 30, "parts": false });
        let old = serde_json::from_value::<DeliveryPlan>(recorded).expect("an old plan parses");
        assert_eq!(old.dae_morphs, None);
        let today = serde_json::to_value(plan(&["glb"], None)).unwrap();
        assert!(today.get("dae_morphs").is_none(), "{today}");
        let asked = serde_json::to_value(DeliveryPlan { dae_morphs: Some(Keep), ..plan(&["glb"], None) }).unwrap();
        assert_eq!(asked["dae_morphs"], serde_json::json!("keep"));
        let back = serde_json::from_value::<DeliveryPlan>(asked).unwrap();
        assert_eq!(back.dae_morphs, Some(Keep));
    }

    /// The plan made at submit records the request's `dae_morphs`, for any
    /// model.
    #[test]
    fn the_plan_made_at_submit_records_dae_morphs() {
        let mut models = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        models.pop();
        models.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&models).unwrap();
        for model in ["mock/morph-3d", "mock/mesh-3d"] {
            let planned = |dae_morphs| {
                let extras = crate::providers::Model3dExtras {
                    output_formats: vec!["glb".into(), "dae".into()],
                    texture: None, pbr: None, target_polycount: None, symmetry: None, topology: None, rig: None,
                    skeleton: None, joint_naming: None, height_m: None,
                    extra: None, source: None, preflight: None, fbx_color_space: None, fbx_light_units: None, usd_light_units: None, dae_morphs, usdz_root: None, usd_clips: None, format_source: None,output_packages: None, ktx2_textures: None,
                    animations: Vec::new(), fps: None,
                    texture_resolution: None, keep_uvs: None,
                    ground: None, center: None,
                };
                DeliveryPlan::for_request(catalog.get(model).unwrap(), &extras).dae_morphs
            };
            assert_eq!(planned(None), None, "{model}: nothing asked, nothing recorded");
            assert_eq!(planned(Some(crate::types::Model3dDaeMorphs::Keep)), Some(crate::types::Model3dDaeMorphs::Keep), "{model}");
            assert_eq!(planned(Some(crate::types::Model3dDaeMorphs::Bake)), Some(crate::types::Model3dDaeMorphs::Bake), "{model}: recorded as asked");
        }
    }

    /// The client's `usdz_root` is applied to the USDZ delivery writes from a
    /// vendor GLB: `usdc` makes the crate the package's first entry, `usda` or
    /// nothing is today's bytes, and the files beside it are the same either
    /// way.
    #[test]
    fn the_requested_usdz_root_writes_every_derived_usdz() {
        use crate::types::Model3dUsdzRoot::{Usda, Usdc};
        let glb = crate::mesh::glb::write_scene(&crate::mesh::fixtures::textured()).unwrap();
        let made = |usdz_root, format: &str| {
            let p = DeliveryPlan { usdz_root, ..plan(&["glb", "usdz", "usda"], Some(("+y", "+z"))) };
            let d = deliver(vec![mesh("glb", glb.clone())], &p);
            assert!(d.fatal.is_none() && d.failures.is_empty(), "{:?} {:?}", d.fatal, d.failures);
            d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh && f.format == format).unwrap_or_else(|| panic!("a {format}")).bytes.clone()
        };
        let first_entry = |usdz: &[u8]| crate::mesh::zip::Archive::parse(usdz).expect("a zip").entries[0].name.clone();
        let default = made(None, "usdz");
        assert_eq!(first_entry(&default), "model.usda", "the default root is text");
        assert!(made(Some(Usda), "usdz") == default, "usda is today's bytes");
        let crated = made(Some(Usdc), "usdz");
        assert_eq!(first_entry(&crated), "model.usdc", "usdc roots the package in the crate");
        assert!(crated != default);
        let read = |bytes: &[u8]| scene_of(bytes, "usdz");
        let from_crate = read(&crated);
        assert_eq!(from_crate, transforms_at_f32(read(&default), &from_crate), "either root reads back as the same scene (transforms at f32)");
        assert!(made(Some(Usdc), "usda") == made(None, "usda"), "the usda is the same either way");
        assert!(made(Some(Usdc), "glb") == made(None, "glb"), "the glb is the same either way");
    }

    /// The client's `usd_clips` is applied to every USD delivery writes from a
    /// vendor GLB that holds three clips (mesh-gaps item 6, Ruling U6): `all`
    /// keeps each clip, in the order and under the names they had, `first`
    /// keeps the first, and nothing asked is the container's default, which is
    /// every clip for a USDA and `USDZ_DEFAULT_CLIPS` for a USDZ. The GLB, which
    /// holds clips of its own, is the same either way.
    #[test]
    fn the_requested_usd_clips_decides_how_many_clips_every_derived_usd_keeps() {
        use crate::types::Model3dUsdClips::{All, First};
        let glb = crate::mesh::glb::write_scene(&crate::mesh::fixtures::clips()).unwrap();
        let clips_of = |usd_clips, format: &str| {
            let p = DeliveryPlan { usd_clips, ..plan(&["glb", "usdz", "usda"], Some(("+y", "+z"))) };
            let d = deliver(vec![mesh("glb", glb.clone())], &p);
            assert!(d.fatal.is_none() && d.failures.is_empty(), "{:?} {:?}", d.fatal, d.failures);
            let file = d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh && f.format == format).unwrap_or_else(|| panic!("a {format}"));
            scene_of(&file.bytes, format).animations.iter().map(|a| a.name.clone().unwrap_or_default()).collect::<Vec<String>>()
        };
        let every = ["walk", "run", "wave"].map(String::from).to_vec();
        let first = vec!["walk".to_string()];
        let usdz_default = match crate::mesh::USDZ_DEFAULT_CLIPS {
            crate::mesh::UsdClips::All => &every,
            crate::mesh::UsdClips::First => &first,
        };
        assert_eq!(clips_of(None, "usda"), every, "a USDA keeps every clip unless told otherwise");
        assert_eq!(clips_of(None, "usdz"), *usdz_default, "a USDZ keeps what its default says");
        for format in ["usda", "usdz"] {
            assert_eq!(clips_of(Some(All), format), every, "{format}: all");
            assert_eq!(clips_of(Some(First), format), first, "{format}: first");
        }
        for asked in [None, Some(All), Some(First)] {
            assert_eq!(clips_of(asked, "glb"), every, "the glb keeps its clips whatever the USD is asked for");
        }
    }

    /// A plan recorded before `usdz_root` and `usd_clips` existed parses, and
    /// one that asks neither records neither (so today's plans are today's
    /// bytes); each choice is recorded as the lowercase word and read back.
    #[test]
    fn a_plan_without_usdz_root_or_usd_clips_parses() {
        use crate::types::{Model3dUsdClips, Model3dUsdzRoot};
        let recorded = serde_json::json!({ "requested_formats": ["glb"], "fps": 30, "parts": false });
        let old = serde_json::from_value::<DeliveryPlan>(recorded).expect("an old plan parses");
        assert_eq!((old.usdz_root, old.usd_clips), (None, None));
        let today = serde_json::to_value(plan(&["glb"], None)).unwrap();
        assert!(today.get("usdz_root").is_none() && today.get("usd_clips").is_none(), "{today}");
        let asked = serde_json::to_value(DeliveryPlan {
            usdz_root: Some(Model3dUsdzRoot::Usdc),
            usd_clips: Some(Model3dUsdClips::All),
            ..plan(&["glb"], None)
        })
        .unwrap();
        assert_eq!((&asked["usdz_root"], &asked["usd_clips"]), (&serde_json::json!("usdc"), &serde_json::json!("all")));
        let back = serde_json::from_value::<DeliveryPlan>(asked).unwrap();
        assert_eq!((back.usdz_root, back.usd_clips), (Some(Model3dUsdzRoot::Usdc), Some(Model3dUsdClips::All)));
    }

    /// The plan made at submit records the request's `usdz_root` and
    /// `usd_clips`, for any model.
    #[test]
    fn the_plan_made_at_submit_records_usdz_root_and_usd_clips() {
        use crate::types::{Model3dUsdClips, Model3dUsdzRoot};
        let mut models = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        models.pop();
        models.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&models).unwrap();
        for model in ["mock/morph-3d", "mock/mesh-3d"] {
            let planned = |usdz_root, usd_clips| {
                let extras = crate::providers::Model3dExtras {
                    output_formats: vec!["glb".into(), "usdz".into()],
                    texture: None, pbr: None, target_polycount: None, symmetry: None, topology: None, rig: None,
                    skeleton: None, joint_naming: None, height_m: None,
                    extra: None, source: None, preflight: None, fbx_color_space: None, fbx_light_units: None, usd_light_units: None, dae_morphs: None, usdz_root, usd_clips, format_source: None, output_packages: None, ktx2_textures: None,
                    animations: Vec::new(), fps: None,
                    texture_resolution: None, keep_uvs: None,
                    ground: None, center: None,
                };
                let plan = DeliveryPlan::for_request(catalog.get(model).unwrap(), &extras);
                (plan.usdz_root, plan.usd_clips)
            };
            assert_eq!(planned(None, None), (None, None), "{model}: nothing asked, nothing recorded");
            assert_eq!(planned(Some(Model3dUsdzRoot::Usdc), None), (Some(Model3dUsdzRoot::Usdc), None), "{model}");
            assert_eq!(planned(Some(Model3dUsdzRoot::Usda), None), (Some(Model3dUsdzRoot::Usda), None), "{model}: recorded as asked");
            assert_eq!(planned(None, Some(Model3dUsdClips::All)), (None, Some(Model3dUsdClips::All)), "{model}");
            assert_eq!(planned(None, Some(Model3dUsdClips::First)), (None, Some(Model3dUsdClips::First)), "{model}: recorded as asked");
        }
    }

    /// 3D doc §3C, a3 #4 (2026-10-31): the requested `fps` reaches the
    /// containers that resample (FBX, Collada, USD), which keyed their own
    /// 30 fps grid between the rekeyed keys and declared 30.
    #[test]
    fn every_animated_container_is_keyed_at_the_requested_fps() {
        let plan = DeliveryPlan { fps: Some(4), ..clip_plan(&["walk"]) };
        let plan = DeliveryPlan { requested_formats: vec!["glb".into(), "fbx".into(), "dae".into(), "usdz".into()], ..plan };
        // Hips that move (a joint, which every container animates).
        let rig = walker(false, false, &[(0.0, [0.0, 0.95, 0.0]), (0.5, [0.0, 0.95, 0.3]), (1.0, [0.0, 0.95, 0.6])]);
        let d = deliver(vec![mesh("glb", crate::mesh::glb::write_scene(&rig).unwrap())], &plan);
        assert!(d.fatal.is_none(), "{:?}", d.fatal);
        for fmt in ["fbx", "dae", "usdz"] {
            let f = d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh && f.format == fmt).unwrap_or_else(|| panic!("a delivered {fmt}"));
            let scene = crate::mesh::read_scene(&f.bytes, MeshFormat::parse(fmt).unwrap(), &crate::mesh::package::NoSidecars).unwrap().scene;
            for ch in &scene.animations[0].channels {
                for t in &ch.times {
                    let k = f64::from(*t) * 4.0;
                    assert!((k - k.round()).abs() < 1e-3, "{fmt}: a key at {t} s is off the 4 fps grid");
                }
            }
        }
    }

    /// 3D doc §3B, §3C: what delivery could not do reaches the caller on the
    /// asset — a naming the rig cannot take, a texture left out (named by
    /// what it was), and what a derived container lost — not only the log.
    #[test]
    fn what_delivery_could_not_do_is_named_on_the_asset() {
        // A joint naming litegen cannot read this rig in.
        let mut s = crate::providers::model3d::humanoid::rigged_humanoid(&crate::providers::model3d::humanoid::MOCK_JOINTS);
        for n in &mut s.nodes {
            n.name = n.name.as_ref().map(|_| "bone".to_string());
        }
        let renamed = crate::mesh::glb::write_scene(&s).unwrap();
        let naming = DeliveryPlan { joint_naming: Some("unity".into()), ..plan(&["glb"], Some(("+y", "+z"))) };
        let d = deliver(vec![mesh("glb", renamed)], &naming);
        let notes = &d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh).unwrap().manifest.as_ref().unwrap().notes;
        assert!(notes.iter().any(|n| n.contains("unity")), "{notes:?}");

        // A WebP texture that will not decode is named as WebP, not KTX2.
        let mut scene = crate::mesh::fixtures::textured();
        scene.images[0].format = crate::mesh::scene::ImageFormat::Webp;
        scene.images[0].data = b"RIFF\x10\x00\x00\x00WEBPVP8 broken".to_vec();
        let d = deliver(vec![mesh("glb", crate::mesh::glb::write_scene(&scene).unwrap())], &plan(&["glb"], Some(("+y", "+z"))));
        let notes = &d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh).unwrap().manifest.as_ref().unwrap().notes;
        assert!(notes.iter().any(|n| n.contains("WebP") && !n.contains("KTX2")), "{notes:?}");

        // Two clips, delivered as USDZ too: what that container cannot hold is named on it.
        let two = animated(&[("walk", &[0.0, 1.0]), ("run", &[0.0, 0.5])]);
        let d = deliver(vec![mesh("glb", two)], &DeliveryPlan { requested_formats: vec!["glb".into(), "usdz".into()], ..clip_plan(&["walk", "run"]) });
        let usdz = d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh && f.format == "usdz").expect("a usdz");
        assert!(!usdz.manifest.as_ref().map(|m| m.notes.clone()).unwrap_or_default().is_empty(), "the clip USDZ cannot keep is named");
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

    /// The 2026-10-31 audit: the upper arm's angle was measured from its
    /// joint's world position to the stored (bind-space) centre of the
    /// vertices it drives, so anything above the joints — the mock's height
    /// root, litegen's own placement or orientation root — tilted it: the
    /// mock T-pose read "unknown" at 1.4 m and "a" at 2.0 m. Both ends are
    /// now where the vertices are drawn (glTF 2.0 §3.7.3).
    #[test]
    fn the_bind_pose_is_read_the_same_whatever_sits_above_the_joints() {
        for height in [1.0, 1.4, 2.0, 3.0] {
            let rig = crate::providers::model3d::humanoid::rigged_humanoid_glb_sized(&crate::providers::model3d::humanoid::MOCK_JOINTS, height);
            let d = deliver(vec![mesh("glb", rig)], &plan(&["glb"], Some(("+y", "+z"))));
            let skin = meshes(&d)[0].manifest.clone().unwrap().skin.unwrap();
            assert_eq!(skin.bind_pose, "t", "{height} m");
        }
        let rig = crate::providers::model3d::humanoid::rigged_humanoid_glb(&crate::providers::model3d::humanoid::MOCK_JOINTS);
        let placed = DeliveryPlan { placement: Some(PlacementPlan { ground: true, center: true, height_m: Some(3.5) }), ..plan(&["glb"], Some(("+y", "+z"))) };
        let skin = meshes(&deliver(vec![mesh("glb", rig)], &placed))[0].manifest.clone().unwrap().skin.unwrap();
        assert_eq!(skin.bind_pose, "t", "placed at 3.5 m");
    }

    /// The 2026-10-31 audit: a clip keyed from 0.5 s reported 1.0 s in glb
    /// and 1.5 s in fbx and dae, which start every take at 0. Clips last from
    /// t = 0 (glTF 2.0 §3.11).
    #[test]
    fn a_clip_lasts_the_same_in_every_container() {
        let d = deliver(
            vec![mesh("glb", animated(&[("walk", &[0.5, 1.5])]))],
            &DeliveryPlan { requested_formats: vec!["glb".into(), "fbx".into(), "dae".into()], ..clip_plan(&["walk"]) },
        );
        for m in meshes(&d) {
            let a = &m.manifest.as_ref().unwrap().animations[0];
            assert!((a.duration_s - 1.5).abs() < 1e-3, "{}: {}", m.format, a.duration_s);
        }
    }

    /// The 2026-10-31 audit: options delivery could not apply were dropped
    /// without a word — a height for a flat mesh, an fps no clip can be keyed
    /// at, a plan recorded on the row that does not parse — and a container
    /// that keeps fewer clips than asked said nothing either.
    #[test]
    fn options_that_cannot_be_applied_are_named_on_the_asset() {
        let notes = |d: &Delivered, format: &str| {
            d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh && f.format == format).unwrap().manifest.as_ref().unwrap().notes.clone()
        };
        // A flat quad has no height to scale.
        let mut flat = crate::mesh::fixtures::cube();
        for p in flat.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
            for q in p.positions.iter_mut() {
                q[1] = 0.0;
            }
        }
        let glb = crate::mesh::glb::write_scene(&flat).unwrap();
        let tall = DeliveryPlan { placement: Some(PlacementPlan { ground: false, center: false, height_m: Some(2.0) }), ..plan(&["glb"], Some(("+y", "+z"))) };
        let d = deliver(vec![mesh("glb", glb)], &tall);
        assert!(notes(&d, "glb").iter().any(|n| n.contains("height_m 2")), "{:?}", notes(&d, "glb"));

        // An fps no clip can be keyed at.
        let d = deliver(vec![mesh("glb", animated(&[("walk", &[0.0, 1.0])]))], &DeliveryPlan { fps: Some(u32::MAX), ..clip_plan(&["walk"]) });
        assert!(d.fatal.is_none(), "{:?}", d.fatal);
        assert!(notes(&d, "glb").iter().any(|n| n.contains("not rekeyed")), "{:?}", notes(&d, "glb"));

        // A container that keeps the first clip only. (Clips on the hips:
        // `animated` keys the skinned mesh's own node, whose transform glTF
        // ignores, and which USD rightly drops.)
        let mut s = crate::providers::model3d::humanoid::rigged_humanoid(&crate::providers::model3d::humanoid::MOCK_JOINTS);
        let hips = s.nodes.iter().position(|n| n.name.as_deref() == Some("hips")).unwrap();
        for (name, end) in [("walk", 1.0f32), ("run", 0.5)] {
            s.animations.push(crate::mesh::scene::Animation {
                name: Some(name.into()),
                channels: vec![crate::mesh::scene::Channel {
                    node: hips,
                    path: crate::mesh::scene::AnimPath::Rotation,
                    interpolation: crate::mesh::scene::Interpolation::Linear,
                    times: vec![0.0, end],
                    values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.38268343, 0.0, 0.9238795],
                }],
            });
        }
        let two = crate::mesh::glb::write_scene(&s).unwrap();
        let d = deliver(vec![mesh("glb", two)], &DeliveryPlan { requested_formats: vec!["glb".into(), "usdz".into()], ..clip_plan(&["walk", "run"]) });
        assert!(notes(&d, "usdz").iter().any(|n| n.contains("holds the clips [walk] for the [walk, run] requested")), "{:?}", notes(&d, "usdz"));
        assert!(!notes(&d, "glb").iter().any(|n| n.contains("holds the clips")), "glb keeps both");

        // A recorded plan that does not parse.
        let now = chrono::Utc::now();
        let row = crate::types::Generation {
            id: "g1".into(),
            key_id: None,
            model: "mock/mesh-3d".into(),
            provider: "mock".into(),
            media_type: "model3d".into(),
            status: crate::types::GenerationStatus::Processing,
            progress: 50,
            provider_job_id: None,
            result_url: None,
            error_message: None,
            cost_usd: 0.0,
            created_at: now,
            completed_at: None,
            metadata: Some(serde_json::json!({ "requested_formats": ["glb"], "delivery": { "requested_formats": "glb" } })),
            org_id: None,
            app_id: None,
            files_deleted_at: None,
            files_deleted_reason: None,
        };
        let p = DeliveryPlan::from_row(&row);
        assert!(p.unreadable.is_some());
        let d = deliver(vec![mesh("glb", crate::providers::model3d::glb::generate_cube_glb("a crate"))], &p);
        assert!(notes(&d, "glb").iter().any(|n| n.contains("could not be read")), "{:?}", notes(&d, "glb"));
    }

    /// The 2026-10-31 audit: joint names were made unique before the
    /// joint-naming renames, so "head" renamed to Unity's "Head" beside a
    /// mesh node already called "Head" delivered two nodes of one name.
    #[test]
    fn names_stay_unique_after_joint_naming() {
        let mut s = crate::providers::model3d::humanoid::rigged_humanoid(&crate::providers::model3d::humanoid::MOCK_JOINTS);
        let mesh_node = s.nodes.iter().position(|n| n.mesh.is_some()).unwrap();
        s.nodes[mesh_node].name = Some("Head".into());
        let glb = crate::mesh::glb::write_scene(&s).unwrap();
        let d = deliver(vec![mesh("glb", glb)], &DeliveryPlan { joint_naming: Some("unity".into()), ..plan(&["glb"], Some(("+y", "+z"))) });
        let back = scene_of(&meshes(&d)[0].bytes, "glb");
        let names: Vec<&str> = back.nodes.iter().filter_map(|n| n.name.as_deref()).collect();
        let distinct: std::collections::HashSet<&&str> = names.iter().collect();
        assert_eq!(distinct.len(), names.len(), "{names:?}");
    }

    /// The mock humanoid (hips = node 1) with `motion` as the clip "walk",
    /// optionally under a static `root` joint or a non-joint `armature` that
    /// carries the motion instead.
    fn walker(root_joint: bool, armature_moves: bool, hips_keys: &[(f32, [f32; 3])]) -> crate::mesh::scene::Scene {
        use crate::mesh::scene::{AnimPath, Animation, Channel, Interpolation, Node};
        let mut s = crate::providers::model3d::humanoid::rigged_humanoid(&crate::providers::model3d::humanoid::MOCK_JOINTS);
        let mut channels = Vec::new();
        if !hips_keys.is_empty() {
            channels.push(Channel {
                node: 1,
                path: AnimPath::Translation,
                interpolation: Interpolation::Linear,
                times: hips_keys.iter().map(|k| k.0).collect(),
                values: hips_keys.iter().flat_map(|k| k.1).collect(),
            });
        }
        if root_joint {
            s.roots.retain(|r| *r != 1);
            let root = s.add_root(Node { name: Some("root".into()), children: vec![1], ..Default::default() });
            s.skins[0].joints.push(root);
            s.skins[0].inverse_bind_matrices.push(crate::mesh::math::IDENTITY);
            s.skins[0].skeleton = Some(root);
        }
        if armature_moves {
            s.roots.retain(|r| *r != 1);
            let arm = s.add_root(Node { name: Some("Armature".into()), children: vec![1], ..Default::default() });
            channels.push(Channel {
                node: arm,
                path: AnimPath::Translation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            });
        }
        s.animations.push(Animation { name: Some("walk".into()), channels });
        s
    }

    fn hips_travel(glb: &[u8]) -> f32 {
        let s = scene_of(glb, "glb");
        let facts = crate::mesh::inspect::inspect(&s).unwrap();
        let c = &facts.animations[0];
        assert!(c.root_travel_m.iter().all(|v| v.is_finite()));
        c.excursion_m
    }

    /// Final review, finding 2 (2026-11-01): `fps` and `in_place` together. The
    /// hold re-keyed the hips at every key plus a 30 Hz grid after the clip
    /// was rekeyed at `fps`, so the hips came off the requested rate (and FBX
    /// declared 24 fps over keys between its frames). Held at the clip's own
    /// keys, every channel stays on the grid, and the hips still hold.
    #[test]
    fn an_in_place_clip_keyed_at_a_requested_fps_stays_on_its_grid() {
        let travel = [(0.0, [0.0, 0.95, 0.0]), (1.0, [0.0, 0.95, 1.0])];
        let scene = walker(true, false, &travel);
        let plan = DeliveryPlan {
            fps: Some(24),
            clips: vec![ClipPlan { name: "walk".into(), in_place: Some(true), loop_requested: None }],
            ..plan(&["glb", "fbx"], Some(("+y", "+z")))
        };
        let d = deliver(vec![mesh("glb", crate::mesh::glb::write_scene(&scene).unwrap())], &plan);
        assert!(d.fatal.is_none(), "{:?}", d.fatal);
        for fmt in ["glb", "fbx"] {
            let f = d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh && f.format == fmt).unwrap();
            let s = crate::mesh::read_scene(&f.bytes, MeshFormat::parse(fmt).unwrap(), &crate::mesh::package::NoSidecars).unwrap().scene;
            for ch in &s.animations[0].channels {
                for t in &ch.times {
                    let k = f64::from(*t) * 24.0;
                    assert!((k - k.round()).abs() < 1e-3, "{fmt}: a key at {t} s is off the 24 fps grid");
                }
            }
        }
        let glb = d.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh && f.format == "glb").unwrap();
        assert!(hips_travel(&glb.bytes) < 0.01, "the hips still travel {} m", hips_travel(&glb.bytes));
    }

    /// The 2026-10-31 audit: in-place was measured and held on the topmost
    /// joint, start to end. A static `root` above travelling hips (VRM's own
    /// hierarchy; UE and CC rigs too) read as in place, so nothing was held;
    /// motion on a non-joint "Armature" could not be held and said nothing;
    /// a clip going out and back counted as in place. The hips are what
    /// moves (VRM 1.0 humanoid.md: "Usually only this bone moves"), held in
    /// world space, measured over the whole clip.
    #[test]
    fn in_place_holds_the_hips_wherever_the_motion_comes_from() {
        let travel = [(0.0, [0.0, 0.95, 0.0]), (1.0, [0.0, 0.95, 1.0])];
        let out_and_back = [(0.0, [0.0, 0.95, 0.0]), (0.5, [0.0, 0.95, 1.0]), (1.0, [0.0, 0.95, 0.0])];
        let cases = [
            ("a static root joint above travelling hips", walker(true, false, &travel)),
            ("motion on a non-joint armature", walker(false, true, &[])),
            ("an out-and-back clip", walker(false, false, &out_and_back)),
        ];
        let in_place = DeliveryPlan {
            clips: vec![ClipPlan { name: "walk".into(), in_place: Some(true), loop_requested: None }],
            ..plan(&["glb"], Some(("+y", "+z")))
        };
        for (what, scene) in cases {
            let facts = crate::mesh::inspect::inspect(&scene).unwrap();
            assert!(!facts.animations[0].in_place(), "{what}: measured as in place");
            let d = deliver(vec![mesh("glb", crate::mesh::glb::write_scene(&scene).unwrap())], &in_place);
            assert!(d.fatal.is_none(), "{what}: {:?}", d.fatal);
            let m = meshes(&d)[0];
            assert!(hips_travel(&m.bytes) < 0.01, "{what}: the hips still travel {} m", hips_travel(&m.bytes));
            let a = &m.manifest.as_ref().unwrap().animations[0];
            assert_eq!((a.in_place, a.in_place_by.as_deref()), (true, Some("litegen")), "{what}");
        }
    }

    /// `glb` with its JSON chunk edited, the BIN chunk kept.
    fn with_json(glb: &[u8], edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let mut doc: serde_json::Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
        edit(&mut doc);
        let mut json = serde_json::to_vec(&doc).unwrap();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let rest = &glb[20 + json_len..];
        let mut out = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&((12 + 8 + json.len() + rest.len()) as u32).to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json);
        out.extend_from_slice(rest);
        out
    }

    /// The 2026-10-12 review, findings 29 and M1: what the reader dropped
    /// from the vendor's file (a material extension it does not carry) never
    /// reached the asset.
    #[test]
    fn what_the_reader_dropped_from_the_vendor_file_is_named_on_the_asset() {
        let cube = crate::providers::model3d::glb::generate_cube_glb("a crate");
        let vendor = with_json(&cube, |doc| {
            doc["extensionsUsed"] = serde_json::json!(["KHR_materials_variants"]);
            doc["extensions"] = serde_json::json!({ "KHR_materials_variants": { "variants": [{ "name": "red" }] } });
        });
        let d = deliver(vec![mesh("glb", vendor)], &plan(&["glb", "obj"], Some(("+y", "+z"))));
        assert!(d.fatal.is_none(), "{:?}", d.fatal);
        for m in meshes(&d) {
            let notes = &m.manifest.as_ref().unwrap().notes;
            assert!(notes.iter().any(|n| n.contains("KHR_materials_variants")), "{}: {notes:?}", m.format);
        }
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

    // ── output_packages: each multi-file container as a zip ──

    fn packaged(formats: &[&str]) -> DeliveryPlan {
        DeliveryPlan { packages: true, ..plan(formats, Some(("+y", "+z"))) }
    }

    fn packages(d: &Delivered) -> Vec<&Model3dFile> {
        d.files.iter().filter(|f| f.kind == Model3dAssetKind::Package).collect()
    }

    fn unzipped(f: &Model3dFile) -> Vec<crate::mesh::PackageFile> {
        let a = crate::mesh::zip::Archive::parse(&f.bytes).expect("a package is a zip");
        a.entries.iter().map(|e| crate::mesh::PackageFile { path: e.name.clone(), bytes: a.read(e).unwrap() }).collect()
    }

    /// Sidecars that remember every path a reader asks for.
    struct Asked<'a> {
        files: &'a [crate::mesh::PackageFile],
        asked: std::cell::RefCell<Vec<String>>,
    }

    impl crate::mesh::Sidecars for Asked<'_> {
        fn get(&self, path: &str) -> Option<Vec<u8>> {
            self.asked.borrow_mut().push(path.to_string());
            self.files.iter().find(|f| f.path == path).map(|f| f.bytes.clone())
        }
    }

    const PACKAGED: [&str; 5] = ["obj", "dae", "usda", "usdc", "gltf"];

    #[test]
    fn packages_are_made_only_when_asked_and_change_no_other_file() {
        let vendor = crate::mesh::glb::write_scene(&crate::mesh::fixtures::textured()).unwrap();
        // A preview beside the mesh, as vendors send: it keeps its place too.
        let preview = Model3dFile {
            kind: Model3dAssetKind::Preview,
            format: "png".into(),
            content_type: "image/png".into(),
            bytes: crate::mesh::fixtures::quadrant_png(),
            polycount: None,
            width: None,
            height: None,
            manifest: None,
            source_url: None,
        };
        let formats = ["glb", "obj", "dae", "usda", "usdc", "gltf", "fbx"];
        let without = deliver(vec![mesh("glb", vendor.clone()), preview.clone()], &plan(&formats, Some(("+y", "+z"))));
        let with = deliver(vec![mesh("glb", vendor), preview], &packaged(&formats));
        assert!(without.files.iter().any(|f| f.kind == Model3dAssetKind::Preview));
        assert!(without.fatal.is_none() && with.fatal.is_none(), "{:?} {:?}", without.fatal, with.fatal);
        assert!(packages(&without).is_empty());
        // Every file of today's delivery, in its place, byte for byte.
        let n = without.files.len();
        let same = |a: &Model3dFile, b: &Model3dFile| a.kind == b.kind && a.format == b.format && a.bytes == b.bytes && a.manifest == b.manifest;
        assert!(with.files.len() > n && without.files.iter().zip(&with.files).all(|(a, b)| same(a, b)), "the packages come after every other file");
        let got: Vec<&str> = packages(&with).iter().map(|p| p.format.as_str()).collect();
        assert_eq!(got, PACKAGED, "one per multi-file container, in request order; glb and fbx are one file");
        for p in packages(&with) {
            assert_eq!(p.content_type, "application/zip");
            assert_eq!(p.polycount, Some(crate::mesh::fixtures::textured().triangle_count() as u32));
            let files = unzipped(p);
            assert!(files.len() > 1, "{}: {:?}", p.format, files.iter().map(|f| &f.path).collect::<Vec<_>>());
            let m = p.manifest.as_ref().unwrap_or_else(|| panic!("{}: a package carries its own manifest", p.format));
            assert!(m.materials.iter().any(|mat| mat.maps.iter().any(|x| x == "base_color")), "{}: {m:?}", p.format);
            let lost = |n: &&String| n.starts_with("textures:") || (n.starts_with("materials:") && n.contains("vertices' colours"));
            assert!(!m.notes.iter().any(|n| lost(&n)), "{}: keeps its textures and materials: {:?}", p.format, m.notes);
        }
    }

    /// Review Focus 5: a plan recorded before `packages` existed reads as
    /// asking none, and a plan that asks none records nothing new.
    #[test]
    fn a_plan_without_packages_parses() {
        let recorded = serde_json::json!({ "requested_formats": ["glb", "obj"], "fps": 30, "parts": false });
        let read: DeliveryPlan = serde_json::from_value(recorded).expect("an old plan parses");
        assert!(!read.packages && read.unreadable.is_none());
        let today = serde_json::to_value(plan(&["obj"], Some(("+y", "+z")))).unwrap();
        assert!(today.get("packages").is_none(), "{today}");
        let asked = serde_json::to_value(packaged(&["obj"])).unwrap();
        assert_eq!(asked["packages"], true);
        assert!(serde_json::from_value::<DeliveryPlan>(asked).unwrap().packages);
    }

    /// A plan recorded before `ktx2_textures` existed reads as `png`; `png`
    /// records nothing (so today's plans are today's bytes); `keep` is
    /// recorded and read back.
    #[test]
    fn a_plan_without_ktx2_textures_parses() {
        use crate::types::Model3dKtx2Textures::{Keep, Png};
        let recorded = serde_json::json!({ "requested_formats": ["glb"], "fps": 30, "parts": false });
        assert_eq!(serde_json::from_value::<DeliveryPlan>(recorded).expect("an old plan parses").ktx2_textures, None);
        let today = serde_json::to_value(plan(&["glb"], None)).unwrap();
        assert!(today.get("ktx2_textures").is_none(), "{today}");
        let asked = serde_json::to_value(DeliveryPlan { ktx2_textures: Some(Keep), ..plan(&["glb"], None) }).unwrap();
        assert_eq!(asked["ktx2_textures"], "keep");
        assert_eq!(serde_json::from_value::<DeliveryPlan>(asked).unwrap().ktx2_textures, Some(Keep));

        let mut models = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        models.pop();
        models.push("models");
        let schema = crate::capabilities::CapabilityRegistry::from_dir(&models).unwrap().get("mock/basisu-3d").unwrap().clone();
        let planned = |ktx2_textures| {
            let extras = crate::providers::Model3dExtras {
                output_formats: vec!["glb".into()],
                texture: None, pbr: None, target_polycount: None, symmetry: None, topology: None, rig: None,
                skeleton: None, joint_naming: None, height_m: None,
                extra: None, source: None, preflight: None, fbx_color_space: None, fbx_light_units: None, usd_light_units: None, dae_morphs: None, usdz_root: None, usd_clips: None, format_source: None,output_packages: None, ktx2_textures,
                animations: Vec::new(), fps: None,
                texture_resolution: None, keep_uvs: None,
                ground: None, center: None,
            };
            DeliveryPlan::for_request(&schema, &extras).ktx2_textures
        };
        assert_eq!((planned(None), planned(Some(Png)), planned(Some(Keep))), (None, None, Some(Keep)));
    }

    /// The vendor's glb of the mock `mock/basisu-3d` model: a base-colour
    /// map that is a KTX2 image, through `KHR_texture_basisu` (required).
    const BASISU: &[u8] = include_bytes!("../providers/model3d/mock_data/textured.basisu.glb");
    const KTX2_FORMATS: [&str; 5] = ["glb", "gltf", "fbx", "usdz", "3mf"];

    /// What `ktx2_textures` makes of the vendor's KTX2: PNG in every container
    /// by default, and with `keep` the vendor's bytes in the glb and gltf, which
    /// say they require `KHR_texture_basisu`. The vendor's file stays beside as
    /// `original` either way.
    #[test]
    fn ktx2_textures_keep_applies_to_glb_and_gltf_only() {
        use crate::mesh::scene::ImageFormat::{Ktx2, Png};
        let vendor_image = scene_of(BASISU, "glb").images[0].data.clone();
        let deliver_as = |ktx2_textures| {
            let plan = DeliveryPlan { requested_formats: KTX2_FORMATS.iter().map(|f| f.to_string()).collect(), ktx2_textures, ..Default::default() };
            let d = deliver(vec![mesh("glb", BASISU.to_vec())], &plan);
            assert!(d.fatal.is_none() && d.failures.is_empty(), "{:?} {:?}", d.fatal, d.failures);
            assert!(d.files.iter().any(|f| f.kind == Model3dAssetKind::Original && f.bytes == BASISU), "the vendor's file stays beside");
            d
        };
        for (ktx2_textures, kept) in [(None, false), (Some(crate::types::Model3dKtx2Textures::Png), false), (Some(crate::types::Model3dKtx2Textures::Keep), true)] {
            let d = deliver_as(ktx2_textures);
            for f in meshes(&d) {
                let scene = scene_of(&f.bytes, &f.format);
                let keeps = kept && (f.format == "glb" || f.format == "gltf");
                assert_eq!(scene.images.iter().map(|i| i.format).collect::<Vec<_>>(), [if keeps { Ktx2 } else { Png }], "{ktx2_textures:?} {}", f.format);
                if keeps {
                    assert_eq!(scene.images[0].data, vendor_image, "{}: the vendor's bytes", f.format);
                }
                let used = &f.manifest.as_ref().unwrap().extensions_used;
                assert_eq!(used.iter().any(|e| e == "KHR_texture_basisu"), keeps, "{ktx2_textures:?} {}: {used:?}", f.format);
            }
        }
    }

    /// A KTX2 litegen cannot decode is left out of the containers that need
    /// PNG, and named there; a glb that keeps it carries the vendor's bytes
    /// and no note that it was lost.
    #[test]
    fn a_ktx2_that_does_not_decode_is_noted_only_where_it_is_left_out() {
        let mut scene = scene_of(BASISU, "glb");
        let cut = scene.images[0].data.len() - 200;
        scene.images[0].data.truncate(cut);
        let broken = crate::mesh::glb::write_scene(&scene).unwrap();
        let plan = DeliveryPlan { requested_formats: vec!["glb".into(), "fbx".into()], ktx2_textures: Some(crate::types::Model3dKtx2Textures::Keep), ..Default::default() };
        let d = deliver(vec![mesh("glb", broken)], &plan);
        assert!(d.fatal.is_none(), "{:?}", d.fatal);
        let notes = |format: &str| meshes(&d).into_iter().find(|f| f.format == format).unwrap().manifest.as_ref().unwrap().notes.clone();
        let (glb, fbx) = (notes("glb"), notes("fbx"));
        assert!(glb.iter().all(|n| !n.contains("could not be decoded")), "kept, not lost: {glb:?}");
        assert!(fbx.iter().any(|n| n.contains("(KTX2) could not be decoded and was left out: ")), "{fbx:?}");
        let kept = scene_of(&meshes(&d).into_iter().find(|f| f.format == "glb").unwrap().bytes, "glb");
        assert_eq!(kept.images[0].format, crate::mesh::scene::ImageFormat::Ktx2);
        assert!(scene_of(&meshes(&d).into_iter().find(|f| f.format == "fbx").unwrap().bytes, "fbx").materials.iter().all(|m| m.base_color_texture.is_none()), "the fbx lost the map");
        // The log counts what was lost in some container asked for: the fbx
        // lost it here, but a request for the glb alone loses nothing.
        assert!(d.failures.iter().any(|(_, why)| why.contains("could not be decoded")), "{:?}", d.failures);
        let glb_only = DeliveryPlan { requested_formats: vec!["glb".into()], ..plan.clone() };
        let d = deliver(vec![mesh("glb", crate::mesh::glb::write_scene(&scene).unwrap())], &glb_only);
        assert!(d.fatal.is_none() && d.failures.is_empty(), "a KTX2 kept in every container asked for is no failure: {:?}", d.failures);
    }

    /// With `keep`, a WebP still becomes PNG (once, in both the kept glb and
    /// the other containers), and a request with no glb or gltf in it
    /// delivers what it would without `keep`.
    #[test]
    fn keep_leaves_webp_png_and_changes_nothing_when_no_glb_or_gltf_is_asked_for() {
        use crate::mesh::scene::{Image, ImageFormat, Texture, TextureRef};
        let mut scene = scene_of(BASISU, "glb");
        let png = crate::mesh::fixtures::textured().images[0].data.clone();
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp).encode(decoded.as_bytes(), decoded.width(), decoded.height(), decoded.color().into()).unwrap();
        scene.images.push(Image { name: Some("webp_map".into()), format: ImageFormat::Webp, data: webp });
        scene.textures.push(Texture { name: None, image: scene.images.len() - 1, sampler: Default::default() });
        scene.materials[0].emissive_texture = Some(TextureRef { texture: scene.textures.len() - 1, uv_set: 0, transform: None });
        let vendor = crate::mesh::glb::write_scene(&scene).unwrap();
        let keep = Some(crate::types::Model3dKtx2Textures::Keep);
        let ask = |formats: &[&str], ktx2_textures| {
            let plan = DeliveryPlan { requested_formats: formats.iter().map(|f| f.to_string()).collect(), ktx2_textures, ..Default::default() };
            let d = deliver(vec![mesh("glb", vendor.clone())], &plan);
            assert!(d.fatal.is_none() && d.failures.is_empty(), "{:?} {:?}", d.fatal, d.failures);
            d
        };
        let formats_of = |d: &Delivered, format: &str| -> Vec<ImageFormat> {
            scene_of(&meshes(d).into_iter().find(|f| f.format == format).unwrap().bytes, format).images.iter().map(|i| i.format).collect()
        };
        let d = ask(&["glb", "fbx"], keep);
        assert_eq!(formats_of(&d, "glb"), [ImageFormat::Ktx2, ImageFormat::Png], "the glb keeps the KTX2; the WebP is PNG");
        assert_eq!(formats_of(&d, "fbx"), [ImageFormat::Png, ImageFormat::Png], "the fbx has PNG for both");
        // No glb or gltf asked for: `keep` has nothing to keep it in.
        let (with, without) = (ask(&["fbx", "usdz"], keep), ask(&["fbx", "usdz"], None));
        for format in ["fbx", "usdz"] {
            let bytes = |d: &Delivered| meshes(d).into_iter().find(|f| f.format == format).unwrap().bytes.clone();
            assert_eq!(bytes(&with), bytes(&without), "{format}");
        }
    }

    /// A scene whose other images already fill the 64 MiB texture budget
    /// leaves its KTX2 out of the containers that need PNG, with the budget
    /// as the reason; the glb that keeps it carries it, and says nothing.
    #[test]
    fn a_ktx2_past_the_texture_budget_is_noted_where_it_is_left_out() {
        use crate::mesh::scene::{Image, ImageFormat, Texture, TextureRef};
        let mut scene = scene_of(BASISU, "glb");
        // A stored PNG of exactly the budget: only its signature is read.
        let mut filler = vec![0u8; normalize::TEXTURE_BUDGET];
        filler[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        scene.images.push(Image { name: Some("filler".into()), format: ImageFormat::Png, data: filler });
        scene.textures.push(Texture { name: None, image: scene.images.len() - 1, sampler: Default::default() });
        scene.materials[0].emissive_texture = Some(TextureRef { texture: scene.textures.len() - 1, uv_set: 0, transform: None });
        let vendor = crate::mesh::glb::write_scene(&scene).unwrap();
        let seen = scene_of(&vendor, "glb");
        let (ix, ktx2) = seen.images.iter().enumerate().find(|(_, i)| i.format == ImageFormat::Ktx2).expect("the KTX2 survives the glb");
        let note = format!("texture {} (KTX2) was left out: {}", ktx2.name.clone().unwrap_or_else(|| format!("image {ix}")), normalize::OVER_BUDGET);
        let plan = DeliveryPlan {
            requested_formats: vec!["glb".into(), "obj".into()],
            ktx2_textures: Some(crate::types::Model3dKtx2Textures::Keep),
            ..Default::default()
        };
        let d = deliver(vec![mesh("glb", vendor)], &plan);
        assert!(d.fatal.is_none(), "{:?}", d.fatal);
        let notes = |format: &str| meshes(&d).into_iter().find(|f| f.format == format).unwrap().manifest.as_ref().unwrap().notes.clone();
        assert!(notes("obj").contains(&note), "{:?}", notes("obj"));
        assert!(notes("glb").iter().all(|n| !n.contains("left out")), "kept, so not left out: {:?}", notes("glb"));
        assert!(d.failures.iter().any(|(_, why)| *why == note), "{:?}", d.failures);
        let glb = scene_of(&meshes(&d).into_iter().find(|f| f.format == "glb").unwrap().bytes, "glb");
        assert!(glb.images.iter().any(|i| i.format == ImageFormat::Ktx2), "the glb carries the KTX2");
    }

    /// Review Focus 1: a KTX2 map litegen cannot decode is dropped from the
    /// scene before any package is written, so no package names a file its
    /// zip lacks; a WebP map arrives as PNG.
    #[test]
    fn a_dropped_texture_leaves_no_dangling_reference() {
        use crate::mesh::scene::{Image, ImageFormat, Texture, TextureRef};
        let mut scene = crate::mesh::fixtures::textured();
        let png = scene.images[0].data.clone();
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
            .encode(decoded.as_bytes(), decoded.width(), decoded.height(), decoded.color().into())
            .unwrap();
        let mut ktx2 = vec![0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB, b'\r', b'\n', 0x1A, b'\n'];
        ktx2.resize(80, 0);
        scene.images = vec![
            Image { name: Some("webp_map".into()), format: ImageFormat::Webp, data: webp },
            Image { name: Some("png_map".into()), format: ImageFormat::Png, data: png },
            Image { name: Some("basis_map".into()), format: ImageFormat::Ktx2, data: ktx2 },
        ];
        scene.textures = (0..3).map(|image| Texture { name: None, image, sampler: Default::default() }).collect();
        for (material, texture) in [(0, 0), (2, 1), (1, 2)] {
            scene.materials[material].base_color_texture = Some(TextureRef { texture, uv_set: 0, transform: None });
        }
        let vendor = crate::mesh::glb::write_scene(&scene).unwrap();
        let d = deliver(vec![mesh("glb", vendor)], &packaged(&PACKAGED));
        assert!(d.fatal.is_none(), "{:?}", d.fatal);
        assert_eq!(packages(&d).len(), PACKAGED.len(), "{:?}", d.files.iter().map(|f| (f.kind, &f.format)).collect::<Vec<_>>());
        for p in packages(&d) {
            let files = unzipped(p);
            let asked = Asked { files: &files, asked: Default::default() };
            let back = crate::mesh::read_scene(&files[0].bytes, MeshFormat::parse(&p.format).unwrap(), &asked).unwrap().scene;
            let missing: Vec<String> = asked.asked.borrow().iter().filter(|a| !files.iter().any(|f| &f.path == *a)).cloned().collect();
            assert!(missing.is_empty(), "{}: names files its zip lacks: {missing:?}", p.format);
            let bound: Vec<ImageFormat> = back
                .materials
                .iter()
                .filter_map(|m| m.base_color_texture.as_ref())
                .map(|t| back.images[back.textures[t.texture].image].format)
                .collect();
            assert_eq!(bound, [ImageFormat::Png, ImageFormat::Png], "{}: the WebP and PNG maps, both PNG", p.format);
            let notes = &p.manifest.as_ref().unwrap().notes;
            assert!(notes.iter().any(|n| n.contains("could not be decoded")), "{}: {notes:?}", p.format);
        }
    }

    /// Review Focus 2: maps named alike, exactly or but for case, are distinct
    /// entries of every package's zip.
    #[test]
    fn same_named_textures_get_distinct_entries() {
        use crate::mesh::scene::{Image, ImageFormat, Texture, TextureRef};
        let shade = |v: u8| crate::mesh::fixtures::encode_png(2, 2, &[v; 16]);
        let mut scene = crate::mesh::fixtures::textured();
        scene.images = [("Wood", 40u8), ("wood", 200)]
            .into_iter()
            .map(|(n, v)| Image { name: Some(n.into()), format: ImageFormat::Png, data: shade(v) })
            .collect();
        scene.textures = (0..2).map(|image| Texture { name: None, image, sampler: Default::default() }).collect();
        for (material, texture) in [(0, 0), (2, 1)] {
            scene.materials[material].base_color_texture = Some(TextureRef { texture, uv_set: 0, transform: None });
        }
        let vendor = crate::mesh::glb::write_scene(&scene).unwrap();
        let d = deliver(vec![mesh("glb", vendor)], &packaged(&PACKAGED));
        assert_eq!(packages(&d).len(), PACKAGED.len(), "every package zipped: {:?}", d.failures);
        for p in packages(&d) {
            let names: Vec<String> = unzipped(p).into_iter().map(|f| f.path).collect();
            let folded: std::collections::HashSet<String> = names.iter().map(|n| n.to_ascii_lowercase()).collect();
            assert_eq!(folded.len(), names.len(), "{}: {names:?}", p.format);
            assert_eq!(names.iter().filter(|n| n.ends_with(".png")).count(), 2, "{}: {names:?}", p.format);
        }
    }
}
