use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::capabilities::ModelSchema;
use crate::providers::model3d::cube::{generate_cube_obj, generate_cube_stl};
use crate::providers::model3d::glb::{generate_cube_glb, CUBE_TRIANGLE_COUNT};
use crate::providers::image::visual_mock::generate_visual_image_png;
use crate::proxy::materializer::MaterializedRequest;
use crate::providers::{
    build_cost_estimate, BaseGenerationRequest, HealthCheckResult, Model3dExtras, Model3dFile,
    Model3dGenerationHandle, Model3dGenerationPollResult, Model3dProvider, ProviderError,
    ProviderInstanceConfig,
};
use crate::types::*;

/// Polls before the job reports `completed`. Three is enough for a client to
/// observe a real pending → processing → completed transition without making
/// tests slow.
const POLLS_BEFORE_DONE: u32 = 3;

/// The model id that terminates in `failed`, for exercising refund paths.
const FAIL_MODEL: &str = "mock/fail-3d";

/// How long a FINISHED job stays answerable before it is swept. A real vendor
/// keeps a completed job queryable for hours; the mock only needs to outlive
/// every observer of one generation (the client's poll loop and the 5s poller),
/// with room to spare for a paused debugger.
const FINISHED_JOB_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// The model id that reports `completed` while delivering only glb, whatever
/// was asked for, and a glb the converter cannot read
/// ([`generate_undecodable_glb`]), so nothing missing can be derived from it.
/// Real vendors under-deliver silently (a convert step that was skipped, a
/// format that quietly fell back); this is the subject that proves the
/// delivered-vs-requested guard catches it at all three observers.
const PARTIAL_MODEL: &str = "mock/partial-3d";

/// The rigging operation: any source becomes the skinned box humanoid.
const RIG_MODEL: &str = "mock/rig-3d";

/// Emits the humanoid facing +X, as a vendor building +X-forward does; its
/// catalog row declares so, and litegen turns it to face +Z.
const TURNED_MODEL: &str = "mock/turned-3d";

/// The animation operation: the rigged source plus one clip per request entry.
const ANIMATE_MODEL: &str = "mock/animate-3d";

/// The rigged source plus one LINEAR clip per requested clip, in order,
/// named `mock_clip_<i>` (litegen names them as asked). Over each clip the
/// skeleton's root rises and falls 5 cm and travels 1 m along +Z — or holds
/// still horizontally when EVERY clip asks to be in place, the way a vendor
/// with one in-place flag (Tripo's `animate_in_place`) honours it. A preset
/// clip lasts 1 s, a prompt clip its `duration_s`; keys at `fps`. Any clips
/// the source carried are replaced.
fn animate_source(
    source: &crate::providers::SourceInput,
    clips: &[crate::providers::AnimationClip],
    fps: u32,
) -> Result<Vec<u8>, ProviderError> {
    use crate::mesh::scene::{AnimPath, Animation, Channel, Interpolation, Transform};
    let format = crate::mesh::MeshFormat::parse(&source.format)
        .ok_or_else(|| ProviderError::InvalidRequest(format!("mock/animate-3d cannot read '{}'", source.format)))?;
    let mut scene = crate::mesh::read_scene(&source.bytes, format, &crate::mesh::package::NoSidecars)
        .map_err(|e| ProviderError::InvalidRequest(format!("mock/animate-3d cannot read the source: {e}")))?
        .scene;
    let skin = scene.skins.first().ok_or_else(|| ProviderError::InvalidRequest("mock/animate-3d needs a rigged source".into()))?;
    let root = skin.skeleton.or_else(|| skin.joints.first().copied()).ok_or_else(|| ProviderError::InvalidRequest("the source's skin has no joints".into()))?;
    let base = match &scene.nodes[root].transform {
        Transform::Trs { translation, .. } => *translation,
        Transform::Matrix(m) => [m[12], m[13], m[14]],
    };
    // Travel is 1 m in the world: divided by the uniform scale above the root.
    let mut parent_of = vec![None; scene.nodes.len()];
    for (i, n) in scene.nodes.iter().enumerate() {
        for &c in &n.children {
            parent_of[c] = Some(i);
        }
    }
    let mut scale_above = 1.0f64;
    let mut at = parent_of[root];
    while let Some(p) = at {
        if let Transform::Trs { scale, .. } = &scene.nodes[p].transform {
            scale_above *= scale[0];
        }
        at = parent_of[p];
    }
    let travel = if clips.iter().all(|c| c.in_place == Some(true)) { 0.0 } else { 1.0 / scale_above };
    let fps = fps.max(1);
    scene.animations = clips
        .iter()
        .enumerate()
        .map(|(i, clip)| {
            let duration = clip.duration_s.unwrap_or(1.0);
            let keys = ((duration * fps as f64).round() as usize).max(1);
            let times: Vec<f32> = (0..=keys).map(|k| (k as f64 / fps as f64) as f32).collect();
            let values: Vec<f32> = times
                .iter()
                .flat_map(|&t| {
                    let u = t as f64 / duration;
                    [
                        base[0] as f32,
                        (base[1] + 0.05 / scale_above * (std::f64::consts::TAU * u).sin()) as f32,
                        (base[2] + travel * u) as f32,
                    ]
                })
                .collect();
            Animation {
                name: Some(format!("mock_clip_{i}")),
                channels: vec![Channel { node: root, path: AnimPath::Translation, interpolation: Interpolation::Linear, times, values }],
            }
        })
        .collect();
    crate::mesh::glb::write_scene(&scene).map_err(|e| ProviderError::InvalidRequest(format!("mock/animate-3d could not write its result: {e}")))
}

