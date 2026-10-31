//! Tripo (VAST AI) images: image-to-multiview (PixelGenie 3D doc §3E).
//!
//! ```text
//! POST {base}/files                           multipart `file` → data.file_token
//! POST {base}/generation/image-to-multiview   {input} → data.task_id
//! GET  {base}/tasks/{task_id}                 output.generate_multiview_image.{front,left,back,right}_view_url
//! ```
//!
//! The same vendor, key and client as the 3D adapter (`model3d::tripo`).
//!
//! # References (read 2026-10-30)
//!
//! - Image to multiview — Verbatim: "On success, this object contains URLs
//!   for available views, such as front_view_url, left_view_url,
//!   back_view_url and right_view_url.":
//!   <https://developers.tripo3d.ai/en/docs/generation-image-to-multiview>
//! - Price — "Image to Multiview / 10" credits; "1 credit = $0.01 USD":
//!   <https://developers.tripo3d.ai/en/pricing>

use async_trait::async_trait;
use reqwest::Method;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::capabilities::ModelSchema;
use crate::providers::model3d::download::AssetError;
use crate::providers::model3d::tripo::TripoModel3dProvider;
use crate::providers::{
    build_cost_estimate, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras, ImageProvider,
    ProviderError, ProviderInstanceConfig,
};
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::types::*;

/// The litegen models served here.
pub const MODELS: &[&str] = &["tripo/multiview"];

/// How long a call waits for its task: well inside the router's 120 s
/// attempt, since an image call answers with the images themselves.
const TASK_BUDGET: Duration = Duration::from_secs(100);

/// Tripo's result field for each view it makes.
const VIEW_FIELDS: [(&str, &str); 4] = [
    ("front", "front_view_url"),
    ("left", "left_view_url"),
    ("back", "back_view_url"),
    ("right", "right_view_url"),
];

/// The extension Tripo's upload names for an image: "Formats: `PNG`, `JPEG`,
/// `WebP`". Anything else is refused before any call.
fn upload_ext(bytes: &[u8]) -> Result<&'static str, ProviderError> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok("png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Ok("jpg")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Ok("webp")
    } else {
        Err(ProviderError::InvalidRequest("Tripo takes a PNG, JPEG or WebP init image".into()))
    }
}

pub struct TripoImageProvider {
    inner: TripoModel3dProvider,
}

impl Default for TripoImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl TripoImageProvider {
    pub fn new() -> Self {
        Self { inner: TripoModel3dProvider::new() }
    }

    fn failed(message: String) -> ProviderError {
        // Accepted by Tripo already: never re-sent.
        ProviderError::RequestFailed { message, status_code: None, provider_error: None, retryable: false }
    }
}

#[async_trait]
impl ImageProvider for TripoImageProvider {
    fn name(&self) -> &str {
        "tripo"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        crate::providers::Model3dProvider::configure(&mut self.inner, config);
    }

    fn is_configured(&self) -> bool {
        crate::providers::Model3dProvider::is_configured(&self.inner)
    }

