use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

use crate::capabilities::ModelSchema;
use crate::config::AppConfig;
use crate::proxy::circuit_breaker::CircuitBreaker;
use crate::providers::{apply_markup, GenerationOutput, ImageExtras, Model3dExtras, Model3dGenerationHandle, ProviderCredentials, ProviderError, VideoExtras, VideoGenerationHandle};
use crate::proxy::cache::GenerationCache;
use crate::proxy::materializer::MaterializedRequest;
use crate::proxy::registry::ProviderRegistry;
use crate::proxy::storage::ImageStore;
use crate::types::*;

/// Maximum number of latency samples retained per provider.
const LATENCY_HISTORY_CAP: usize = 16;

/// USD per internal token. Kept next to the async job maps because every
/// `UsageInfo` the router mints — submit and poll alike — must convert at the
/// same rate, or the same generation would quote two different token counts.
const USD_PER_TOKEN: f64 = 0.001;

/// How long the router keeps a submitted 3D job in memory. The poller reaps a
/// generation still active after 2 hours, so a job older than this has a
/// terminal row and nothing left to poll it through.
const MODEL3D_JOB_MAX_AGE_SECS: i64 = 3 * 60 * 60;

/// An in-flight async job plus the `usage` triple quoted when it was submitted.
///
/// The consumer contract reads `usage` off the POLL response, not the submit
/// response (a client that submits and polls from different processes only ever
/// sees the poll). Carrying the submit-time triple alongside the handle lets
/// every poll report the identical `cost_usd` / `tokens` / `cost_source` the
/// submit quoted and the generations row persisted, with no DB round trip and
/// no dependence on the row insert (which the submit handler spawns) having
/// landed yet.
#[derive(Clone)]
struct TrackedJob<H> {
    handle: H,
    usage: Option<UsageInfo>,
}

/// An in-flight 3D job.
///
/// Carries the containers promised to the caller at submit time alongside the
/// handle, rather than on `Model3dGenerationHandle`, so no adapter can forget
/// to populate them: the router stamps this from the validated extras at the
/// one point a job is tracked, and every observer of completion reads it back
/// from here. It is also written into `metadata.requested_formats` on the
/// terminal row, because the row outlives this map.
///
/// It also keeps the provider INSTANCE that submitted the job. With a per-org
/// BYO credential that instance is the only one holding the key the vendor
/// knows this job by: polling through the platform-default instance instead
/// asked the vendor with the operator's key (or no key at all), so a client's
/// `GET /v1/models3d/{id}` could never advance a BYO job and every one waited on
/// the background poller. The same instance answers `cancel`.
#[derive(Clone)]
struct Model3dJob {
    handle: Model3dGenerationHandle,
    usage: Option<UsageInfo>,
    /// What delivering it must do: the same plan the submit recorded on the
    /// row for the poller.
    delivery: crate::proxy::model3d_deliver::DeliveryPlan,
    provider: Arc<dyn crate::providers::Model3dProvider>,
    /// Unix seconds at submit. Every poll reports this, not the poll time, so
    /// `created` means the same thing on every observer of one generation.
    created: i64,
}

/// The main proxy router that resolves model routes, applies routing
/// strategies (fallback, weighted, lowest-cost, lowest-latency), caching, and retries.
pub struct ProxyRouter {
    pub registry: Arc<ProviderRegistry>,
    pub cache: Arc<GenerationCache>,
    pub config: Arc<AppConfig>,
    pub image_store: Arc<dyn ImageStore>,
    /// Per-provider latency history (recent samples in ms), capped at LATENCY_HISTORY_CAP.
    latency_history: Arc<tokio::sync::RwLock<HashMap<String, VecDeque<u64>>>>,
    /// In-flight video generation jobs, keyed by the locally-generated `litegen-vid-...` ID.
    video_jobs: Arc<tokio::sync::RwLock<HashMap<String, TrackedJob<VideoGenerationHandle>>>>,
    /// In-flight 3D generation jobs, keyed by the local `litegen-3d-...` ID.
    model3d_jobs: Arc<tokio::sync::RwLock<HashMap<String, Model3dJob>>>,
    /// Storage for re-hosted 3D assets (S3 when configured, local otherwise).
    pub model3d_store: Arc<dyn crate::proxy::storage::ImageStorage>,
    /// Circuit breaker tracking consecutive failures per provider.
    pub circuit_breaker: Arc<CircuitBreaker>,
    /// Settings changed at runtime through the management API. Each set field
    /// overrides the matching `config` value for every request that follows.
    overrides: std::sync::RwLock<RuntimeOverrides>,
}

/// Runtime overrides of [`AppConfig`] values, set through `/v1/admin/*`.
#[derive(Debug, Clone, Default)]
pub struct RuntimeOverrides {
    /// Replaces `config.model_routes` when set.
    pub model_routes: Option<Vec<crate::config::ModelRouteConfig>>,
    /// The stored gateway settings (markup, alerting, end-user defaults).
    pub gateway: crate::management::types::GatewaySettings,
}

impl ProxyRouter {
    pub fn new(
        registry: Arc<ProviderRegistry>,
        cache: Arc<GenerationCache>,
        config: Arc<AppConfig>,
        image_store: Arc<dyn ImageStore>,
    ) -> Self {
        let cb = Arc::new(CircuitBreaker::new(
            config.circuit_breaker.threshold,
            std::time::Duration::from_secs(config.circuit_breaker.open_for_seconds),
        ));
        // Derived from the same config the image store uses — 3D assets get
        // their own key prefix (see `MODEL3D_PATH_PREFIX`) but share the S3
        // bucket / local-serving decision, so no separate config surface is
        // needed for this modality.
        let model3d_store = crate::proxy::storage::build_model3d_store(
            &config.image_storage,
            &config.server.public_base_url(),
        );
        Self {
            registry,
            cache,
            config,
            image_store,
            latency_history: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            video_jobs: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            model3d_jobs: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            model3d_store,
            circuit_breaker: cb,
            overrides: std::sync::RwLock::new(RuntimeOverrides::default()),
        }
    }