struct Job {
    model: String,
    /// One entry per container the caller was promised, in request order.
    meshes: Vec<(String, Vec<u8>)>,
    preview: Vec<u8>,
    polls: AtomicU32,
    /// When this job first reported a terminal status. Set once; read by the
    /// sweep in `generate`. `None` means still in flight.
    finished_at: std::sync::OnceLock<std::time::Instant>,
}

/// Process-global job store. The mock has no backing service, so submitted work
/// lives here between `generate` and `poll_status` — the same technique
/// `video::visual_mock::global_store` uses for its GIF bytes.
fn jobs() -> &'static tokio::sync::RwLock<HashMap<String, Arc<Job>>> {
    static JOBS: std::sync::OnceLock<tokio::sync::RwLock<HashMap<String, Arc<Job>>>> =
        std::sync::OnceLock::new();
    JOBS.get_or_init(|| tokio::sync::RwLock::new(HashMap::new()))
}

/// Mock 3D provider for tests, CI, and the Playground.
///
/// No external API is called: `generate` renders a real glTF 2.0 cube locally
/// and `poll_status` walks a short progress ramp before handing back the bytes.
/// The only external contract honoured is the GLB byte format.
///
/// @see <https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html>
pub struct MockModel3dProvider {
    configured: bool,
}

impl MockModel3dProvider {
    pub fn new() -> Self {
        Self { configured: false }
    }
}

impl Default for MockModel3dProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// The cube as a GLB litegen's converter refuses: it declares Draco
/// compression as required, as a vendor that compresses its meshes does, and
/// `mesh` decodes no Draco. Nothing can be derived from it, so whatever
/// container `mock/partial-3d` leaves out stays missing — the one way a
/// promised container still goes missing now that every container litegen
/// names is derivable from a mesh it can read.
pub(crate) fn generate_undecodable_glb(prompt: &str) -> Vec<u8> {
    let glb = generate_cube_glb(prompt);
    let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
    let mut json: serde_json::Value = serde_json::from_slice(&glb[20..20 + json_len]).expect("the mock's own glb");
    json["extensionsUsed"] = serde_json::json!(["KHR_draco_mesh_compression"]);
    json["extensionsRequired"] = serde_json::json!(["KHR_draco_mesh_compression"]);
    let mut text = serde_json::to_vec(&json).expect("json");
    while text.len() % 4 != 0 {
        text.push(b' ');
    }
    let bin = &glb[20 + json_len..];
    let total = 20 + text.len() + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&text);
    out.extend_from_slice(bin);
    out
}

