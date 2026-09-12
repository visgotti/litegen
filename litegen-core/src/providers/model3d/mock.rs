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
/// was asked for. Real vendors under-deliver silently (a convert step that was
/// skipped, a format that quietly fell back); this is the subject that proves
/// the delivered-vs-requested guard catches it at all three observers.
const PARTIAL_MODEL: &str = "mock/partial-3d";

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
        let meshes: Vec<(String, Vec<u8>)> = requested
            .iter()
            .filter(|f| model.id != PARTIAL_MODEL || f.as_str() == "glb")
            .filter_map(|f| match f.as_str() {
                "glb" => Some((f.clone(), generate_cube_glb(&base.prompt))),
                "obj" => Some((f.clone(), generate_cube_obj(&base.prompt))),
                "stl" => Some((f.clone(), generate_cube_stl(&base.prompt))),
                _ => None,
            })
            .collect();

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
        });

        Ok(Model3dGenerationPollResult {
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
            display_name: id.into(),
            description: String::new(),
            pricing: ModelPricing { base_cost_usd: 0.0, variable_pricing: None },
            capabilities: ModelCapabilityFlags { text_to_3d: true, ..Default::default() },
            prompt: PromptSpec { required: true, min_length: None, max_length: None },
            params: HashMap::new(),
            ref_inputs: None,
            extra_allowlist: vec![],
            tags: vec![],
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