    /// A snapshot of the runtime overrides.
    pub fn overrides(&self) -> RuntimeOverrides {
        self.overrides.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Replace the runtime overrides; applies to every request that follows.
    pub fn set_overrides(&self, overrides: RuntimeOverrides) {
        *self.overrides.write().unwrap_or_else(|p| p.into_inner()) = overrides;
    }

    /// The markup in effect: the runtime override, else the config value.
    pub fn cost_markup_percent(&self) -> f64 {
        self.overrides
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .gateway
            .cost_markup_percent
            .unwrap_or(self.config.cost_markup_percent)
    }

    /// The model routes in effect: the runtime override, else the config's.
    pub fn model_routes(&self) -> Vec<crate::config::ModelRouteConfig> {
        self.overrides
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .model_routes
            .clone()
            .unwrap_or_else(|| self.config.model_routes.clone())
    }

    /// Record a latency sample for a provider. Keeps at most LATENCY_HISTORY_CAP samples.
    async fn record_latency(&self, provider: &str, ms: u64) {
        let mut history = self.latency_history.write().await;
        let deque = history.entry(provider.to_string()).or_insert_with(VecDeque::new);
        deque.push_back(ms);
        if deque.len() > LATENCY_HISTORY_CAP {
            deque.pop_front();
        }
    }

    /// Compute the average latency for a provider (u64::MAX if no samples).
    async fn avg_latency(&self, provider: &str) -> u64 {
        let history = self.latency_history.read().await;
        match history.get(provider) {
            Some(deque) if !deque.is_empty() => {
                let sum: u64 = deque.iter().sum();
                sum / deque.len() as u64
            }
            _ => u64::MAX,
        }
    }

    /// Test helper: directly insert latency samples for a provider.
    #[cfg(test)]
    pub async fn set_latency_history(&self, provider: &str, samples: Vec<u64>) {
        let mut history = self.latency_history.write().await;
        let deque: VecDeque<u64> = samples.into_iter().collect();
        history.insert(provider.to_string(), deque);
    }

    // ─── Image Generation ───────────────────────────────────────────────

    /// Generate an image through the proxy using the typed schema + materialized refs.
    /// If a model route is configured, it dispatches through the route's strategy
    /// (fallback, weighted_round_robin, lowest_cost, lowest_latency). Otherwise,
    /// falls back to single-provider direct dispatch using schema.provider.
    #[tracing::instrument(
        // `app_creds` carries decrypted BYO provider secrets — see the note on
        // `generate_model3d`; an instrumented argument becomes a span attribute.
        skip(self, schema, base, extras, materialized, app_creds, app_store),
        fields(model = %schema.id, provider = %schema.provider)
    )]
    pub async fn generate_image(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
        app_creds: Option<ProviderCredentials>,
        app_store: Option<Arc<dyn ImageStore>>,
        // Tenant scope for the cache key (app id, falling back to org id). The
        // cache is process-global, so this prevents cross-tenant cache hits.
        tenant: Option<&str>,
    ) -> Result<ImageGenerationResponse, ProxyError> {
        let start = Instant::now();

        // 1. Check cache
        if let Some(cached) = self.cache.get_image(tenant, &schema.id, base, extras).await {
            info!(model = %schema.id, "Cache hit for image generation");
            return Ok(cached);
        }

        // 2. Find a route or use single-provider direct dispatch
        let (provider_name, output) = if let Some(route) = self.find_model_route(&schema.id) {
            self.execute_route_image(schema, base, extras, materialized, &route, app_creds).await?
        } else {
            // Direct dispatch: use schema.provider
            let provider = self.registry
                .image_provider_for_request(&schema.provider, app_creds)
                .await
                .ok_or_else(|| ProxyError::ProviderNotConfigured(schema.provider.clone()))?;

            let max_retries = 2u32;
            let mut last_error: Option<ProviderError> = None;
            let mut output: Option<GenerationOutput> = None;

            for attempt in 0..=max_retries {
                if attempt > 0 {
                    info!(
                        provider = %schema.provider,
                        attempt = attempt,
                        "Retrying image generation"
                    );
                }
                extras.vendor_job.clear();
                let attempt_start = Instant::now();
                match tokio::time::timeout(
                    self.config.server.vendor_timeout(),
                    provider.generate(schema, base, extras, materialized),
                ).await {
                    Ok(Ok(o)) => {
                        self.record_latency(&schema.provider, attempt_start.elapsed().as_millis() as u64).await;
                        output = Some(o);
                        break;
                    }
                    Ok(Err(ProviderError::Timeout { .. })) | Err(_) => {
                        return Err(ProxyError::VendorTimeout {
                            model: schema.id.clone(),
                            provider: schema.provider.clone(),
                            vendor_job_id: extras.vendor_job.get(),
                        });
                    }
                    Ok(Err(e)) => {
                        let resend = may_resend(&e, extras.vendor_job.get().is_some());
                        last_error = Some(e);
                        if !resend { break; }
                    }
                }
            }

            let o = output.ok_or_else(|| failure(&schema.id, &schema.provider, last_error, extras.vendor_job.get()))?;
            (schema.provider.clone(), o)
        };

        let latency_ms = start.elapsed().as_millis() as u64;

        // 3D doc §4: the init pasted back outside the edit area, before any
        // result is stored.
        let output = match extras.edit.as_ref().and_then(|e| e.preserve.clone()) {
            Some(p) => preserve_output(output, p, schema).await?,
            None => output,
        };

        // 3. Build the image results, then bill for the number actually
        //    produced. The proxy currently returns a single image per
        //    generation (GenerationOutput carries one image), so billing must
        //    reflect delivered images — NOT the requested `n`, or an n>1
        //    request would be over-charged for images it never receives.
        let data = build_image_results(
            &output,
            extras,
            app_store.as_ref().unwrap_or(&self.image_store),
        )
        .await;
        // A multiview model's views come from one billed vendor call.
        let produced = if schema.task == crate::capabilities::ModelTask::Multiview { 1.0 } else { data.len().max(1) as f64 };
        let base_cost = unit_price(schema, base, extras) * produced;
        let (markup, total) = apply_markup(base_cost, self.cost_markup_percent());
        let _ = markup;
        let usage = Some(UsageInfo {
            cost_usd: total,
            tokens: crate::providers::usd_to_tokens(total, USD_PER_TOKEN),
            cost_source: CostSource::Estimated,
        });

        let response = ImageGenerationResponse {
            created: chrono::Utc::now().timestamp(),
            data,
            model: schema.id.clone(),
            provider: provider_name.clone(),
            usage,
            id: format!("litegen-img-{}", uuid::Uuid::new_v4()),
        };

        // 4. Store in cache
        self.cache
            .put_image(tenant, &schema.id, base, extras, &response)
            .await;

        info!(
            model = %schema.id,
            provider = %provider_name,
            latency_ms = latency_ms,
            "Image generation completed"
        );

        Ok(response)
    }

    /// Promptable segmentation (PixelGenie L7): the vendor's masks, each made
    /// a black-and-white PNG the image's size with its measured box, best
    /// first; billed once per call. Retried like an image generation.
    #[tracing::instrument(skip(self, schema, input, app_creds), fields(model = %schema.id, provider = %schema.provider))]
    pub async fn segment_image(
        &self,
        schema: &ModelSchema,
        input: &crate::providers::SegmentationInput,
        app_creds: Option<ProviderCredentials>,
    ) -> Result<crate::types::ImageSegmentationResponse, ProxyError> {
        let provider = self
            .registry
            .image_provider_for_request(&schema.provider, app_creds)
            .await
            .ok_or_else(|| ProxyError::ProviderNotConfigured(schema.provider.clone()))?;
        let mut last_error: Option<ProviderError> = None;
        let mut vendor_masks = None;
        for attempt in 0..=2u32 {
            if attempt > 0 {
                info!(provider = %schema.provider, attempt, "Retrying segmentation");
            }
            let attempt_start = Instant::now();
            match tokio::time::timeout(self.config.server.vendor_timeout(), provider.segment(schema, input)).await {
                Ok(Ok(m)) => {
                    self.record_latency(&schema.provider, attempt_start.elapsed().as_millis() as u64).await;
                    vendor_masks = Some(m);
                    break;
                }
                Ok(Err(ProviderError::Timeout { .. })) | Err(_) => {
                    return Err(ProxyError::VendorTimeout { model: schema.id.clone(), provider: schema.provider.clone(), vendor_job_id: None });
                }
                Ok(Err(e)) => {
                    let resend = may_resend(&e, false);
                    last_error = Some(e);
                    if !resend {
                        break;
                    }
                }
            }
        }
        let vendor_masks = vendor_masks.ok_or_else(|| failure(&schema.id, &schema.provider, last_error, None))?;
        let (width, height, max) = (input.width, input.height, input.max_masks);
        let masks = tokio::task::spawn_blocking(move || super::segmentation::masks(vendor_masks, width, height, max))
            .await
            .map_err(|e| ProxyError::Internal(format!("mask normalisation stopped: {e}")))?
            .map_err(|e| {
                let unreadable = ProviderError::RequestFailed {
                    message: format!("returned a mask litegen cannot read: {e}"),
                    status_code: None,
                    provider_error: None,
                    retryable: false,
                };
                ProxyError::VendorFailed {
                    model: schema.id.clone(),
                    provider: schema.provider.clone(),
                    vendor: VendorError::from_provider(&unreadable),
                    vendor_job_id: None,
                }
            })?;
        let (_, total) = apply_markup(crate::capabilities::pricing::row_price_usd(schema), self.cost_markup_percent());
        Ok(crate::types::ImageSegmentationResponse {
            id: format!("litegen-seg-{}", uuid::Uuid::new_v4()),
            created: chrono::Utc::now().timestamp(),
            model: schema.id.clone(),
            provider: schema.provider.clone(),
            masks,
            usage: Some(UsageInfo {
                cost_usd: total,
                tokens: crate::providers::usd_to_tokens(total, USD_PER_TOKEN),
                cost_source: CostSource::Estimated,
            }),
        })
    }

    /// Dispatch an image request through a configured model route.
    async fn execute_route_image(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
        route: &ModelRoute,
        app_creds: Option<ProviderCredentials>,
    ) -> Result<(String, GenerationOutput), ProxyError> {
        match route.strategy {
            RoutingStrategy::Fallback => {
                self.fallback_image(schema, base, extras, materialized, &route.deployments, app_creds).await
            }
            RoutingStrategy::WeightedRoundRobin => {
                self.weighted_image(schema, base, extras, materialized, &route.deployments, app_creds).await
            }
            RoutingStrategy::LowestCost => {
                self.lowest_cost_image(schema, base, extras, materialized, &route.deployments, app_creds).await
            }
            RoutingStrategy::LowestLatency => {
                self.lowest_latency_image(schema, base, extras, materialized, &route.deployments, app_creds).await
            }
        }
    }

    /// Try deployments in order, falling back on retryable errors.
    async fn fallback_image(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
        deployments: &[Deployment],
        app_creds: Option<ProviderCredentials>,
    ) -> Result<(String, GenerationOutput), ProxyError> {
        let mut last_error: Option<ProviderError> = None;

        for deployment in deployments {
            // ── Circuit breaker: skip provider if breaker is open ──
            if self.circuit_breaker.is_open(&deployment.provider).await {
                warn!(
                    provider = %deployment.provider,
                    "Circuit breaker open — skipping deployment"
                );
                last_error = Some(ProviderError::NotConfigured(deployment.provider.clone()));
                continue;
            }

            let provider = match self.registry.image_provider_for_request(&deployment.provider, app_creds.clone()).await {
                Some(p) => p,
                None => {
                    warn!(provider = %deployment.provider, "Image provider not configured, skipping deployment");
                    // Treat missing provider as a non-retryable skip
                    last_error = Some(ProviderError::NotConfigured(deployment.provider.clone()));
                    continue;
                }
            };

            for attempt in 0..=deployment.max_retries {
                if attempt > 0 {
                    info!(
                        provider = %deployment.provider,
                        attempt = attempt,
                        "Retrying image generation"
                    );
                }
                extras.vendor_job.clear();
                let attempt_start = Instant::now();
                match tokio::time::timeout(
                    std::time::Duration::from_secs(deployment.timeout_seconds),
                    provider.generate(schema, base, extras, materialized),
                ).await {
                    Ok(Ok(output)) => {
                        self.record_latency(&deployment.provider, attempt_start.elapsed().as_millis() as u64).await;
                        self.circuit_breaker.record_success(&deployment.provider).await;
                        return Ok((deployment.provider.clone(), output));
                    }
                    Ok(Err(ProviderError::Timeout { .. })) | Err(_) => {
                        warn!(
                            provider = %deployment.provider,
                            timeout_s = deployment.timeout_seconds,
                            "Image request timed out; not re-sent"
                        );
                        self.circuit_breaker.record_failure(&deployment.provider).await;
                        // Neither retried nor handed to the next deployment:
                        // this one may still run, and bill, the job.
                        return Err(ProxyError::VendorTimeout {
                            model: schema.id.clone(),
                            provider: deployment.provider.clone(),
                            vendor_job_id: extras.vendor_job.get(),
                        });
                    }
                    Ok(Err(e)) => {
                        warn!(
                            provider = %deployment.provider,
                            error = %e,
                            retryable = e.is_retryable(),
                            "Image provider error"
                        );
                        self.circuit_breaker.record_failure(&deployment.provider).await;
                        if let Some(job) = extras.vendor_job.get() {
                            // The vendor took the job: no other deployment may
                            // be sent it too.
                            return Err(failure(&schema.id, &deployment.provider, Some(e), Some(job)));
                        }
                        let resend = may_resend(&e, false);
                        last_error = Some(e);
                        if !resend {
                            break; // Skip to next deployment
                        }
                    }
                }
            }
        }

        Err(failure(&schema.id, &schema.provider, last_error, None))
    }

    /// Weighted round-robin across deployments: pick a deployment by weight hash,
    /// then fall back to remaining deployments in order.
    async fn weighted_image(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
        deployments: &[Deployment],
        app_creds: Option<ProviderCredentials>,
    ) -> Result<(String, GenerationOutput), ProxyError> {
        if deployments.is_empty() {
            return Err(ProxyError::NoDeployments { model: schema.id.clone() });
        }

        // Build weighted schedule
        let mut schedule: Vec<usize> = Vec::new();
        for (i, d) in deployments.iter().enumerate() {
            for _ in 0..d.weight.max(1) {
                schedule.push(i);
            }
        }

        // Pick a deterministic slot based on prompt hash
        let slot = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            base.prompt.hash(&mut hasher);
            schema.id.hash(&mut hasher);
            hasher.finish() as usize
        };
        let primary_idx = schedule[slot % schedule.len()];

        // Try primary first, then remaining in original order
        let mut ordered: Vec<&Deployment> = Vec::with_capacity(deployments.len());
        ordered.push(&deployments[primary_idx]);
        for (i, d) in deployments.iter().enumerate() {
            if i != primary_idx {
                ordered.push(d);
            }
        }
        let ordered_owned: Vec<Deployment> = ordered.into_iter().cloned().collect();
        self.fallback_image(schema, base, extras, materialized, &ordered_owned, app_creds).await
    }

    /// Sort deployments by estimated cost (ascending) then fall back in that order.
    async fn lowest_cost_image(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
        deployments: &[Deployment],
        app_creds: Option<ProviderCredentials>,
    ) -> Result<(String, GenerationOutput), ProxyError> {
        let mut cost_sorted: Vec<(f64, &Deployment)> = Vec::new();
        // Build a minimal ImageGenerationRequest for cost estimation
        let dummy_req = ImageGenerationRequest {
            base: base.clone(),
            size: extras.size.clone(),
            aspect_ratio: extras.aspect_ratio.clone(),
            quality: extras.quality.clone(),
            style: extras.style.clone(),
            steps: extras.steps,
            guidance_scale: extras.guidance_scale,
            strength: extras.strength,
            response_format: extras.response_format.clone(),
            region: None, mask_grow_px: None, views: None,
            output_format: None, background: None,
            expand: extras.expand,
            layout: None, mask_prompt: None,
            preserve_unmasked: None, feather_px: None,
        };
        for d in deployments {
            let cost = if let Some(p) = self.registry.image_provider_for_request(&d.provider, app_creds.clone()).await {
                p.estimate_cost(schema, &dummy_req)
                    .await
                    .map(|c| c.total_cost_usd)
                    .unwrap_or(f64::MAX)
            } else {
                f64::MAX
            };
            cost_sorted.push((cost, d));
        }
        cost_sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let sorted: Vec<Deployment> = cost_sorted.into_iter().map(|(_, d)| d.clone()).collect();
        self.fallback_image(schema, base, extras, materialized, &sorted, app_creds).await
    }

    /// Sort deployments by average measured latency (ascending), fall back in that order.
    /// Providers with no history sort to the end (u64::MAX average).
    async fn lowest_latency_image(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
        deployments: &[Deployment],
        app_creds: Option<ProviderCredentials>,
    ) -> Result<(String, GenerationOutput), ProxyError> {
        let mut lat_sorted: Vec<(u64, &Deployment)> = Vec::new();
        for d in deployments {
            let avg = self.avg_latency(&d.provider).await;
            lat_sorted.push((avg, d));
        }
        lat_sorted.sort_by_key(|(avg, _)| *avg);
        let sorted: Vec<Deployment> = lat_sorted.into_iter().map(|(_, d)| d.clone()).collect();
        self.fallback_image(schema, base, extras, materialized, &sorted, app_creds).await
    }

    // ─── Video Generation ───────────────────────────────────────────────

    /// Start a video generation through the proxy.
    /// If a model route is configured, dispatches through route strategy.
    /// Otherwise falls back to single-provider direct dispatch using schema.provider.
    #[tracing::instrument(
        // `app_creds` carries decrypted BYO provider secrets — see the note on
        // `generate_model3d`; an instrumented argument becomes a span attribute.
        skip(self, schema, base, extras, materialized, app_creds),
        fields(model = %schema.id, provider = %schema.provider)
    )]
    pub async fn generate_video(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
        app_creds: Option<ProviderCredentials>,
    ) -> Result<VideoGenerationResponse, ProxyError> {
        // Find route or use direct dispatch
        let (provider_name, handle) = if let Some(route) = self.find_model_route(&schema.id) {
            self.execute_route_video(schema, base, extras, materialized, &route, app_creds).await?
        } else {
            // Direct dispatch
            let provider = self.registry
                .video_provider_for_request(&schema.provider, app_creds)
                .await
                .ok_or_else(|| ProxyError::ProviderNotConfigured(schema.provider.clone()))?;

            let max_retries = 2u32;
            let mut last_error: Option<ProviderError> = None;
            let mut handle: Option<VideoGenerationHandle> = None;

            for attempt in 0..=max_retries {
                if attempt > 0 {
                    info!(
                        provider = %schema.provider,
                        attempt = attempt,
                        "Retrying video generation"
                    );
                }
                let attempt_start = Instant::now();
                match tokio::time::timeout(
                    self.config.server.vendor_timeout(),
                    provider.generate(schema, base, extras, materialized),
                ).await {
                    Ok(Ok(h)) => {
                        self.record_latency(&schema.provider, attempt_start.elapsed().as_millis() as u64).await;
                        handle = Some(h);
                        break;
                    }
                    // A submit that timed out may have been accepted: a second
                    // one would queue (and bill) a job nothing tracks.
                    Ok(Err(ProviderError::Timeout { .. })) | Err(_) => {
                        return Err(ProxyError::VendorTimeout {
                            model: schema.id.clone(),
                            provider: schema.provider.clone(),
                            vendor_job_id: None,
                        });
                    }
                    Ok(Err(e)) => {
                        let resend = may_resend(&e, false);
                        last_error = Some(e);
                        if !resend { break; }
                    }
                }
            }

            let h = handle.ok_or_else(|| failure(&schema.id, &schema.provider, last_error, None))?;
            (schema.provider.clone(), h)
        };

        // Build cost from schema pricing. This path submits exactly one
        // provider job and returns exactly one video (a single video_url), so
        // billing must reflect the one video delivered — NOT the requested `n`,
        // which is meaningless for video and unbounded on the wire. (Same
        // reasoning as the image path above, which bills per delivered image.)
        let produced = 1.0;
        let base_cost = unit_price(schema, base, extras) * produced;
        let (_, total) = apply_markup(base_cost, self.cost_markup_percent());
        let usage_info = Some(UsageInfo {
            cost_usd: total,
            tokens: crate::providers::usd_to_tokens(total, USD_PER_TOKEN),
            cost_source: CostSource::Estimated,
        });

        let local_id = format!("litegen-vid-{}", uuid::Uuid::new_v4());
        self.video_jobs.write().await.insert(
            local_id.clone(),
            TrackedJob { handle, usage: usage_info.clone() },
        );

        Ok(VideoGenerationResponse {
            id: local_id,
            status: GenerationStatus::Pending,
            model: schema.id.clone(),
            provider: provider_name,
            video_url: None,
            progress: 0,
            error: None,
            usage: usage_info,
            created: chrono::Utc::now().timestamp(),
            media: None,
            segments: None,
        })
    }

    /// Look up an in-flight video generation by local ID and poll its provider.
    #[tracing::instrument(skip(self, app_store), fields(id = %id))]
    /// Poll an in-flight video by local id. A finished video is re-hosted in
    /// the same call (`rehost_video`), into the app's own bucket when it has
    /// one, so the answer names litegen's copy; the second value is what its
    /// generation row records (`metadata.assets`). Until the copy is stored the
    /// job stays tracked and the call answers a retryable error.
    pub async fn get_video_status(
        &self,
        id: &str,
        app_store: Option<Arc<dyn crate::proxy::storage::ImageStorage>>,
        app_prefix: Option<&str>,
    ) -> Result<(VideoGenerationResponse, Option<serde_json::Value>), ProxyError> {
        let job = {
            let jobs = self.video_jobs.read().await;
            jobs.get(id).cloned()
        };
        let TrackedJob { handle, usage } =
            job.ok_or_else(|| ProxyError::NotFound(format!("video job '{}' not found", id)))?;

        let provider = self.registry
            .video_provider_for(&handle.provider)
            .await
            .ok_or_else(|| ProxyError::ProviderNotConfigured(handle.provider.clone()))?;

        let poll = provider.poll_status(&handle).await.map_err(|e| ProxyError::ProviderError {
            provider: handle.provider.clone(),
            error: e.to_string(),
            retryable: e.is_retryable(),
        })?;

        let (video_url, assets) = if poll.status == GenerationStatus::Completed {
            let stored = rehost_video(
                app_store.as_ref().unwrap_or(&self.model3d_store),
                app_prefix,
                id,
                poll.video_data.clone(),
                poll.video_url.as_deref(),
                poll.content_type.as_deref(),
            )
            .await
            .map_err(|e| ProxyError::ProviderError {
                provider: handle.provider.clone(),
                error: format!("the video finished but could not be kept: {e}"),
                retryable: true,
            })?;
            (Some(stored.url), Some(serde_json::json!({ "assets": [stored.asset] })))
        } else {
            (poll.video_url.clone(), None)
        };
        let media = assets.as_ref().and_then(|a| crate::types::VideoMedia::from_assets(a));

        if matches!(
            poll.status,
            GenerationStatus::Completed | GenerationStatus::Failed | GenerationStatus::Cancelled
        ) {
            self.video_jobs.write().await.remove(id);
        }

        Ok((
            VideoGenerationResponse {
                id: id.to_string(),
                status: poll.status,
                model: handle.model.clone(),
                provider: handle.provider.clone(),
                video_url,
                progress: poll.progress,
                error: poll.error,
                // The triple quoted at submit, not `None`: the consumer's cost path
                // reads `usage` off the poll response it actually waits for.
                usage,
                created: chrono::Utc::now().timestamp(),
                media,
                segments: None,
            },
            assets,
        ))
    }

    // ─── 3D Model Generation ────────────────────────────────────────────

    /// The vendor's free preflight for a 3D request (3D doc §3B), run before
    /// the billed submit. Bounded by the vendor timeout: a check that cannot
    /// finish refuses nothing and bills nothing, so the caller retries.
    pub async fn preflight_model3d(
        &self,
        schema: &ModelSchema,
        extras: &Model3dExtras,
        app_creds: Option<ProviderCredentials>,
    ) -> Result<crate::providers::Preflight, ProxyError> {
        let provider = self
            .registry
            .model3d_provider_for_request(&schema.provider, app_creds)
            .await
            .ok_or_else(|| ProxyError::ProviderNotConfigured(schema.provider.clone()))?;
        let limit = self.config.server.vendor_timeout();
        match tokio::time::timeout(limit, provider.preflight(schema, extras)).await {
            Ok(Ok(p)) => Ok(p),
            Ok(Err(e)) => Err(ProxyError::ProviderError {
                provider: schema.provider.clone(),
                error: format!("preflight failed: {e}"),
                retryable: e.is_retryable(),
            }),
            Err(_) => Err(ProxyError::ProviderError {
                provider: schema.provider.clone(),
                error: format!("preflight did not finish within {}s", limit.as_secs()),
                retryable: true,
            }),
        }
    }

    /// Submit a 3D generation. Always async: the response is `pending` and the
    /// poller (or `get_model3d_status`) drives it to a terminal state.
    // `app_creds` is in `skip` because it is NOT loggable: its derived `Debug`
    // prints a tenant's decrypted BYO `api_key`/`key_secret` verbatim, and an
    // instrumented argument is recorded as a span attribute on every span AND
    // inherited by every event inside it — so with the JSON/pretty fmt layer or
    // an OTLP exporter the plaintext secret lands in the log sink. Same reason
    // for `generate_image`/`generate_video`. Never remove one from this list.
    #[tracing::instrument(
        skip(self, schema, base, extras, materialized, app_creds),
        fields(model = %schema.id, provider = %schema.provider)
    )]
    pub async fn generate_model3d(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &Model3dExtras,
        materialized: &MaterializedRequest,
        app_creds: Option<ProviderCredentials>,
    ) -> Result<Model3dGenerationResponse, ProxyError> {
        let provider = self
            .registry
            .model3d_provider_for_request(&schema.provider, app_creds)
            .await
            .ok_or_else(|| ProxyError::ProviderNotConfigured(schema.provider.clone()))?;

        let max_retries = 2u32;
        let mut last_error: Option<ProviderError> = None;
        let mut handle: Option<Model3dGenerationHandle> = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                info!(provider = %schema.provider, attempt, "Retrying 3D generation");
            }
            let attempt_start = Instant::now();
            match tokio::time::timeout(
                std::time::Duration::from_secs(120),
                provider.generate(schema, base, extras, materialized),
            )
            .await
            {
                Ok(Ok(h)) => {
                    self.record_latency(&schema.provider, attempt_start.elapsed().as_millis() as u64).await;
                    handle = Some(h);
                    break;
                }
                Ok(Err(e)) => {
                    let retryable = e.is_retryable();
                    last_error = Some(e);
                    if !retryable { break; }
                }
                Err(_) => {
                    // A queue submit is not idempotent: dropping the future does
                    // not withdraw a job the vendor may already have accepted,
                    // so re-POSTing here can queue (and bill) a second and a
                    // third job that nothing ever tracks or cancels.
                    last_error = Some(ProviderError::Timeout { timeout_ms: 120_000 });
                    break;
                }
            }
        }

        let handle = handle.ok_or_else(|| failure(&schema.id, &schema.provider, last_error, None))?;

        // One submitted job produces one mesh: one unit at this request's
        // price — `n` is meaningless here, exactly as it is for video.
        let base_cost = unit_price(schema, base, extras);
        let (_, total) = apply_markup(base_cost, self.cost_markup_percent());
        let usage_info = Some(UsageInfo {
            cost_usd: total,
            tokens: crate::providers::usd_to_tokens(total, USD_PER_TOKEN),
            cost_source: CostSource::Estimated,
        });

        // An empty set would silently disable the completion guard, so it is
        // floored at GLB rather than trusted. The handler always resolves a
        // non-empty list; this catches extras assembled by some other caller.
        let requested_formats = if extras.output_formats.is_empty() {
            vec![crate::types::DEFAULT_MODEL3D_FORMAT.to_string()]
        } else {
            extras.output_formats.clone()
        };

        let local_id = format!("litegen-3d-{}", uuid::Uuid::new_v4());
        let created = chrono::Utc::now().timestamp();
        {
            let mut jobs = self.model3d_jobs.write().await;
            // Only a client GET that itself observes completion (or a cancel)
            // removes a job; one the poller finishes for a webhook-only caller
            // would otherwise stay here, provider instance and all, until a
            // restart. Past the poller's 2 h reap age no observer can need it.
            jobs.retain(|_, j| created - j.created < MODEL3D_JOB_MAX_AGE_SECS);
            jobs.insert(
                local_id.clone(),
                Model3dJob {
                    handle,
                    usage: usage_info.clone(),
                    delivery: crate::proxy::model3d_deliver::DeliveryPlan::for_request(
                        schema,
                        &Model3dExtras { output_formats: requested_formats.clone(), ..extras.clone() },
                    ),
                    provider,
                    created,
                },
            );
        }

        Ok(Model3dGenerationResponse {
            id: local_id,
            status: GenerationStatus::Pending,
            model: schema.id.clone(),
            provider: schema.provider.clone(),
            assets: Vec::new(),
            progress: 0,
            error: None,
            usage: usage_info,
            created,
        })
    }

    /// Poll an in-flight 3D generation by local ID, re-hosting its files if the
    /// provider reports completion on this call.
    ///
    /// `app_store`/`app_prefix` are the calling app's BYO bucket, resolved by
    /// the handler exactly as `generate_image` takes `app_store`. They are NOT
    /// optional in spirit: this call and the poller are the two paths that can
    /// observe completion, and the poller already re-hosts into the app's
    /// bucket. Passing `None` here when the app has one would put that tenant's
    /// mesh in the operator's bucket depending only on which observer won.
    #[tracing::instrument(skip(self, app_store, db), fields(id = %id))]
    pub async fn get_model3d_status(
        &self,
        id: &str,
        app_store: Option<Arc<dyn crate::proxy::storage::ImageStorage>>,
        app_prefix: Option<&str>,
        db: &Arc<dyn crate::db::DatabaseStore>,
    ) -> Result<Model3dGenerationResponse, ProxyError> {
        let job = {
            let jobs = self.model3d_jobs.read().await;
            jobs.get(id).cloned()
        };
        let Model3dJob { mut handle, usage, delivery, provider, created } =
            job.ok_or_else(|| ProxyError::NotFound(format!("3d job '{}' not found", id)))?;
        let requested_formats = delivery.requested_formats.clone();

        // The instance that submitted the job — see `Model3dJob`.
        let mut poll = provider.poll_status(&handle).await.map_err(|e| ProxyError::ProviderError {
            provider: handle.provider.clone(),
            error: e.to_string(),
            retryable: e.is_retryable(),
        })?;

        // A vendor flow's next billed stage, or its new continuation state:
        // the same bookkeeping the poller applies, so whichever observer sees
        // a stage boundary first handles it — once.
        let before = handle.stage_context.clone();
        crate::proxy::model3d_stages::advance(db, provider.as_ref(), id, &mut handle, &mut poll, "client").await;
        if handle.stage_context != before {
            if let Some(job) = self.model3d_jobs.write().await.get_mut(id) {
                job.handle.stage_context = handle.stage_context.clone();
            }
        }

        // Normalise, measure and fill in the promised containers BEFORE
        // rehost, so a delivered file is stored and served like a native one:
        // the step the poller runs too, so whichever observer wins the race
        // produces the same assets.
        crate::proxy::model3d_deliver::deliver_poll(&mut poll, delivery, id).await;
        let is_terminal = matches!(
            poll.status,
            GenerationStatus::Completed | GenerationStatus::Failed | GenerationStatus::Cancelled
        );
        let files = poll.files.clone();

        // Re-host in the same call that saw completion — provider URLs expire.
        let assets = if poll.status == GenerationStatus::Completed && !files.is_empty() {
            rehost_model3d_files(
                app_store.as_ref().unwrap_or(&self.model3d_store),
                app_prefix,
                id,
                &files,
            )
                .await
                .map_err(|e| ProxyError::ProviderError {
                    provider: handle.provider.clone(),
                    error: format!("failed to store 3d assets: {e}"),
                    retryable: true,
                })?
        } else {
            Vec::new()
        };

        if is_terminal {
            self.model3d_jobs.write().await.remove(id);
        }

        let mut resp = Model3dGenerationResponse {
            id: id.to_string(),
            status: poll.status,
            model: handle.model.clone(),
            provider: handle.provider.clone(),
            assets,
            progress: poll.progress,
            error: poll.error,
            // The triple quoted at submit, not `None` — see `get_video_status`.
            usage,
            created,
        };
        enforce_requested_formats(&mut resp, &requested_formats);
        Ok(resp)
    }

    /// Drop an in-flight 3D job WITHOUT polling it, handing back what is needed
    /// to stop it at the vendor.
    ///
    /// Called when the generation is cancelled. The persisted row is terminal at
    /// that point, but the router still held the job, so the next
    /// `GET /v1/models3d/{id}` polled the provider anyway, saw `Completed`,
    /// re-hosted the assets and wrote `completed` over the cancelled row —
    /// i.e. cancellation did not stop the job, it just delayed it by one poll.
    /// `None` for an already-terminal or poller-owned generation, which is not
    /// an error.
    pub async fn forget_model3d_job(
        &self,
        id: &str,
    ) -> Option<(Model3dGenerationHandle, Arc<dyn crate::providers::Model3dProvider>)> {
        self.model3d_jobs.write().await.remove(id).map(|j| (j.handle, j.provider))
    }

    /// Video's counterpart to [`forget_model3d_job`](Self::forget_model3d_job).
    pub async fn forget_video_job(&self, id: &str) -> bool {
        self.video_jobs.write().await.remove(id).is_some()
    }

    /// Provider job id for a submitted 3D generation (for the DB row).
    pub async fn get_model3d_provider_job_id(&self, local_id: &str) -> Option<String> {
        self.model3d_jobs.read().await.get(local_id).map(|j| j.handle.provider_job_id.clone())
    }

    /// The provider's opaque continuation state for a submitted 3D generation.
    ///
    /// Persisted onto the row alongside `provider_job_id`, because the poller
    /// rebuilds the handle from the row and cannot see this map — a restart, or
    /// simply the poller winning the race, would otherwise hand the adapter a
    /// handle missing whatever it needs to poll with. `poller.rs` has always
    /// read `metadata.stage_context`; until now nothing wrote it.
    pub async fn get_model3d_stage_context(&self, local_id: &str) -> Option<serde_json::Value> {
        self.model3d_jobs.read().await.get(local_id).and_then(|j| j.handle.stage_context.clone())
    }

    pub async fn has_model3d_provider(&self, name: &str) -> bool {
        self.registry.model3d_provider_for(name).await.is_some()
    }

    /// Estimate the cost of a 3D generation without dispatching it.
    ///
    /// Mirrors [`estimate_video_cost`](Self::estimate_video_cost) in both
    /// respects that matter to a consumer calling `/v1/models3d/cost` before
    /// every generation:
    ///
    /// * `app_creds` resolves the provider the same way `generate_model3d`
    ///   does, so a single-tenant install whose 3D vendor key is stored as an
    ///   ORG credential (no platform default) gets an estimate instead of a
    ///   424 for a model it can generate with.
    /// * the configured markup is applied, so the estimate matches what the
    ///   generation actually bills.
    #[tracing::instrument(skip(self, schema, request, app_creds), fields(model = %schema.id))]
    pub async fn estimate_model3d_cost(
        &self,
        schema: &ModelSchema,
        request: &Model3dGenerationRequest,
        app_creds: Option<ProviderCredentials>,
    ) -> Result<CostEstimate, ProxyError> {
        let provider = self
            .registry
            .model3d_provider_for_request(&schema.provider, app_creds)
            .await
            .ok_or_else(|| ProxyError::ProviderNotConfigured(schema.provider.clone()))?;
        let mut est = provider.estimate_cost(schema, request).await.map_err(|e| {
            ProxyError::ProviderError {
                provider: schema.provider.clone(),
                error: e.to_string(),
                retryable: e.is_retryable(),
            }
        })?;

        price_estimate(&mut est, schema, &request.base, request, self.cost_markup_percent());
        Ok(est)
    }

    /// Dispatch a video request through a configured model route.
    async fn execute_route_video(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
        route: &ModelRoute,
        app_creds: Option<ProviderCredentials>,
    ) -> Result<(String, VideoGenerationHandle), ProxyError> {
        match route.strategy {
            RoutingStrategy::Fallback => {
                self.fallback_video(schema, base, extras, materialized, &route.deployments, app_creds).await
            }
            RoutingStrategy::WeightedRoundRobin => {
                self.weighted_video(schema, base, extras, materialized, &route.deployments, app_creds).await
            }
            RoutingStrategy::LowestCost => {
                self.lowest_cost_video(schema, base, extras, materialized, &route.deployments, app_creds).await
            }
            RoutingStrategy::LowestLatency => {
                self.lowest_latency_video(schema, base, extras, materialized, &route.deployments, app_creds).await
            }
        }
    }

    /// Try video deployments in order, falling back on retryable errors.
    async fn fallback_video(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
        deployments: &[Deployment],
        app_creds: Option<ProviderCredentials>,
    ) -> Result<(String, VideoGenerationHandle), ProxyError> {
        let mut last_error: Option<ProviderError> = None;

        for deployment in deployments {
            // ── Circuit breaker: skip provider if breaker is open ──
            if self.circuit_breaker.is_open(&deployment.provider).await {
                warn!(
                    provider = %deployment.provider,
                    "Circuit breaker open — skipping video deployment"
                );
                last_error = Some(ProviderError::NotConfigured(deployment.provider.clone()));
                continue;
            }

            let provider = match self.registry.video_provider_for_request(&deployment.provider, app_creds.clone()).await {
                Some(p) => p,
                None => {
                    warn!(provider = %deployment.provider, "Video provider not configured, skipping deployment");
                    last_error = Some(ProviderError::NotConfigured(deployment.provider.clone()));
                    continue;
                }
            };

            for attempt in 0..=deployment.max_retries {
                if attempt > 0 {
                    info!(
                        provider = %deployment.provider,
                        attempt = attempt,
                        "Retrying video generation"
                    );
                }
                let attempt_start = Instant::now();
                match tokio::time::timeout(
                    std::time::Duration::from_secs(deployment.timeout_seconds),
                    provider.generate(schema, base, extras, materialized),
                ).await {
                    Ok(Ok(handle)) => {
                        self.record_latency(&deployment.provider, attempt_start.elapsed().as_millis() as u64).await;
                        self.circuit_breaker.record_success(&deployment.provider).await;
                        return Ok((deployment.provider.clone(), handle));
                    }
                    // A submit that timed out may have been accepted: neither
                    // this deployment nor the next is sent it again.
                    Ok(Err(ProviderError::Timeout { .. })) | Err(_) => {
                        self.circuit_breaker.record_failure(&deployment.provider).await;
                        return Err(ProxyError::VendorTimeout {
                            model: schema.id.clone(),
                            provider: deployment.provider.clone(),
                            vendor_job_id: None,
                        });
                    }
                    Ok(Err(e)) => {
                        warn!(
                            provider = %deployment.provider,
                            error = %e,
                            "Video provider error"
                        );
                        self.circuit_breaker.record_failure(&deployment.provider).await;
                        let resend = may_resend(&e, false);
                        last_error = Some(e);
                        if !resend {
                            break;
                        }
                    }
                }
            }
        }

        Err(failure(&schema.id, &schema.provider, last_error, None))
    }

    /// Weighted round-robin for video deployments.
    async fn weighted_video(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
        deployments: &[Deployment],
        app_creds: Option<ProviderCredentials>,
    ) -> Result<(String, VideoGenerationHandle), ProxyError> {
        if deployments.is_empty() {
            return Err(ProxyError::NoDeployments { model: schema.id.clone() });
        }

        let mut schedule: Vec<usize> = Vec::new();
        for (i, d) in deployments.iter().enumerate() {
            for _ in 0..d.weight.max(1) {
                schedule.push(i);
            }
        }

        let slot = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            base.prompt.hash(&mut hasher);
            schema.id.hash(&mut hasher);
            hasher.finish() as usize
        };
        let primary_idx = schedule[slot % schedule.len()];

        let mut ordered: Vec<&Deployment> = Vec::with_capacity(deployments.len());
        ordered.push(&deployments[primary_idx]);
        for (i, d) in deployments.iter().enumerate() {
            if i != primary_idx {
                ordered.push(d);
            }
        }
        let ordered_owned: Vec<Deployment> = ordered.into_iter().cloned().collect();
        self.fallback_video(schema, base, extras, materialized, &ordered_owned, app_creds).await
    }

    /// Sort video deployments by estimated cost (ascending) then fall back in that order.
    async fn lowest_cost_video(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
        deployments: &[Deployment],
        app_creds: Option<ProviderCredentials>,
    ) -> Result<(String, VideoGenerationHandle), ProxyError> {
        let dummy_req = VideoGenerationRequest {
            base: base.clone(),
            duration_seconds: Some(extras.duration_seconds),
            aspect_ratio: extras.aspect_ratio.clone(),
            resolution: extras.resolution.clone(),
            fps: extras.fps,
            camera: None, audio: None, r#loop: None,
            source_video: None, direction: None, time_range: None, replace: None,
        };
        let mut cost_sorted: Vec<(f64, &Deployment)> = Vec::new();
        for d in deployments {
            let cost = if let Some(p) = self.registry.video_provider_for_request(&d.provider, app_creds.clone()).await {
                p.estimate_cost(schema, &dummy_req)
                    .await
                    .map(|c| c.total_cost_usd)
                    .unwrap_or(f64::MAX)
            } else {
                f64::MAX
            };
            cost_sorted.push((cost, d));
        }
        cost_sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let sorted: Vec<Deployment> = cost_sorted.into_iter().map(|(_, d)| d.clone()).collect();
        self.fallback_video(schema, base, extras, materialized, &sorted, app_creds).await
    }

    /// Sort video deployments by average measured latency (ascending), fall back in that order.
    async fn lowest_latency_video(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
        deployments: &[Deployment],
        app_creds: Option<ProviderCredentials>,
    ) -> Result<(String, VideoGenerationHandle), ProxyError> {
        let mut lat_sorted: Vec<(u64, &Deployment)> = Vec::new();
        for d in deployments {
            let avg = self.avg_latency(&d.provider).await;
            lat_sorted.push((avg, d));
        }
        lat_sorted.sort_by_key(|(avg, _)| *avg);
        let sorted: Vec<Deployment> = lat_sorted.into_iter().map(|(_, d)| d.clone()).collect();
        self.fallback_video(schema, base, extras, materialized, &sorted, app_creds).await
    }

    // ─── Cost Estimation ────────────────────────────────────────────────

    /// Estimate cost for an image generation request.
    /// Uses the provider declared in `schema.provider` from the capability registry.
    pub async fn estimate_image_cost(
        &self,
        schema: &ModelSchema,
        request: &ImageGenerationRequest,
        app_creds: Option<ProviderCredentials>,
    ) -> Result<CostEstimate, ProxyError> {
        let provider = self.registry
            .image_provider_for_request(&schema.provider, app_creds)
            .await
            .ok_or_else(|| ProxyError::ProviderNotConfigured(schema.provider.clone()))?;

        let mut est = provider
            .estimate_cost(schema, request)
            .await
            .map_err(|e| ProxyError::ProviderError {
                provider: schema.provider.clone(),
                error: e.to_string(),
                retryable: false,
            })?;

        price_estimate(&mut est, schema, &request.base, request, self.cost_markup_percent());
        Ok(est)
    }

    /// Estimate cost for a video generation request.
    /// Uses the provider declared in `schema.provider` from the capability registry.
    pub async fn estimate_video_cost(
        &self,
        schema: &ModelSchema,
        request: &VideoGenerationRequest,
        app_creds: Option<ProviderCredentials>,
    ) -> Result<CostEstimate, ProxyError> {
        let provider = self.registry
            .video_provider_for_request(&schema.provider, app_creds)
            .await
            .ok_or_else(|| ProxyError::ProviderNotConfigured(schema.provider.clone()))?;

        let mut est = provider
            .estimate_cost(schema, request)
            .await
            .map_err(|e| ProxyError::ProviderError {
                provider: schema.provider.clone(),
                error: e.to_string(),
                retryable: false,
            })?;

        price_estimate(&mut est, schema, &request.base, request, self.cost_markup_percent());
        Ok(est)
    }

    // ─── Accessor helpers ───────────────────────────────────────────────

    /// Return the `provider_job_id` for an in-flight video job, if present.
    /// Used by the HTTP handler to persist the job id into the generations table.
    pub async fn get_provider_job_id(&self, local_id: &str) -> Option<String> {
        self.video_jobs
            .read()
            .await
            .get(local_id)
            .map(|j| j.handle.provider_job_id.clone())
    }

    /// Whether a global image provider instance is registered for `name`.
    /// Used by the HTTP layer to decide whether a missing per-app BYO credential
    /// should fall back to the platform default or surface `provider_not_configured`.
    pub async fn has_image_provider(&self, name: &str) -> bool {
        self.registry.image_provider_for(name).await.is_some()
    }

    /// Whether a global video provider instance is registered for `name`.
    /// See [`has_image_provider`](Self::has_image_provider).
    pub async fn has_video_provider(&self, name: &str) -> bool {
        self.registry.video_provider_for(name).await.is_some()
    }

    // ─── Helpers ────────────────────────────────────────────────────────

    fn find_model_route(&self, model: &str) -> Option<ModelRoute> {
        for route_cfg in &self.model_routes() {
            if route_matches(&route_cfg.model, model) {
                return Some(ModelRoute {
                    model: route_cfg.model.clone(),
                    deployments: route_cfg
                        .deployments
                        .iter()
                        .map(|d| Deployment {
                            provider: d.provider.clone(),
                            weight: d.weight,
                            max_retries: d.max_retries,
                            timeout_seconds: d.timeout_seconds,
                            rpm_limit: d.rpm_limit,
                            respect_health: true,
                        })
                        .collect(),
                    strategy: match route_cfg.strategy.as_deref() {
                        Some("weighted_round_robin") => RoutingStrategy::WeightedRoundRobin,
                        Some("lowest_cost") => RoutingStrategy::LowestCost,
                        Some("lowest_latency") => RoutingStrategy::LowestLatency,
                        _ => RoutingStrategy::Fallback,
                    },
                    cache: route_cfg.cache.as_ref().map(|c| CacheConfig {
                        enabled: c.enabled,
                        ttl_seconds: c.ttl_seconds,
                        max_items: 1000,
                    }),
                });
            }
        }
        None
    }
}