#[async_trait]
impl Model3dProvider for MockModel3dProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn configure(&mut self, _config: ProviderInstanceConfig) {
        self.configured = true;
    }

    fn is_configured(&self) -> bool {
        self.configured
    }

    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &Model3dExtras,
        _materialized: &MaterializedRequest,
    ) -> Result<Model3dGenerationHandle, ProviderError> {
        let job_id = uuid::Uuid::new_v4().to_string();

        // Honour the requested containers rather than always answering glb.
        // A format the mock cannot write is simply not produced — which is what
        // makes `mock/all-params-3d` a real subject for the guard instead of a
        // model that advertises four containers and emits one.
        let requested = if extras.output_formats.is_empty() {
            vec![crate::types::DEFAULT_MODEL3D_FORMAT.to_string()]
        } else {
            extras.output_formats.clone()
        };
        if model.id == ANIMATE_MODEL {
            let source = extras.source.as_ref().ok_or_else(|| {
                ProviderError::InvalidRequest("mock/animate-3d needs a source_model".into())
            })?;
            source.record(match &source.vendor_task_id {
                Some(id) if jobs().read().await.contains_key(id) => crate::providers::SourceDelivery::VendorTask,
                _ => crate::providers::SourceDelivery::DataUri,
            });
            let animated = animate_source(source, &extras.animations, extras.fps.unwrap_or(30))?;
            let job = Arc::new(Job {
                model: model.id.clone(),
                meshes: vec![("glb".into(), animated)],
                preview: generate_visual_image_png("animated"),
                polls: AtomicU32::new(0),
                finished_at: std::sync::OnceLock::new(),
            });
            jobs().write().await.insert(job_id.clone(), job);
            return Ok(Model3dGenerationHandle {
                provider_job_id: job_id,
                provider: "mock".to_string(),
                model: model.id.clone(),
                stage_context: None,
            });
        }
        if model.id == RIG_MODEL {
            // Any source litegen let through becomes the same rig. Chained by
            // the mock's own task id when the source is one of its jobs, as a
            // vendor that keeps its own outputs chains them; inline otherwise.
            let source = extras.source.as_ref().ok_or_else(|| {
                ProviderError::InvalidRequest("mock/rig-3d needs a source_model".into())
            })?;
            source.record(match &source.vendor_task_id {
                Some(id) if jobs().read().await.contains_key(id) => crate::providers::SourceDelivery::VendorTask,
                _ => crate::providers::SourceDelivery::DataUri,
            });
            // The vendor's own naming setting, as Tripo's `spec` is.
            use crate::providers::model3d::humanoid::{rigged_humanoid_glb_sized, MOCK_JOINTS, VRM_JOINTS};
            let names = match extras.joint_naming.as_deref() {
                Some("vrm") => &VRM_JOINTS,
                _ => &MOCK_JOINTS,
            };
            let rig = rigged_humanoid_glb_sized(names, extras.height_m.unwrap_or(1.75));
            let job = Arc::new(Job {
                model: model.id.clone(),
                meshes: vec![("glb".into(), rig)],
                preview: generate_visual_image_png("rigged humanoid"),
                polls: AtomicU32::new(0),
                finished_at: std::sync::OnceLock::new(),
            });
            jobs().write().await.insert(job_id.clone(), job);
            return Ok(Model3dGenerationHandle {
                provider_job_id: job_id,
                provider: "mock".to_string(),
                model: model.id.clone(),
                stage_context: None,
            });
        }
        let meshes: Vec<(String, Vec<u8>)> = if model.id == TURNED_MODEL {
            vec![("glb".into(), crate::providers::model3d::humanoid::turned_humanoid_glb())]
        } else {
            requested
            .iter()
            .filter(|f| model.id != PARTIAL_MODEL || f.as_str() == "glb")
            .filter_map(|f| match f.as_str() {
                "glb" if model.id == PARTIAL_MODEL => Some((f.clone(), generate_undecodable_glb(&base.prompt))),
                "glb" => Some((f.clone(), generate_cube_glb(&base.prompt))),
                "obj" => Some((f.clone(), generate_cube_obj(&base.prompt))),
                "stl" => Some((f.clone(), generate_cube_stl(&base.prompt))),
                _ => None,
            })
            .collect()
        };

        // Bytes are rendered at submit time so the poll ramp measures polling,
        // not generation, and so a prompt that reached us is provable later.
        let job = Arc::new(Job {
            model: model.id.clone(),
            meshes,
            preview: generate_visual_image_png(&base.prompt),
            polls: AtomicU32::new(0),
            finished_at: std::sync::OnceLock::new(),
        });
        let mut jobs = jobs().write().await;
        // Sweep long-finished jobs here rather than on the poll that finishes
        // them — see `poll_status`, which must keep answering terminally.
        jobs.retain(|_, j| {
            !matches!(j.finished_at.get(), Some(t) if t.elapsed() >= FINISHED_JOB_TTL)
        });
        jobs.insert(job_id.clone(), job);
        drop(jobs);
        Ok(Model3dGenerationHandle {
            provider_job_id: job_id,
            provider: "mock".to_string(),
            model: model.id.clone(),
            stage_context: None,
        })
    }

    /// `mock/rig-3d`'s rig-check: a flat source (thinnest side under 1% of
    /// its longest) has no volume to rig.
    async fn preflight(&self, model: &ModelSchema, extras: &Model3dExtras) -> Result<crate::providers::Preflight, ProviderError> {
        let Some(source) = extras.source.as_ref().filter(|_| model.id == RIG_MODEL) else {
            return Ok(crate::providers::Preflight::Proceed { context: None });
        };
        let format = crate::mesh::MeshFormat::parse(&source.format)
            .ok_or_else(|| ProviderError::InvalidRequest(format!("mock rig-check cannot read '{}'", source.format)))?;
        let scene = crate::mesh::read_scene(&source.bytes, format, &crate::mesh::package::NoSidecars)
            .map_err(|e| ProviderError::InvalidRequest(format!("mock rig-check cannot read the source: {e}")))?
            .scene;
        let bounds = crate::mesh::inspect::inspect(&scene)
            .map_err(|e| ProviderError::InvalidRequest(format!("mock rig-check cannot measure the source: {e}")))?
            .bounds;
        let extents = bounds.map(|(lo, hi)| [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]]).unwrap_or([0.0; 3]);
        let longest = extents.iter().cloned().fold(0.0f32, f32::max);
        let thinnest = extents.iter().cloned().fold(f32::INFINITY, f32::min);
        if longest <= 0.0 || thinnest < longest * 0.01 {
            return Ok(crate::providers::Preflight::Refuse {
                code: "not_riggable",
                message: format!("the vendor's rig-check found nothing to rig: the source is flat (extents {extents:?} m)"),
            });
        }
        Ok(crate::providers::Preflight::Proceed { context: None })
    }

    async fn poll_status(
        &self,
        handle: &Model3dGenerationHandle,
    ) -> Result<Model3dGenerationPollResult, ProviderError> {
        let job = jobs()
            .read()
            .await
            .get(&handle.provider_job_id)
            .cloned()
            .ok_or_else(|| {
                // Not retryable: a job we have no record of will never appear.
                ProviderError::InvalidRequest(format!(
                    "unknown mock 3d job '{}'",
                    handle.provider_job_id
                ))
            })?;

        let n = job.polls.fetch_add(1, Ordering::SeqCst) + 1;
        let meta = HashMap::from([
            ("mock".to_string(), serde_json::json!(true)),
            ("job_id".to_string(), serde_json::json!(handle.provider_job_id)),
        ]);

        if n < POLLS_BEFORE_DONE {
            return Ok(Model3dGenerationPollResult {
                stage_context: None,
                next_stage: None,
                status: if n == 1 { GenerationStatus::Pending } else { GenerationStatus::Processing },
                progress: ((n * 100) / POLLS_BEFORE_DONE) as u8,
                files: Vec::new(),
                error: None,
                metadata: meta,
            });
        }

        // Terminal — but the job is NOT forgotten. A 3D generation has two
        // independent observers: the client's `GET /v1/models3d/{id}` and the
        // background poller, and the client usually gets here first (it polls
        // every 2s against the poller's 5s tick). A provider that dropped the
        // job on its first terminal poll answered the poller's next tick with a
        // NON-retryable "unknown job", which reaps the already-completed row —
        // and its request log — as `failed`. Real vendors answer a finished job
        // with the same terminal result for as long as they retain it; so does
        // this. `generate` sweeps jobs finished longer ago than FINISHED_JOB_TTL.
        let _ = job.finished_at.set(std::time::Instant::now());

        if job.model == FAIL_MODEL {
            return Ok(Model3dGenerationPollResult {
                stage_context: None,
                next_stage: None,
                status: GenerationStatus::Failed,
                progress: 100,
                files: Vec::new(),
                error: Some("mock 3d generation failed (mock/fail-3d always fails)".into()),
                metadata: meta,
            });
        }

        let mut files: Vec<Model3dFile> = job
            .meshes
            .iter()
            .map(|(format, bytes)| Model3dFile {
                kind: Model3dAssetKind::Mesh,
                format: format.clone(),
                content_type: crate::proxy::storage::model3d_content_type(
                    format,
                    "application/octet-stream",
                )
                .to_string(),
                bytes: bytes.clone(),
                polycount: Some(CUBE_TRIANGLE_COUNT),
                width: None,
                height: None,
                manifest: None,
            })
            .collect();
        files.push(Model3dFile {
            kind: Model3dAssetKind::Preview,
            format: "png".into(),
            content_type: "image/png".into(),
            bytes: job.preview.clone(),
            polycount: None,
            width: Some(512),
            height: Some(512),
            manifest: None,
        });

        Ok(Model3dGenerationPollResult {
            stage_context: None,
            next_stage: None,
            status: GenerationStatus::Completed,
            progress: 100,
            files,
            error: None,
            metadata: meta,
        })
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &Model3dGenerationRequest,
    ) -> Result<CostEstimate, ProviderError> {
        Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, None))
    }

    async fn health_check(&self) -> HealthCheckResult {
        HealthCheckResult {
            healthy: true,
            message: "Mock 3D provider always healthy".into(),
            latency_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::{MediaType, ModelCapabilityFlags, ModelPricing, PromptSpec};
    use std::collections::HashMap;

    fn schema(id: &str) -> ModelSchema {
        ModelSchema {
            id: id.into(),
            provider: "mock".into(),
            media_type: MediaType::Model3d,
            task: Default::default(),
            native_orientation: None,
            preserves_unmasked: None,
            display_name: id.into(),
            description: String::new(),
            docs_url: None,
            pricing: ModelPricing { base_cost_usd: 0.0, variable_pricing: Vec::new() },
            capabilities: ModelCapabilityFlags { text_to_3d: true, ..Default::default() },
            prompt: PromptSpec { required: true, min_length: None, max_length: None },
            params: HashMap::new(),
            ref_inputs: None,
            extra_allowlist: vec![],
            tags: vec![],
            deprecated_at: None, retires_at: None, replaced_by: None, status: Default::default(),
        }
    }

    fn base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.into(),
            model: model.into(),
            n: 1,
            negative_prompt: None,
            seed: None,
            reference_images: vec![],
            strict: true,
            extra: None,
            metadata: None,
        }
    }

    fn extras() -> Model3dExtras {
        Model3dExtras {
            output_formats: vec![crate::types::DEFAULT_MODEL3D_FORMAT.to_string()],
            texture: Some(true), pbr: None, target_polycount: None,
            symmetry: None, topology: None, rig: None, extra: None,
            source: None,
            animations: Vec::new(), fps: None,
            skeleton: None, joint_naming: None, height_m: None, preflight: None,
        }
    }

    fn provider() -> MockModel3dProvider {
        let mut p = MockModel3dProvider::new();
        p.configure(ProviderInstanceConfig::default());
        p
    }

    async fn poll_to_terminal(
        p: &MockModel3dProvider,
        h: &Model3dGenerationHandle,
    ) -> (Model3dGenerationPollResult, usize) {
        let mut polls = 0;
        loop {
            polls += 1;
            let r = p.poll_status(h).await.unwrap();
            if !matches!(r.status, GenerationStatus::Pending | GenerationStatus::Processing) {
                return (r, polls);
            }
            assert!(polls < 20, "mock must terminate quickly");
        }
    }

    #[tokio::test]
    async fn reports_monotonic_progress_across_several_polls() {
        // aipix's contract §6 asks the mock to actually exercise the polling
        // path, not snap straight to completed.
        let p = provider();
        let s = schema("mock/mesh-3d");
        let h = p.generate(&s, &base("a fox", "mock/mesh-3d"), &extras(), &MaterializedRequest::default())
            .await.unwrap();

        let mut seen = Vec::new();
        loop {
            let r = p.poll_status(&h).await.unwrap();
            seen.push(r.progress);
            if !matches!(r.status, GenerationStatus::Pending | GenerationStatus::Processing) { break; }
            assert!(seen.len() < 20);
        }
        assert!(seen.len() >= 3, "expected several polls, got {seen:?}");
        assert!(seen.windows(2).all(|w| w[1] >= w[0]), "progress must be monotonic: {seen:?}");
        assert_eq!(*seen.last().unwrap(), 100);
    }

    #[test]
    fn the_partial_models_glb_is_one_the_converter_refuses() {
        let glb = generate_undecodable_glb("a fox");
        assert_eq!(&glb[..4], b"glTF");
        assert_eq!(u32::from_le_bytes([glb[8], glb[9], glb[10], glb[11]]) as usize, glb.len());
        let err = crate::mesh::read(&glb, crate::mesh::MeshFormat::Glb).unwrap_err();
        assert!(matches!(err, crate::mesh::MeshError::Unsupported(_)), "{err}");
        assert!(crate::mesh::read(&generate_cube_glb("a fox"), crate::mesh::MeshFormat::Glb).is_ok());
    }

    #[tokio::test]
    async fn completed_poll_carries_exactly_one_mesh_with_valid_glb_bytes() {
        let p = provider();
        let s = schema("mock/mesh-3d");
        let h = p.generate(&s, &base("a fox", "mock/mesh-3d"), &extras(), &MaterializedRequest::default())
            .await.unwrap();
        let (r, _) = poll_to_terminal(&p, &h).await;

        assert_eq!(r.status, GenerationStatus::Completed);
        let meshes: Vec<_> = r.files.iter().filter(|f| f.kind == Model3dAssetKind::Mesh).collect();
        assert_eq!(meshes.len(), 1, "exactly one mesh, always");
        let mesh = meshes[0];
        assert_eq!(&mesh.bytes[0..4], b"glTF", "mesh bytes must be a real GLB");
        assert_eq!(mesh.format, "glb");
        assert_eq!(mesh.content_type, "model/gltf-binary");
        assert_eq!(mesh.polycount, Some(crate::providers::model3d::glb::CUBE_TRIANGLE_COUNT));

        // A preview image rides along so the gallery has a 2D thumb.
        let preview = r.files.iter().find(|f| f.kind == Model3dAssetKind::Preview).unwrap();
        assert_eq!(&preview.bytes[0..4], b"\x89PNG");
        assert_eq!(preview.format, "png");
    }

    #[tokio::test]
    async fn distinct_prompts_yield_distinct_mesh_bytes() {
        let p = provider();
        let s = schema("mock/mesh-3d");
        let mut out = Vec::new();
        for prompt in ["prompt one", "prompt two"] {
            let h = p.generate(&s, &base(prompt, "mock/mesh-3d"), &extras(), &MaterializedRequest::default())
                .await.unwrap();
            let (r, _) = poll_to_terminal(&p, &h).await;
            out.push(r.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh).unwrap().bytes.clone());
        }
        assert_ne!(out[0], out[1], "the prompt must reach the generator");
    }

    #[tokio::test]
    async fn fail_model_terminates_failed_with_an_error_string() {
        // aipix needs a deterministic failure to test per-slot refunds.
        let p = provider();
        let s = schema("mock/fail-3d");
        let h = p.generate(&s, &base("anything", "mock/fail-3d"), &extras(), &MaterializedRequest::default())
            .await.unwrap();
        let (r, _) = poll_to_terminal(&p, &h).await;
        assert_eq!(r.status, GenerationStatus::Failed);
        assert!(r.error.as_deref().is_some_and(|e| !e.is_empty()));
        assert!(r.files.is_empty(), "a failed generation must not carry assets");
    }

    #[tokio::test]
    async fn unknown_job_id_is_a_non_retryable_error() {
        let p = provider();
        let h = Model3dGenerationHandle {
            provider_job_id: "does-not-exist".into(),
            provider: "mock".into(),
            model: "mock/mesh-3d".into(),
            stage_context: None,
        };
        let err = p.poll_status(&h).await.unwrap_err();
        assert!(!err.is_retryable(), "a purged job cannot be recovered by retrying");
    }
}