    /// Upload the init image, ask for its views, wait for the task and
    /// download the views the request named, in its order.
    async fn generate(
        &self,
        model: &ModelSchema,
        _base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        if !MODELS.contains(&model.id.as_str()) {
            return Err(ProviderError::ModelNotSupported { model: model.id.clone(), provider: "tripo".into() });
        }
        let init = materialized
            .refs
            .iter()
            .find(|r| r.role == "init")
            .ok_or_else(|| ProviderError::InvalidRequest(format!("'{}' turns an init image into views; send one", model.id)))?;
        let input = match &init.form {
            MaterializedRefForm::Url(u) => u.clone(),
            MaterializedRefForm::Base64(b) => {
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b)
                    .map_err(|e| ProviderError::InvalidRequest(format!("the init image is not base64: {e}")))?;
                self.inner.upload_bytes(&bytes, upload_ext(&bytes)?).await?
            }
            MaterializedRefForm::MultipartField { bytes, .. } => self.inner.upload_bytes(bytes, upload_ext(bytes)?).await?,
        };
        let created = self
            .inner
            .call(self.inner.request(Method::POST, "generation/image-to-multiview")?.json(&json!({ "input": input })), "image-to-multiview", true)
            .await?;
        let task = TripoModel3dProvider::task_id(&created, "image-to-multiview")?;
        // Accepted, so billable: the router must never send it again.
        extras.vendor_job.record(task.clone());
        let started = Instant::now();
        let output = loop {
            let t = self.inner.call(self.inner.request(Method::GET, &format!("tasks/{task}"))?, "multiview poll", false).await;
            let t = match t {
                Ok(t) => t,
                // The job is accepted: a poll that fails is retried within the
                // budget, never the submit.
                Err(e) if e.is_retryable() && started.elapsed() < TASK_BUDGET => {
                    tokio::time::sleep(self.inner.poll_interval()).await;
                    continue;
                }
                Err(e) => return Err(Self::failed(format!("tripo multiview task {task} could not be read: {e}"))),
            };
            match t["status"].as_str().unwrap_or("") {
                "success" => break t["output"]["generate_multiview_image"].clone(),
                "failed" | "cancelled" | "banned" | "expired" => {
                    let reason = t["error_message"].as_str().unwrap_or("no reason given");
                    return Err(Self::failed(format!("tripo multiview task {task} {}: {reason}", t["status"].as_str().unwrap_or(""))));
                }
                _ if started.elapsed() >= TASK_BUDGET => {
                    return Err(Self::failed(format!(
                        "tripo multiview task {task} did not finish within {}s; it may still finish, and bill, at Tripo",
                        TASK_BUDGET.as_secs()
                    )));
                }
                _ => tokio::time::sleep(self.inner.poll_interval()).await,
            }
        };
        let views: Vec<String> = if extras.views.is_empty() {
            VIEW_FIELDS.iter().map(|(v, _)| v.to_string()).collect()
        } else {
            extras.views.clone()
        };
        let mut images = Vec::with_capacity(views.len());
        for view in &views {
            let field = VIEW_FIELDS
                .iter()
                .find(|(v, _)| v == view)
                .map(|(_, f)| *f)
                .ok_or_else(|| ProviderError::InvalidRequest(format!("Tripo makes no '{view}' view")))?;
            let url = output[field].as_str().filter(|u| !u.is_empty()).ok_or_else(|| Self::failed(format!("tripo returned no {field}")))?;
            // Tripo has billed the task: a lost view fails the call, never a resubmit.
            let (data, ct) = self.inner.fetch(url).await.map_err(|e| {
                let why = match e {
                    AssetError::Transient(e) => e.to_string(),
                    AssetError::Gone(why) => why,
                };
                Self::failed(format!("the {view} view could not be downloaded: {why}"))
            })?;
            let mut metadata = HashMap::new();
            metadata.insert("view".to_string(), json!(view));
            if let Some(yaw) = ImageView::parse(view).and_then(|v| v.yaw_deg()) {
                metadata.insert("yaw_deg".to_string(), json!(yaw));
            }
            metadata.insert("task_id".to_string(), Value::String(task.clone()));
            let content_type = if ct.starts_with("image/") { ct } else { "image/png".to_string() };
            images.push(GenerationOutput { data, content_type, metadata, more: Vec::new() });
        }
        let mut first = images.remove(0);
        first.more = images;
        Ok(first)
    }

    async fn estimate_cost(&self, model: &ModelSchema, _request: &ImageGenerationRequest) -> Result<CostEstimate, ProviderError> {
        Ok(build_cost_estimate(model.pricing.base_cost_usd, 0.0, CostSource::Estimated, None))
    }

    async fn health_check(&self) -> HealthCheckResult {
        crate::providers::Model3dProvider::health_check(&self.inner).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::{Cleanup, MaterializedRef};
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ref_schema(id: &str) -> ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        crate::capabilities::CapabilityRegistry::from_dir(&p).unwrap().get(id).unwrap().clone()
    }

    fn provider(server: &MockServer) -> TripoImageProvider {
        let mut p = TripoImageProvider::new();
        p.configure(ProviderInstanceConfig {
            api_keys: vec![ApiKeyEntry { key: "tsk_pooled".into(), weight: 1, label: None }],
            api_base: Some(format!("{}/v3", server.uri())),
            ..Default::default()
        });
        p.inner.on_loopback_for_tests();
        p
    }

    fn extras(views: &[&str]) -> ImageExtras {
        ImageExtras {
            size: None, aspect_ratio: None, quality: None, style: None, steps: None, guidance_scale: None, strength: None,
            response_format: "b64_json".into(), extra: None, edit: None,
            views: views.iter().map(|v| v.to_string()).collect(),
            vendor_job: Default::default(),
            output_format: None, background: None,
            expand: None,
            layout: None, mask_prompt: None,
            preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
        }
    }

    fn init() -> MaterializedRequest {
        use base64::Engine;
        MaterializedRequest {
            refs: vec![MaterializedRef { role: "init".into(), form: MaterializedRefForm::Base64(base64::engine::general_purpose::STANDARD.encode(crate::providers::image::visual_mock::generate_visual_image_png("a knight"))), time_s: None }],
            cleanup: Cleanup::empty(),
        }
    }

    fn base() -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: "a knight".into(), model: "tripo/multiview".into(), n: 1, negative_prompt: None, seed: None,
            reference_images: vec![], strict: true, extra: None, metadata: None,
        }
    }

    async fn mount(server: &MockServer, status: Value) {
        Mock::given(method("POST")).and(path("/v3/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "data": { "file_token": "file_1" } })))
            .mount(server).await;
        Mock::given(method("POST")).and(path("/v3/generation/image-to-multiview")).and(body_json(json!({ "input": "file_1" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "data": { "task_id": "mv-1" } })))
            .mount(server).await;
        Mock::given(method("GET")).and(path("/v3/tasks/mv-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "code": 0, "data": status })))
            .mount(server).await;
        for v in ["front", "left", "back", "right"] {
            Mock::given(method("GET")).and(path(format!("/cdn/{v}.png")))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(format!("{v}-bytes").into_bytes()).insert_header("content-type", "image/png"))
                .mount(server).await;
        }
    }

    fn success(server: &MockServer) -> Value {
        let u = |v: &str| format!("{}/cdn/{v}.png", server.uri());
        json!({ "task_id": "mv-1", "type": "image_to_multiview", "status": "success", "output": { "generate_multiview_image": {
            "front_view_url": u("front"), "left_view_url": u("left"), "back_view_url": u("back"), "right_view_url": u("right"),
        } } })
    }

    #[tokio::test]
    async fn one_upload_one_task_and_one_image_per_asked_view_in_order() {
        let server = MockServer::start().await;
        mount(&server, success(&server)).await;
        let out = provider(&server).generate(&ref_schema("tripo/multiview"), &base(), &extras(&["back", "front", "left"]), &init()).await.unwrap();
        let all: Vec<(String, f64, Vec<u8>)> = std::iter::once(&out).chain(out.more.iter())
            .map(|o| (o.metadata["view"].as_str().unwrap().to_string(), o.metadata["yaw_deg"].as_f64().unwrap(), o.data.clone()))
            .collect();
        assert_eq!(all, [
            ("back".to_string(), 180.0, b"back-bytes".to_vec()),
            ("front".to_string(), 0.0, b"front-bytes".to_vec()),
            ("left".to_string(), 90.0, b"left-bytes".to_vec()),
        ]);
    }

    #[tokio::test]
    async fn a_failed_task_reports_tripos_reason_and_is_never_resent() {
        let server = MockServer::start().await;
        mount(&server, json!({ "task_id": "mv-1", "status": "failed", "error_message": "content policy" })).await;
        let x = extras(&["front"]);
        let err = provider(&server).generate(&ref_schema("tripo/multiview"), &base(), &x, &init()).await.unwrap_err();
        assert!(!err.is_retryable() && err.to_string().contains("content policy"), "{err}");
        // What stops the router resubmitting after a timeout of its own.
        assert_eq!(x.vendor_job.get().as_deref(), Some("mv-1"), "the accepted task is recorded");
    }

    #[tokio::test]
    async fn an_image_tripo_cannot_read_is_refused_before_any_call() {
        let server = MockServer::start().await;
        use base64::Engine;
        let gif = MaterializedRequest {
            refs: vec![MaterializedRef { role: "init".into(), form: MaterializedRefForm::Base64(base64::engine::general_purpose::STANDARD.encode(b"GIF89a....")), time_s: None }],
            cleanup: Cleanup::empty(),
        };
        let err = provider(&server).generate(&ref_schema("tripo/multiview"), &base(), &extras(&["front"]), &gif).await.unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err}");
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing reached Tripo");
    }

    #[test]
    fn every_tripo_image_row_is_served_here() {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&p).unwrap();
        let rows: Vec<String> = catalog.for_provider("tripo").filter(|m| m.media_type == crate::capabilities::MediaType::Image).map(|m| m.id.clone()).collect();
        assert_eq!(rows, MODELS);
    }
}
