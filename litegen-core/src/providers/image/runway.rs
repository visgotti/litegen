use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::capabilities::ModelSchema;
use crate::proxy::materializer::{MaterializedRefForm, MaterializedRequest};
use crate::providers::auth::{AuthSpec, ProviderCredentials};
use crate::providers::{
    ApiKeyPool, BaseGenerationRequest, GenerationOutput, HealthCheckResult, ImageExtras,
    ImageProvider, ProviderError, ProviderInstanceConfig, build_cost_estimate,
};
use crate::types::*;

/// Runway Gen-4 image generation provider.
///
/// Shares the Runway developer API host (`api.dev.runwayml.com/v1`), Bearer auth,
/// and mandatory `X-Runway-Version` header with the Runway video provider.
/// Asynchronous: `POST /text_to_image` returns a task id; poll `GET /tasks/{id}`
/// until `SUCCEEDED`, then download the first `output` URL.
///
/// @see <https://docs.dev.runwayml.com/guides/using-the-api/> — text_to_image
///   Verbatim: "curl -X POST https://api.dev.runwayml.com/v1/text_to_image"
///   Verbatim: "\"promptText\": \"@EiffelTower painted in the style of @StarryNight\", \"model\": \"gen4_image\", \"ratio\": \"1920:1080\","
///   Verbatim: "-H \"Authorization: Bearer $RUNWAYML_API_SECRET\" -H \"X-Runway-Version: 2024-11-06\""
pub struct RunwayImageProvider {
    config: Option<ProviderInstanceConfig>,
    key_pool: Option<ApiKeyPool>,
    auth: AuthSpec,
    client: Client,
    /// Result downloads: no redirects, so a URL that passed the public-address
    /// check cannot bounce into a private one.
    asset_client: Client,
    /// Whether a result URL must resolve to a public address. Only the tests,
    /// whose mock vendor runs on loopback, turn it off.
    pub(crate) public_assets_only: bool,
}

const RUNWAY_VERSION: &str = "2024-11-06";

/// What litegen itself sends to `POST /text_to_image`; `extra` cannot replace it.
const OWNED_FIELDS: &[&str] = &["promptText", "model", "ratio", "seed", "referenceImages"];

/// The longest data URI Runway takes: `"maxLength": 5242880` ("to fit an
/// image within the 5MB limit, the binary file must be 3.3MB or less").
/// @see <https://docs.dev.runwayml.com/assets/inputs.md> (2026-10-30)
const MAX_DATA_URI_CHARS: usize = 5_242_880;

/// A reference as Runway takes it, for both the image and the video
/// adapter: a URL, or a data URI of a JPEG, PNG or WebP within
/// [`MAX_DATA_URI_CHARS`] ("Data URIs are accepted in any field where a URL is
/// accepted"; "GIF images are not supported"). Anything else is refused
/// before any call. `None` for a form Runway has no field for.
/// @see <https://docs.dev.runwayml.com/assets/inputs.md> (2026-10-30)
pub(crate) fn runway_uri(form: &MaterializedRefForm) -> Result<Option<String>, ProviderError> {
    let uri = match form {
        MaterializedRefForm::Url(u) => u.clone(),
        MaterializedRefForm::Base64(b64) => crate::proxy::materializer::base64_data_uri(b64, None),
        MaterializedRefForm::MultipartField { .. } => return Ok(None),
    };
    if let Some(rest) = uri.strip_prefix("data:") {
        let mime = rest.split([';', ',']).next().unwrap_or("");
        if !matches!(mime, "image/jpeg" | "image/jpg" | "image/png" | "image/webp") {
            return Err(ProviderError::InvalidRequest(format!(
                "runway takes a JPEG, PNG or WebP reference, not {}",
                if mime.is_empty() { "an untyped one" } else { mime }
            )));
        }
        if uri.len() > MAX_DATA_URI_CHARS {
            return Err(ProviderError::InvalidRequest(format!(
                "runway takes a reference of at most {MAX_DATA_URI_CHARS} characters as a data URI (3.3 MB of image); this one is {}",
                uri.len()
            )));
        }
    }
    Ok(Some(uri))
}

impl RunwayImageProvider {
    pub fn new() -> Self {
        Self {
            config: None,
            key_pool: None,
            auth: AuthSpec::bearer(),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .expect("failed to build HTTP client"),
            asset_client: crate::util::ssrf::no_redirect_download_client(
                std::time::Duration::from_secs(120),
                std::time::Duration::from_secs(30),
            ),
            public_assets_only: true,
        }
    }