/// Match a model pattern against a model ID.
/// Supports exact match, prefix/* glob, and * (match all).
/// One output's price for a request about to run, from the extras built for
/// it (3D doc §3G): the same rules and facts the quote and the reservation use.
fn unit_price(schema: &ModelSchema, base: &BaseGenerationRequest, extras: &impl serde::Serialize) -> f64 {
    let params = serde_json::to_value(extras).unwrap_or_default();
    crate::capabilities::pricing::unit_price_usd(schema, &crate::capabilities::pricing::PriceFacts::new(schema, base, &params))
}

/// A quote at the request's own price: the row's rules over the request,
/// then litegen's markup — what the reservation holds and the charge takes.
fn price_estimate(
    est: &mut CostEstimate,
    schema: &ModelSchema,
    base: &BaseGenerationRequest,
    request: &impl serde::Serialize,
    markup_percent: f64,
) {
    let params = serde_json::to_value(request).unwrap_or_default();
    est.base_cost_usd = crate::capabilities::pricing::request_price_usd(schema, base, &params);
    let (markup, total) = apply_markup(est.base_cost_usd, markup_percent);
    est.markup_usd = markup;
    est.total_cost_usd = total;
    est.tokens_required = crate::providers::usd_to_tokens(total, USD_PER_TOKEN);
}