    fn api_base(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.api_base.as_deref())
            .unwrap_or("https://api.dev.runwayml.com/v1")
    }

    fn creds(&self) -> Result<ProviderCredentials, ProviderError> {
        let base = self
            .config
            .as_ref()
            .map(|c| c.credentials.clone())
            .ok_or_else(|| ProviderError::NotConfigured("runway".into()))?;
        if let Some(pool) = &self.key_pool {
            return Ok(base.with_api_key(pool.next().to_string()));
        }
        Ok(base)
    }

    /// The Runway model for a litegen id; `None` for an id with no arm, which
    /// is refused rather than billed as another model (L13).
    fn resolve_model(model_id: &str) -> Option<&'static str> {
        match model_id.strip_prefix("runway/").unwrap_or(model_id) {
            "gen4_image_turbo" | "gen4-image-turbo" => Some("gen4_image_turbo"),
            "gen4_image" | "gen4-image" => Some("gen4_image"),
            _ => None,
        }
    }

    /// The exact `ratio` enum shared by `gen4_image` and `gen4_image_turbo` on
    /// `POST /v1/text_to_image`. Anything outside this set is a 400.
    /// @see <https://docs.dev.runwayml.com/openapi.json>
    pub(crate) const RATIOS: [&'static str; 16] = [
        "1024:1024", "1080:1080", "1168:880", "1360:768", "1440:1080", "1080:1440",
        "1808:768", "1920:1080", "1080:1920", "2112:912", "1280:720", "720:1280",
        "720:720", "960:720", "720:960", "1680:720",
    ];

    /// Map a unified size / aspect ratio to a Runway `ratio` (pixel WxH string).
    ///
    /// Every return value is a member of [`Self::RATIOS`]. A caller-supplied
    /// `size` is snapped to the closest enum member by aspect ratio rather than
    /// forwarded verbatim — `1000x800` is not a legal Runway ratio. Note there
    /// is no true 3:2 pair in the enum, which is why `3:2` / `2:3` are not in
    /// any Runway model's `allowed` list.
    fn resolve_ratio(extras: &ImageExtras) -> String {
        if let Some(size) = extras.size.as_deref() {
            if let Some((w, h)) = size.split_once('x') {
                if let (Ok(w), Ok(h)) = (w.parse::<f64>(), h.parse::<f64>()) {
                    if h > 0.0 {
                        return Self::closest_ratio(w / h).to_string();
                    }
                }
            }
        }
        match extras.aspect_ratio.as_deref() {
            Some("9:16") => "1080:1920",
            Some("1:1") => "1024:1024",
            Some("4:3") => "1440:1080",
            Some("3:4") => "1080:1440",
            Some("21:9") => "2112:912",
            Some("16:9") => "1920:1080",
            Some(other) => Self::closest_ratio(Self::parse_ratio(other).unwrap_or(16.0 / 9.0)),
            None => "1920:1080",
        }
        .to_string()
    }

    /// Parse a `"w:h"` or `"wxh"` ratio string into a numeric ratio.
    fn parse_ratio(s: &str) -> Option<f64> {
        let (a, b) = s.split_once(':').or_else(|| s.split_once('x'))?;
        let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
        if b == 0.0 { None } else { Some(a / b) }
    }

    /// The enum member whose aspect ratio is nearest `target`.
    fn closest_ratio(target: f64) -> &'static str {
        Self::RATIOS
            .iter()
            .copied()
            .min_by(|a, b| {
                let da = (Self::parse_ratio(a).unwrap_or(1.0) - target).abs();
                let db = (Self::parse_ratio(b).unwrap_or(1.0) - target).abs();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or("1920:1080")
    }

    async fn poll_task(&self, id: &str) -> Result<Value, ProviderError> {
        let creds = self.creds()?;
        let url = format!("{}/tasks/{}", self.api_base(), id);
        let max_attempts = 90; // 90 * 2s = 3 min
        for _ in 0..max_attempts {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let builder = crate::providers::auth::apply(
                &self.auth,
                &creds,
                self.client.get(&url).header("X-Runway-Version", RUNWAY_VERSION),
            )?;
            let resp = match crate::providers::inject_trace_headers(builder).send().await {
                Ok(resp) => resp,
                // The task is Runway's now; a dropped poll is asked again.
                Err(_) => continue,
            };
            // A 5xx or 429 is asked again; any other error status is the answer.
            let data = match crate::providers::video::read_poll_json(resp, "runway").await {
                Ok(data) => data,
                Err(ProviderError::RequestFailed { retryable: true, .. }) => continue,
                Err(e) => return Err(e),
            };
            match data["status"].as_str() {
                Some("SUCCEEDED") => return Ok(data),
                Some("FAILED") | Some("CANCELLED") => {
                    let err = data["failure"].as_str().or_else(|| data["error"].as_str()).unwrap_or("Unknown error").to_string();
                    return Err(ProviderError::RequestFailed {
                        message: format!("Runway task failed: {err}"),
                        status_code: None,
                        provider_error: Some(data),
                        retryable: false,
                    });
                }
                _ => continue,
            }
        }
        Err(ProviderError::Timeout { timeout_ms: max_attempts * 2000 })
    }

    async fn fetch_image_bytes(&self, url: &str) -> Result<Vec<u8>, ProviderError> {
        super::fetch_result(&self.asset_client, url, self.public_assets_only, "runway image").await
    }
}