fn route_matches(pattern: &str, model: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/*") {
        return model.starts_with(prefix);
    }
    pattern.eq_ignore_ascii_case(model)
}

async fn build_image_results(
    output: &GenerationOutput,
    extras: &ImageExtras,
    image_store: &Arc<dyn ImageStore>,
) -> Vec<ImageResult> {
    let revised_prompt = output
        .metadata
        .get("revised_prompt")
        .and_then(|v| v.as_str())
        .map(String::from);

    // The call's first image, then any more it made (a multiview model's
    // views), each stored and returned as its own result.
    let mut results = Vec::with_capacity(1 + output.more.len());
    for (index, image) in std::iter::once(output).chain(output.more.iter()).enumerate() {
        let generation_id = uuid::Uuid::new_v4().to_string();
        // The bytes say what they are; an adapter's label can be wrong (L9).
        let content_type = image_content_type(&image.data).map_or_else(|| image.content_type.clone(), str::to_string);
        // Try to upload to configured storage backend (S3, etc.)
        let stored_url = image_store.store(&image.data, &content_type, &generation_id).await.ok();
        let encode = || base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &image.data);
        let (url, b64_json) = match stored_url {
            // Image was uploaded to object storage — return URL
            Some(url) if extras.response_format == "b64_json" => (Some(url), Some(encode())),
            Some(url) => (Some(url), None),
            // No storage configured — return base64 inline
            None => (None, Some(encode())),
        };
        let view = image.metadata.get("view").and_then(|v| v.as_str()).map(String::from);
        let yaw_deg = image.metadata.get("yaw_deg").and_then(|v| v.as_f64());
        // The size it came back at, from its header (3D doc §4: an expanded
        // canvas is reported as delivered).
        let size = image::ImageReader::new(std::io::Cursor::new(&image.data))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok());
        results.push(ImageResult {
            url,
            b64_json,
            revised_prompt: revised_prompt.clone(),
            view,
            yaw_deg,
            content_type,
            width: size.map(|s| s.0),
            height: size.map(|s| s.1),
            index: index as u32,
        });
    }
    results
}

/// Every image of a call with the init pasted back outside the edit area (3D
/// doc §4); an image litegen cannot read fails the call as the vendor's.
async fn preserve_output(mut output: GenerationOutput, p: crate::proxy::preserve::Preserve, schema: &ModelSchema) -> Result<GenerationOutput, ProxyError> {
    let content_type = if p.webp { "image/webp" } else { "image/png" };
    let joined = tokio::task::spawn_blocking(move || -> Result<GenerationOutput, image::ImageError> {
        output.data = crate::proxy::preserve::composite(&output.data, &p)?;
        output.content_type = content_type.into();
        for more in &mut output.more {
            more.data = crate::proxy::preserve::composite(&more.data, &p)?;
            more.content_type = content_type.into();
        }
        Ok(output)
    })
    .await
    .map_err(|e| ProxyError::Internal(format!("preservation stopped: {e}")))?;
    joined.map_err(|e| {
        let unreadable = ProviderError::RequestFailed {
            message: format!("returned an image litegen cannot read to preserve: {e}"),
            status_code: None,
            provider_error: None,
            retryable: false,
        };
        ProxyError::VendorFailed { model: schema.id.clone(), provider: schema.provider.clone(), vendor: VendorError::from_provider(&unreadable), vendor_job_id: None }
    })
}

/// An image's content type read from its first bytes (PixelGenie L9): PNG,
/// JPEG, WebP or GIF, else `None` and the adapter's label stands.
pub fn image_content_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else {
        None
    }
}

/// Upload every file a 3D provider returned to litegen storage and describe them
/// as `Model3dAsset`s.
///
/// Called from BOTH the router (when a provider completes inline) and the poller
/// (the usual case), which is why it is a free function. It must run in the same
/// tick that observed completion: several vendors expire their download URLs
/// within minutes of task success, and re-hosting is also what makes an app's
/// BYO bucket apply to meshes exactly as it does to images.
///
/// The mesh is always keyed `model.<ext>`, so a client can construct the primary
/// URL without parsing the asset list. Keys are uniqued per `(stem, format)`,
/// not per stem: a generation that was asked for glb AND fbx returns two meshes
/// whose extensions already distinguish them, and suffixing the second to
/// `model_1.fbx` would break the documented `model.<ext>` promise for every
/// container but the first one the adapter happened to push.
pub async fn rehost_model3d_files(
    store: &Arc<dyn crate::proxy::storage::ImageStorage>,
    path_prefix: Option<&str>,
    generation_id: &str,
    files: &[crate::providers::Model3dFile],
) -> Result<Vec<Model3dAsset>, crate::proxy::storage::ImageStoreError> {
    use crate::proxy::storage::{model3d_asset_key, model3d_content_type, sanitize_model3d_format};

    let mut seen: HashMap<(&'static str, String), u32> = HashMap::new();
    let mut assets = Vec::with_capacity(files.len());

    for f in files {
        let stem: &'static str = match f.kind {
            Model3dAssetKind::Mesh => "model",
            Model3dAssetKind::Texture => "texture",
            Model3dAssetKind::Preview => "preview",
            Model3dAssetKind::Original => "original",
        };
        // The vendor's spelling reaches a storage key and a public URL, so it is
        // normalised here rather than trusted.
        let format = sanitize_model3d_format(&f.format);
        let n = seen.entry((stem, format.clone())).or_insert(0);
        let name = if *n == 0 { stem.to_string() } else { format!("{stem}_{n}") };
        *n += 1;

        let key = model3d_asset_key(path_prefix, generation_id, &name, &format);
        let url = store
            .put(&key, &bytes::Bytes::from(f.bytes.clone()), model3d_content_type(&format, &f.content_type))
            .await?;

        assets.push(Model3dAsset {
            kind: f.kind,
            url,
            format,
            size_bytes: Some(f.bytes.len() as u64),
            polycount: f.polycount,
            width: f.width,
            height: f.height,
            sha256: Some(sha256_hex(&f.bytes)),
            scene: f.manifest.clone(),
        });
    }
    Ok(assets)
}

/// Lowercase hex SHA-256: what every stored asset reports, so a client can
/// check what it downloaded.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// How a stored video was named, and what a generation row records about it.
pub struct StoredVideo {
    pub url: String,
    pub asset: GenerationAsset,
}

/// Largest vendor video litegen downloads to keep.
const MAX_VIDEO_BYTES: usize = 1024 * 1024 * 1024;

/// Keep a finished video on litegen storage (PixelGenie L3): the vendor's own
/// URL may expire within the hour (Replicate), need the vendor's key (Veo),
/// or vanish with the job. Uses the bytes the adapter already downloaded when
/// it has them, else fetches `url`, which must be a public http(s) URL.
pub async fn rehost_video(
    store: &Arc<dyn crate::proxy::storage::ImageStorage>,
    path_prefix: Option<&str>,
    generation_id: &str,
    data: Option<Vec<u8>>,
    url: Option<&str>,
    content_type: Option<&str>,
) -> Result<StoredVideo, String> {
    let (bytes, ct) = match data {
        Some(b) => (b, content_type.unwrap_or("video/mp4").to_string()),
        None => {
            let url = url.ok_or("the vendor returned neither the video nor its URL")?;
            crate::util::ssrf::validate_public_url(url).await?;
            let client = crate::util::ssrf::no_redirect_download_client(
                std::time::Duration::from_secs(600),
                std::time::Duration::from_secs(60),
            );
            let resp = client.get(url).send().await.map_err(|e| format!("download failed: {}", e.without_url()))?;
            if !resp.status().is_success() {
                return Err(format!("download answered HTTP {}", resp.status()));
            }
            if resp.content_length().is_some_and(|n| n as usize > MAX_VIDEO_BYTES) {
                return Err("the video is larger than litegen keeps".into());
            }
            let ct = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
                .or_else(|| content_type.map(str::to_string))
                .unwrap_or_else(|| "video/mp4".into());
            let bytes = resp.bytes().await.map_err(|e| format!("download failed: {}", e.without_url()))?;
            if bytes.len() > MAX_VIDEO_BYTES {
                return Err("the video is larger than litegen keeps".into());
            }
            (bytes.to_vec(), ct)
        }
    };
    let format = match ct.split(';').next().unwrap_or("").trim() {
        "image/gif" => "gif",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        _ => "mp4",
    };
    let key = format!(
        "{}/{generation_id}/video.{format}",
        path_prefix.unwrap_or(crate::proxy::storage::VIDEO_PATH_PREFIX).trim_matches('/')
    );
    // What came back, read from the bytes (PixelGenie L10).
    let media = crate::proxy::video_probe::probe(&bytes);
    let url = store
        .put(&key, &bytes::Bytes::from(bytes.clone()), crate::proxy::storage::model3d_content_type(format, &ct))
        .await
        .map_err(|e| format!("could not store the video: {e}"))?;
    Ok(StoredVideo {
        asset: GenerationAsset {
            index: 0,
            kind: "video".into(),
            format: format.into(),
            url: url.clone(),
            size_bytes: Some(bytes.len() as u64),
            sha256: Some(sha256_hex(&bytes)),
            width: media.width,
            height: media.height,
            duration_s: media.duration_s,
            fps: media.fps,
            has_audio: media.has_audio,
        },
        url,
    })
}

/// Demote a `completed` 3D response that is missing a container the caller was
/// promised.
///
/// A partial delivery is a failure, not a degraded success: the caller asked for
/// `["glb", "fbx"]` because something downstream needs the fbx, and reporting
/// `completed` would bill them for a result they cannot use. Assets are cleared
/// for the same reason the mesh guard clears them — a failed generation must not
/// look half-usable.
///
/// This runs at EVERY observer of completion. There are three
/// (`get_model3d_status` here, the poller, and `model3d_response_from_row`), and
/// a check in fewer than all of them means the observer that wins the race
/// decides whether the generation succeeded.
pub fn enforce_requested_formats(resp: &mut Model3dGenerationResponse, requested: &[String]) {
    if resp.status != GenerationStatus::Completed {
        return;
    }
    let delivered = resp.delivered_formats();
    // No mesh at all is the mesh guard's diagnosis, not this one. Both are true
    // of an empty result, but "reported success without a mesh asset" says what
    // actually happened; "did not return glb" reads as a format quibble about a
    // generation that produced nothing. Leaving it to the caller-side guard also
    // keeps that error string — which consumers match on — stable.
    if delivered.is_empty() {
        return;
    }
    let missing = crate::types::missing_model3d_formats(requested, &delivered);
    if missing.is_empty() {
        return;
    }
    resp.status = GenerationStatus::Failed;
    resp.error = Some(format!(
        "provider did not return the requested format(s): {}",
        missing.join(", ")
    ));
    resp.assets.clear();
}

// ─── Proxy Error ────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("Model not found: {model}")]
    ModelNotFound { model: String },

    #[error("Provider not configured: {0}")]
    ProviderNotConfigured(String),

    #[error("No deployments configured for model: {model}")]
    NoDeployments { model: String },

    #[error("All deployments failed for model {model}: {last_error:?}")]
    AllDeploymentsFailed {
        model: String,
        last_error: Option<String>,
    },

    #[error("Provider {provider} error: {error}")]
    ProviderError {
        provider: String,
        error: String,
        retryable: bool,
    },

    /// The vendor refused the request (its 4xx, PixelGenie L15): sending it
    /// unchanged again will not help.
    #[error("{provider} rejected the request for model {model}: {}{}", vendor.message, vendor_job_id.as_ref().map(|j| format!(" (vendor job {j})")).unwrap_or_default())]
    VendorRejected {
        model: String,
        provider: String,
        vendor: VendorError,
        vendor_job_id: Option<String>,
    },

    /// The vendor failed (a 5xx, a 429, or no answer at all): the same
    /// request may work later.
    #[error("{provider} failed for model {model}: {}{}", vendor.message, vendor_job_id.as_ref().map(|j| format!(" (vendor job {j})")).unwrap_or_default())]
    VendorFailed {
        model: String,
        provider: String,
        vendor: VendorError,
        vendor_job_id: Option<String>,
    },

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Not found: {0}")]
    NotFound(String),

    /// The vendor did not answer in time. Never re-sent: it may still run the
    /// job, and bill it. Carries the vendor's job id when it had accepted one.
    #[error("{} did not answer for model {model} in time{}", provider, vendor_job_id.as_ref().map(|j| format!(" (vendor job {j})")).unwrap_or_default())]
    VendorTimeout {
        model: String,
        provider: String,
        vendor_job_id: Option<String>,
    },
}

impl ProxyError {
    pub fn status_code(&self) -> u16 {
        match self {
            Self::ModelNotFound { .. } => 404,
            // The model's provider has no usable credential (no per-app BYO key and
            // no platform default). This is a client/config problem, not a transient
            // outage — surface a 424 rather than a 5xx so callers don't blindly retry.
            Self::ProviderNotConfigured(_) => 424,
            Self::NoDeployments { .. } => 503,
            Self::AllDeploymentsFailed { .. } => 502,
            Self::ProviderError { retryable, .. } => {
                if *retryable { 502 } else { 400 }
            }
            Self::Internal(_) => 500,
            Self::NotFound(_) => 404,
            Self::VendorTimeout { .. } => 504,
            // Its refusal of litegen's own credential is not the caller's to fix.
            Self::VendorRejected { vendor, .. } if matches!(vendor.status, Some(401 | 403)) => 424,
            Self::VendorRejected { .. } => 400,
            Self::VendorFailed { vendor, .. } if vendor.status == Some(429) => 429,
            Self::VendorFailed { .. } => 502,
        }
    }

    /// Whether this failure happened after a vendor was called — i.e. whether
    /// the vendor may have done (and billed) work. An `Idempotency-Key`
    /// records such an outcome; a refusal before any vendor call does not
    /// spend the key.
    pub fn reached_vendor(&self) -> bool {
        match self {
            Self::ModelNotFound { .. } | Self::ProviderNotConfigured(_) | Self::NoDeployments { .. } => false,
            Self::AllDeploymentsFailed { .. }
            | Self::ProviderError { .. }
            | Self::VendorTimeout { .. }
            | Self::VendorRejected { .. }
            | Self::VendorFailed { .. } => true,
            Self::Internal(_) | Self::NotFound(_) => false,
        }
    }
}

/// What a vendor said when it refused or failed (PixelGenie L15): its HTTP
/// status, its own error code and its message, read from the error shapes
/// vendors use (`{"error": {"message", "code"}}`, fal's `detail` list,
/// `{"code", "message"}` envelopes).
pub use crate::types::VendorError;

impl VendorError {
    fn from_provider(err: &ProviderError) -> Self {
        let (status, body, fallback) = match err {
            ProviderError::RequestFailed { status_code, provider_error, message, .. } => (*status_code, provider_error.clone(), message.clone()),
            ProviderError::RateLimited { .. } => (Some(429), None, err.to_string()),
            other => (None, None, other.to_string()),
        };
        let body = body.unwrap_or(serde_json::Value::Null);
        let text = |v: &serde_json::Value| match v {
            serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        };
        let first_detail = &body["detail"][0];
        let code = [&body["error"]["code"], &body["error"]["type"], &first_detail["type"], &body["code"], &body["error_code"], &body["type"]]
            .into_iter()
            .find_map(text)
            .filter(|c| c != "0");
        let message = [&body["error"]["message"], &first_detail["msg"], &body["detail"], &body["message"], &body["msg"], &body["error"]]
            .into_iter()
            .find_map(text)
            .unwrap_or(fallback);
        Self { status, code, message }
    }

    /// Whether the same request may work later: no answer, a 5xx, or a 429.
    pub fn retryable(&self) -> bool {
        self.status.is_none_or(|s| s >= 500 || s == 429 || s == 408)
    }
}

/// The error a caller sees once every attempt has failed (PixelGenie L15): a
/// vendor's 4xx is its refusal, a 5xx or a dropped connection its failure, a
/// timeout a timeout, and a refusal before any call stays the adapter's.
fn failure(model: &str, provider: &str, last: Option<ProviderError>, vendor_job_id: Option<String>) -> ProxyError {
    let Some(err) = last else {
        return ProxyError::AllDeploymentsFailed { model: model.to_string(), last_error: None };
    };
    let (model, provider) = (model.to_string(), provider.to_string());
    match &err {
        ProviderError::Timeout { .. } => ProxyError::VendorTimeout { model, provider, vendor_job_id },
        ProviderError::RequestFailed { status_code: Some(s), .. } if (400..500).contains(s) && !matches!(s, 408 | 429) => {
            ProxyError::VendorRejected { model, provider, vendor: VendorError::from_provider(&err), vendor_job_id }
        }
        ProviderError::RequestFailed { .. } | ProviderError::RateLimited { .. } => {
            ProxyError::VendorFailed { model, provider, vendor: VendorError::from_provider(&err), vendor_job_id }
        }
        _ => ProxyError::ProviderError { provider, error: with_vendor_job(err.to_string(), vendor_job_id), retryable: false },
    }
}

/// Whether a failed vendor attempt may be sent again, to the same deployment
/// or the next one (PixelGenie L1).
///
/// Never after a timeout: dropping the future withdraws nothing, and the
/// vendor may still run and bill the job. Never once the vendor accepted the
/// job, which an adapter that polls says by recording its id
/// ([`crate::providers::VendorJobSink`]): a failure from then on is a broken
/// poll, not a rejected submit. Otherwise, whatever the provider error says.
fn may_resend(err: &ProviderError, vendor_accepted: bool) -> bool {
    err.is_retryable() && !vendor_accepted && !matches!(err, ProviderError::Timeout { .. })
}

/// `last_error`'s text, naming the vendor job when one was accepted.
fn with_vendor_job(error: String, vendor_job_id: Option<String>) -> String {
    match vendor_job_id {
        Some(j) => format!("{error} (vendor job {j})"),
        None => error,
    }
}

#[cfg(test)]
#[path = "router_tests.rs"]
mod router_tests;

#[cfg(test)]
mod model3d_router_tests {
    use super::*;
    use crate::providers::Model3dFile;
    use crate::proxy::storage::LocalModel3dStorage;
    use crate::types::Model3dAssetKind;

    fn mesh_file() -> Model3dFile {
        Model3dFile {
            kind: Model3dAssetKind::Mesh,
            format: "glb".into(),
            content_type: "model/gltf-binary".into(),
            bytes: b"glTF\x02\x00\x00\x00fake".to_vec(),
            polycount: Some(12),
            width: None,
            height: None,
            manifest: None,
        }
    }

    fn preview_file() -> Model3dFile {
        Model3dFile {
            kind: Model3dAssetKind::Preview,
            format: "png".into(),
            content_type: "image/png".into(),
            bytes: b"\x89PNG\r\n\x1a\n".to_vec(),
            polycount: None,
            width: Some(512),
            height: Some(512),
            manifest: None,
        }
    }

    #[tokio::test]
    async fn rehost_writes_each_file_under_a_predictable_key_and_returns_absolute_urls() {
        let store: Arc<dyn crate::proxy::storage::ImageStorage> =
            Arc::new(LocalModel3dStorage::new(tempfile::tempdir().unwrap().keep(), "https://cdn.example.com".into()));

        let assets = rehost_model3d_files(&store, None, "litegen-3d-42", &[mesh_file(), preview_file()])
            .await
            .unwrap();

        assert_eq!(assets.len(), 2);
        let mesh = assets.iter().find(|a| a.kind == Model3dAssetKind::Mesh).unwrap();
        assert_eq!(mesh.url, "https://cdn.example.com/v1/models3d/assets/litegen/3d/litegen-3d-42/model.glb");
        assert_eq!(mesh.format, "glb");
        assert_eq!(mesh.polycount, Some(12));
        assert_eq!(mesh.size_bytes, Some(mesh_file().bytes.len() as u64));

        let preview = assets.iter().find(|a| a.kind == Model3dAssetKind::Preview).unwrap();
        assert!(preview.url.ends_with("/litegen-3d-42/preview.png"), "got {}", preview.url);
        assert_eq!(preview.width, Some(512));
        assert_eq!(preview.polycount, None);

        for a in &assets {
            assert!(a.url.starts_with("https://"), "asset urls must be absolute: {}", a.url);
        }
    }

    #[tokio::test]
    async fn rehost_numbers_multiple_files_of_the_same_kind() {
        let store: Arc<dyn crate::proxy::storage::ImageStorage> =
            Arc::new(LocalModel3dStorage::new(tempfile::tempdir().unwrap().keep(), "https://cdn.example.com".into()));
        let mut a = preview_file();
        a.format = "png".into();
        let b = preview_file();
        let assets = rehost_model3d_files(&store, None, "litegen-3d-7", &[mesh_file(), a, b]).await.unwrap();
        let urls: Vec<&str> = assets.iter().map(|x| x.url.as_str()).collect();
        assert_eq!(urls.len(), 3);
        assert_eq!(
            urls.iter().collect::<std::collections::HashSet<_>>().len(),
            3,
            "keys must not collide: {urls:?}"
        );
    }

    #[tokio::test]
    async fn rehost_honours_a_per_app_path_prefix() {
        let store: Arc<dyn crate::proxy::storage::ImageStorage> =
            Arc::new(LocalModel3dStorage::new(tempfile::tempdir().unwrap().keep(), "https://cdn.example.com".into()));
        let assets = rehost_model3d_files(&store, Some("tenant-9/meshes"), "g1", &[mesh_file()]).await.unwrap();
        assert!(assets[0].url.contains("/tenant-9/meshes/g1/model.glb"), "got {}", assets[0].url);
    }

    // ─── The instance that submitted a job is the one that polls it ─────────

    async fn test_db() -> Arc<dyn crate::db::DatabaseStore> {
        Arc::new(crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap())
    }

    async fn router_with_operator_fal(server: &wiremock::MockServer) -> ProxyRouter {
        let mut config = crate::config::AppConfig::default();
        config.providers.insert(
            "fal".into(),
            serde_json::from_value(serde_json::json!({ "api_key": "operator-key", "api_base": server.uri() })).unwrap(),
        );
        let registry = Arc::new(crate::proxy::registry::ProviderRegistry::new());
        registry.init_from_config(&config).await;
        ProxyRouter::new(
            registry,
            Arc::new(crate::proxy::cache::GenerationCache::new(&crate::config::CacheGlobalConfig::default())),
            Arc::new(config),
            Arc::new(crate::proxy::storage::LocalStore),
        )
    }

    fn trellis() -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        crate::capabilities::CapabilityRegistry::from_dir(&p).unwrap().get("fal/trellis").unwrap().clone()
    }

    fn org_key() -> crate::providers::ProviderCredentials {
        crate::providers::ProviderCredentials { api_key: Some("org-key".into()), ..Default::default() }
    }

    async fn submit_byo(router: &ProxyRouter) -> Model3dGenerationResponse {
        let base = crate::types::BaseGenerationRequest {
            prompt: String::new(),
            model: "fal/trellis".into(),
            n: 1,
            negative_prompt: None,
            seed: None,
            reference_images: vec![],
            strict: true,
            extra: None,
            metadata: None,
        };
        let extras = crate::providers::Model3dExtras {
            output_formats: vec!["glb".into()],
            texture: None, pbr: None, target_polycount: None,
            symmetry: None, topology: None, rig: None, extra: None,
            source: None,
            animations: Vec::new(), fps: None,
            skeleton: None, joint_naming: None, height_m: None, preflight: None,
            texture_resolution: None, keep_uvs: None,
            ground: None, center: None,
        };
        let refs = crate::proxy::materializer::MaterializedRequest {
            refs: vec![crate::proxy::materializer::MaterializedRef {
                role: "init".into(),
                form: crate::proxy::materializer::MaterializedRefForm::Url("https://example.com/robot.png".into()),
                time_s: None,
            }],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };
        router.generate_model3d(&trellis(), &base, &extras, &refs, Some(org_key())).await.expect("submit")
    }

    #[tokio::test]
    async fn a_byo_job_is_polled_with_the_credential_that_submitted_it() {
        // Polling through the platform-default instance asked fal about an org's
        // job with the OPERATOR's key, so `GET /v1/models3d/{id}` could never
        // advance a BYO job. fal knows a request only under the key that made it.
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let job = format!("{}/fal-ai/trellis/requests/byo-1", server.uri());
        Mock::given(method("POST")).and(path("/fal-ai/trellis")).and(header("authorization", "Key org-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "request_id": "byo-1", "status_url": format!("{job}/status"), "response_url": job,
            })))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/fal-ai/trellis/requests/byo-1/status")).and(header("authorization", "Key org-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "status": "IN_PROGRESS" })))
            .mount(&server).await;

        let router = router_with_operator_fal(&server).await;
        let submitted = submit_byo(&router).await;
        let polled = router.get_model3d_status(&submitted.id, None, None, &test_db().await).await.expect("polled with the org key");
        assert_eq!(polled.status, GenerationStatus::Processing);
        assert_eq!(polled.created, submitted.created, "`created` is the submit time on every poll");

        for r in server.received_requests().await.unwrap() {
            assert_eq!(r.headers.get("authorization").unwrap(), "Key org-key", "{} went out under another key", r.url);
        }
    }

    #[tokio::test]
    async fn forgetting_a_job_hands_back_what_cancels_it_at_the_vendor() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/fal-ai/trellis"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "request_id": "byo-2" })))
            .mount(&server).await;
        Mock::given(method("PUT")).and(path("/fal-ai/trellis/requests/byo-2/cancel")).and(header("authorization", "Key org-key"))
            .respond_with(ResponseTemplate::new(202).set_body_json(serde_json::json!({ "status": "CANCELLATION_REQUESTED" })))
            .mount(&server).await;

        let router = router_with_operator_fal(&server).await;
        let submitted = submit_byo(&router).await;
        let (handle, provider) = router.forget_model3d_job(&submitted.id).await.expect("tracked");
        assert_eq!(provider.cancel(&handle).await.unwrap(), crate::providers::CancelOutcome::Requested);
        assert!(router.forget_model3d_job(&submitted.id).await.is_none(), "forgotten once");
        assert!(matches!(router.get_model3d_status(&submitted.id, None, None, &test_db().await).await, Err(ProxyError::NotFound(_))));
    }

    // ─── A submit that times out is never sent again ─────────────────────────

    /// Counts submits and never answers, like a vendor that accepted the job
    /// but whose reply never arrives.
    struct NeverAnswers(Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait::async_trait]
    impl crate::providers::Model3dProvider for NeverAnswers {
        fn name(&self) -> &str { "mock" }
        fn configure(&mut self, _c: crate::providers::ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool { true }
        async fn generate(
            &self,
            _m: &crate::capabilities::ModelSchema,
            _b: &crate::types::BaseGenerationRequest,
            _e: &crate::providers::Model3dExtras,
            _r: &crate::proxy::materializer::MaterializedRequest,
        ) -> Result<crate::providers::Model3dGenerationHandle, crate::providers::ProviderError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            unreachable!("the router times the submit out first")
        }
        async fn poll_status(
            &self,
            _h: &crate::providers::Model3dGenerationHandle,
        ) -> Result<crate::providers::Model3dGenerationPollResult, crate::providers::ProviderError> {
            unreachable!()
        }
        async fn estimate_cost(
            &self,
            _m: &crate::capabilities::ModelSchema,
            _r: &crate::types::Model3dGenerationRequest,
        ) -> Result<crate::types::CostEstimate, crate::providers::ProviderError> {
            unreachable!()
        }
        async fn health_check(&self) -> crate::providers::HealthCheckResult {
            unreachable!()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_timed_out_3d_submit_is_not_resubmitted() {
        // The vendor may already have queued (and will bill) the first submit;
        // resubmitting would start more jobs that nothing tracks.
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let registry = Arc::new(crate::proxy::registry::ProviderRegistry::new());
        registry.register_mock_model3d(Arc::new(NeverAnswers(calls.clone()))).await;
        let router = ProxyRouter::new(
            registry,
            Arc::new(crate::proxy::cache::GenerationCache::new(&crate::config::CacheGlobalConfig::default())),
            Arc::new(crate::config::AppConfig::default()),
            Arc::new(crate::proxy::storage::LocalStore),
        );
        let schema = crate::capabilities::ModelSchema {
            id: "mock/slow-3d".into(), provider: "mock".into(), media_type: crate::capabilities::MediaType::Model3d,
            task: Default::default(),
            native_orientation: None,
            preserves_unmasked: None,
            display_name: "slow".into(), description: String::new(), docs_url: None,
            pricing: crate::capabilities::ModelPricing { base_cost_usd: 0.0, variable_pricing: Vec::new() },
            capabilities: crate::capabilities::ModelCapabilityFlags { text_to_3d: true, ..Default::default() },
            prompt: crate::capabilities::PromptSpec { required: true, min_length: None, max_length: None },
            params: Default::default(), ref_inputs: None, extra_allowlist: vec![], tags: vec![],
            deprecated_at: None, retires_at: None, replaced_by: None, status: Default::default(),
        };
        let base = crate::types::BaseGenerationRequest {
            prompt: "a fox".into(), model: "mock/slow-3d".into(), n: 1, negative_prompt: None, seed: None,
            reference_images: vec![], strict: true, extra: None, metadata: None,
        };
        let extras = crate::providers::Model3dExtras {
            output_formats: vec!["glb".into()],
            texture: None, pbr: None, target_polycount: None,
            symmetry: None, topology: None, rig: None, extra: None,
            source: None,
            animations: Vec::new(), fps: None,
            skeleton: None, joint_naming: None, height_m: None, preflight: None,
            texture_resolution: None, keep_uvs: None,
            ground: None, center: None,
        };
        let refs = crate::proxy::materializer::MaterializedRequest {
            refs: vec![],
            cleanup: crate::proxy::materializer::Cleanup::empty(),
        };

        let out = router.generate_model3d(&schema, &base, &extras, &refs, None).await;
        assert!(matches!(out, Err(ProxyError::VendorTimeout { .. })), "{:?}", out.err());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1, "a timed-out submit must not be sent again");
    }
}