impl Default for RunwayImageProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageProvider for RunwayImageProvider {
    fn name(&self) -> &str {
        "runway"
    }

    fn configure(&mut self, config: ProviderInstanceConfig) {
        if !config.api_keys.is_empty() {
            self.key_pool = Some(ApiKeyPool::shared(config.api_keys.clone()));
        }
        self.config = Some(config);
    }

    fn is_configured(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|c| self.auth.is_satisfied_by(&c.credentials) || self.key_pool.is_some())
    }

    /// Submit `POST /text_to_image`, poll the task, fetch the first output.
    ///
    /// @see <https://docs.dev.runwayml.com/guides/using-the-api/> — `promptText`,
    ///   `model`, `ratio`, `referenceImages[]` ({uri, tag}), and `GET /tasks/{id}`.
    async fn generate(
        &self,
        model: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<GenerationOutput, ProviderError> {
        let creds = self.creds()?;
        let native = Self::resolve_model(&model.id).ok_or_else(|| ProviderError::ModelNotSupported { model: model.id.clone(), provider: "runway".into() })?;
        let url = format!("{}/text_to_image", self.api_base());

        let mut body = json!({
            "promptText": base.prompt,
            "model": native,
            "ratio": Self::resolve_ratio(extras),
        });
        if let Some(seed) = base.seed {
            body["seed"] = Value::Number(seed.into());
        }

        // Reference images: {uri, tag}. uri may be a URL or a data URI of the
        // image's own type.
        let mut refs: Vec<Value> = Vec::new();
        for (i, r) in materialized.refs.iter().enumerate() {
            let tag = if r.role.is_empty() || r.role == "init" { format!("ref{i}") } else { r.role.clone() };
            if let Some(uri) = runway_uri(&r.form)? {
                refs.push(json!({ "uri": uri, "tag": tag }));
            }
        }
        if !refs.is_empty() {
            body["referenceImages"] = Value::Array(refs);
        }

        if let Some(Value::Object(extra_map)) = &extras.extra {
            if let Some(obj) = body.as_object_mut() {
                for (k, v) in extra_map.iter().filter(|(k, _)| !OWNED_FIELDS.contains(&k.as_str())) {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        let builder = crate::providers::auth::apply(
            &self.auth,
            &creds,
            self.client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("X-Runway-Version", RUNWAY_VERSION)
                .json(&body),
        )?;
        let resp = crate::providers::inject_trace_headers(builder)
            .send()
            .await
            .map_err(|e| ProviderError::RequestFailed {
                message: format!("Runway image request failed: {e}"),
                status_code: e.status().map(|s| s.as_u16()),
                provider_error: None,
                // Only a connection that never opened is safe to retry: a
                // request that timed out may have been accepted and billed.
                retryable: e.is_connect(),
            })?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let submit: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let err = submit["error"].as_str().map(str::to_string).unwrap_or_else(|| text.chars().take(200).collect());
            return Err(ProviderError::RequestFailed {
                message: format!("Runway API error: {err}"),
                status_code: Some(status.as_u16()),
                provider_error: Some(submit),
                // Turned away, not run: "429 | If the client makes too many requests … | Yes".
                // @see <https://docs.dev.runwayml.com/errors/errors.md>
                retryable: status.as_u16() == 429 || status.is_server_error(),
            });
        }
        let submit: Value = serde_json::from_str(&text).map_err(|e| ProviderError::RequestFailed {
            message: format!("Failed to parse Runway response: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: false,
        })?;

        let id = submit["id"].as_str().ok_or_else(|| ProviderError::RequestFailed {
            message: "Runway response missing task id".to_string(),
            status_code: None,
            provider_error: Some(submit.clone()),
            retryable: false,
        })?;
        // Accepted: from here on a failure must not re-send the job (L1).
        extras.vendor_job.record(id);

        let final_result = self.poll_task(id).await?;
        let image_url = match &final_result["output"] {
            Value::Array(arr) => arr.first().and_then(|v| v.as_str()).map(str::to_string),
            Value::String(s) => Some(s.clone()),
            _ => None,
        }
        .ok_or_else(|| ProviderError::RequestFailed {
            message: "Runway task output missing image URL".to_string(),
            status_code: None,
            provider_error: Some(final_result.clone()),
            retryable: false,
        })?;

        let bytes = self.fetch_image_bytes(&image_url).await?;
        let mut metadata = HashMap::new();
        metadata.insert("task_id".to_string(), Value::String(id.to_string()));
        metadata.insert("url".to_string(), Value::String(image_url));

        Ok(GenerationOutput {
            data: bytes,
            content_type: "image/png".to_string(),
            metadata,
            more: Vec::new(),
        })
    }

    async fn estimate_cost(
        &self,
        model: &ModelSchema,
        _request: &ImageGenerationRequest,
    ) -> Result<CostEstimate, ProviderError> {
        Ok(build_cost_estimate(
            model.pricing.base_cost_usd,
            0.0,
            CostSource::Estimated,
            Some(json!({ "model": model.id })),
        ))
    }

    async fn health_check(&self) -> HealthCheckResult {
        if !self.is_configured() {
            return HealthCheckResult {
                healthy: false,
                message: "Runway image provider not configured".into(),
                latency_ms: None,
            };
        }
        // Runway has no cheap unauthenticated ping; report configured.
        HealthCheckResult {
            healthy: true,
            message: "Runway image provider configured".into(),
            latency_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::Cleanup;
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// L13: every catalog row has its own arm, and an id with none is refused
    /// rather than billed as another model.
    #[test]
    fn every_catalog_row_resolves_and_an_unknown_id_does_not() {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&dir).unwrap();
        for m in catalog.for_provider("runway").filter(|m| m.media_type == crate::capabilities::MediaType::Image) {
            assert!(RunwayImageProvider::resolve_model(&m.id).is_some(), "{} has no arm", m.id);
        }
        assert!(RunwayImageProvider::resolve_model("runway/not-a-model").is_none());
    }

    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load");
        r.get(id).expect("model").clone()
    }

    fn make_provider(api_base: &str) -> RunwayImageProvider {
        let mut p = RunwayImageProvider::new();
        let mut cfg = ProviderInstanceConfig {
            api_key: "rw-secret".to_string(),
            api_base: Some(api_base.to_string()),
            ..Default::default()
        };
        cfg.credentials.api_key = Some("rw-secret".to_string());
        p.configure(cfg);
        p.public_assets_only = false;
        p
    }

    fn make_base(prompt: &str, model: &str) -> BaseGenerationRequest {
        BaseGenerationRequest {
            prompt: prompt.to_string(),
            model: model.to_string(),
            n: 1,
            negative_prompt: None,
            seed: None,
            reference_images: vec![],
            strict: true,
            extra: None,
            metadata: None,
        }
    }

    fn make_extras() -> ImageExtras {
        ImageExtras {
            vendor_job: Default::default(),
            size: None,
            aspect_ratio: Some("16:9".to_string()),
            quality: None,
            style: None,
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "url".to_string(),
            edit: None, views: Vec::new(),
            extra: None,
            output_format: None, background: None,
            expand: None,
            layout: None, mask_prompt: None,
            preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
        }
    }

    #[tokio::test]
    async fn generates_gen4_image_via_polling() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        let image_url = format!("{}/out.png", image_server.uri());

        Mock::given(method("POST"))
            .and(path("/text_to_image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "task_1" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/tasks/task_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "task_1", "status": "SUCCEEDED", "output": [image_url.clone()]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"RUNWAYPNG".to_vec()).insert_header("content-type", "image/png"))
            .mount(&image_server)
            .await;

        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4_image");
        let base = make_base("a neon tokyo alley", "runway/gen4_image");
        let extras = make_extras();
        let materialized = MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() };

        let result = provider.generate(&schema, &base, &extras, &materialized).await;
        // The vendor accepted the job before the poll: the router must never re-send it.
        assert_eq!(extras.vendor_job.get().as_deref(), Some("task_1"));
        assert!(result.is_ok(), "generate failed: {:?}", result.err());
        assert_eq!(result.unwrap().data, b"RUNWAYPNG");

        let received = server.received_requests().await.unwrap();
        let post = received.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("authorization").unwrap(), "Bearer rw-secret");
        assert_eq!(post.headers.get("x-runway-version").unwrap(), "2024-11-06");
        let body: Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["promptText"], "a neon tokyo alley");
        assert_eq!(body["model"], "gen4_image");
        assert_eq!(body["ratio"], "1920:1080");
    }

    /// Exercise the real `resolve_ratio`, not a copy of its table: every value
    /// it can produce must be a member of Runway's `ratio` enum.
    #[test]
    fn resolve_ratio_only_ever_returns_enum_members() {
        let mk = |ar: Option<&str>, size: Option<&str>| ImageExtras {
            vendor_job: Default::default(),
            size: size.map(str::to_string),
            aspect_ratio: ar.map(str::to_string),
            quality: None,
            style: None,
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "url".to_string(),
            edit: None, views: Vec::new(),
            extra: None,
            output_format: None, background: None,
            expand: None,
            layout: None, mask_prompt: None,
            preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
        };

        // Every aspect ratio any Runway model advertises.
        for id in ["runway/gen4_image", "runway/gen4_image_turbo"] {
            let schema = ref_schema(id);
            let allowed = match schema.params.get("aspect_ratio") {
                Some(crate::capabilities::schema::ParamSpec::AspectRatio(ar)) => ar.allowed.clone(),
                other => panic!("{id} aspect_ratio is {other:?}"),
            };
            assert!(!allowed.is_empty());
            for ar in allowed {
                let got = RunwayImageProvider::resolve_ratio(&mk(Some(&ar), None));
                assert!(
                    RunwayImageProvider::RATIOS.contains(&got.as_str()),
                    "{id} ratio {ar:?} produced {got:?}, which Runway rejects"
                );
            }
        }

        // Unset, unknown, and free-form pixel sizes must also land in the enum —
        // a caller-supplied size used to be forwarded verbatim.
        for (ar, size) in [
            (None, None),
            (Some("3:2"), None),
            (Some("2:3"), None),
            (Some("banana"), None),
            (None, Some("1000x800")),
            (None, Some("512x2048")),
            (None, Some("4096x1024")),
            (None, Some("not-a-size")),
            (None, Some("100x0")),
        ] {
            let got = RunwayImageProvider::resolve_ratio(&mk(ar, size));
            assert!(
                RunwayImageProvider::RATIOS.contains(&got.as_str()),
                "ar={ar:?} size={size:?} produced {got:?}, which Runway rejects"
            );
        }
    }

    // ─── Runway audit, 2026-10-30 ───────────────────────────────────────────

    fn no_refs() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    /// Runway answers every submit with a 400, so `generate()` returns before
    /// polling; only the outbound body matters.
    async fn refusing_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": "stop" })))
            .mount(&server)
            .await;
        server
    }

    async fn sent_bodies(server: &MockServer) -> Vec<Value> {
        server.received_requests().await.unwrap().iter().map(|r| serde_json::from_slice(&r.body).unwrap()).collect()
    }

    /// #16: `extra` adds Runway's own knobs but never replaces what litegen sends.
    #[tokio::test]
    async fn extra_cannot_replace_what_litegen_sends() {
        let server = refusing_server().await;
        let mut base = make_base("a lighthouse", "runway/gen4_image");
        base.seed = Some(3);
        let mut extras = make_extras();
        extras.extra = Some(json!({
            "model": "gen4_image_turbo", "promptText": "other", "ratio": "720:720", "seed": 9,
            "referenceImages": [], "contentModeration": { "publicFigureThreshold": "low" }
        }));
        let _ = make_provider(&server.uri()).generate(&ref_schema("runway/gen4_image"), &base, &extras, &no_refs()).await;
        let body = &sent_bodies(&server).await[0];
        assert_eq!(body["model"], "gen4_image");
        assert_eq!(body["promptText"], "a lighthouse");
        assert_eq!(body["ratio"], "1920:1080");
        assert_eq!(body["seed"], 3);
        assert!(body.get("referenceImages").is_none(), "{body}");
        assert_eq!(body["contentModeration"]["publicFigureThreshold"], "low");
    }

    /// #17, #18: a submit that timed out may have been accepted and billed, so
    /// it is never sent again; a 429 was turned away and may be ("429 | If the
    /// client makes too many requests … | Yes").
    /// @see <https://docs.dev.runwayml.com/errors/errors.md> (2026-10-30)
    #[tokio::test]
    async fn a_timed_out_submit_is_never_sent_again_and_a_rate_limited_one_may_be() {
        let slow = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t" })).set_delay(std::time::Duration::from_secs(5)))
            .mount(&slow)
            .await;
        let mut p = make_provider(&slow.uri());
        p.client = reqwest::Client::builder().timeout(std::time::Duration::from_millis(100)).build().unwrap();
        let schema = ref_schema("runway/gen4_image");
        let err = p.generate(&schema, &make_base("x", "runway/gen4_image"), &make_extras(), &no_refs()).await.unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");

        let limited = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({ "error": "Too many requests" })))
            .mount(&limited)
            .await;
        let err = make_provider(&limited.uri())
            .generate(&schema, &make_base("x", "runway/gen4_image"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: true, status_code: Some(429), .. }), "{err:?}");
    }

    /// #21: "GIF images are not supported", and a data URI is at most
    /// 5,242,880 characters matching `^data:image\/.*`. A base64 reference goes
    /// with its own type; one Runway cannot take is refused before any call.
    /// @see <https://docs.dev.runwayml.com/assets/inputs.md> (2026-10-30)
    #[tokio::test]
    async fn a_reference_goes_with_its_own_type_and_one_runway_cannot_take_is_refused() {
        use base64::Engine;
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let one = |form| MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef { role: "init".into(), form, time_s: None }],
            cleanup: Cleanup::empty(),
        };
        let server = refusing_server().await;
        let provider = make_provider(&server.uri());
        let schema = ref_schema("runway/gen4_image");
        let base = make_base("x", "runway/gen4_image");
        let jpeg = one(MaterializedRefForm::Base64(b64(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F'])));
        let _ = provider.generate(&schema, &base, &make_extras(), &jpeg).await;
        let uri = sent_bodies(&server).await[0]["referenceImages"][0]["uri"].as_str().unwrap().to_string();
        assert!(uri.starts_with("data:image/jpeg;base64,"), "{}", &uri[..40.min(uri.len())]);

        for form in [
            MaterializedRefForm::Base64(b64(b"GIF89a\x01\x00\x01\x00")),
            MaterializedRefForm::Url(format!("data:image/png;base64,{}", "A".repeat(5_242_880))),
        ] {
            let err = provider.generate(&schema, &base, &make_extras(), &one(form)).await.unwrap_err();
            assert!(matches!(err, ProviderError::InvalidRequest(_)), "{err:?}");
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 1, "a refused reference reached Runway");
    }

    /// #13: a poll Runway answers with an error is that error, not a task
    /// still running ("404 | Requests will return a 404 when the referenced
    /// resource is not available | No").
    /// @see <https://docs.dev.runwayml.com/errors/errors.md> (2026-10-30)
    #[tokio::test]
    async fn a_poll_error_is_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tasks/t1"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "error": "Task not found" })))
            .mount(&server)
            .await;
        let err = make_provider(&server.uri())
            .generate(&ref_schema("runway/gen4_image"), &make_base("x", "runway/gen4_image"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { status_code: Some(404), retryable: false, .. }), "{err:?}");
    }

    /// #23: the result is fetched without the key, only from a public address,
    /// and at most 64 MiB; Runway has billed the task by then, so a refusal is
    /// final.
    #[tokio::test]
    async fn a_result_is_fetched_only_from_a_public_address() {
        let server = MockServer::start().await;
        let image_server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "t1" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tasks/t1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "t1", "status": "SUCCEEDED", "output": [format!("{}/out.png", image_server.uri())]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"\x89PNG\r\n\x1a\n".to_vec()))
            .mount(&image_server)
            .await;
        let mut provider = make_provider(&server.uri());
        provider.public_assets_only = true;
        let err = provider
            .generate(&ref_schema("runway/gen4_image"), &make_base("x", "runway/gen4_image"), &make_extras(), &no_refs())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::RequestFailed { retryable: false, .. }), "{err:?}");
        assert!(image_server.received_requests().await.unwrap().is_empty(), "a private address was fetched");
    }
}
