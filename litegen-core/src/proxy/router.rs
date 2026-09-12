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
#[derive(Clone)]
struct Model3dJob {
    handle: Model3dGenerationHandle,
    usage: Option<UsageInfo>,
    requested_formats: Vec<String>,
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
        }
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
                let attempt_start = Instant::now();
                match tokio::time::timeout(
                    std::time::Duration::from_secs(120),
                    provider.generate(schema, base, extras, materialized),
                ).await {
                    Ok(Ok(o)) => {
                        self.record_latency(&schema.provider, attempt_start.elapsed().as_millis() as u64).await;
                        output = Some(o);
                        break;
                    }
                    Ok(Err(e)) => {
                        let retryable = e.is_retryable();
                        last_error = Some(e);
                        if !retryable { break; }
                    }
                    Err(_) => {
                        last_error = Some(ProviderError::Timeout { timeout_ms: 120_000 });
                    }
                }
            }

            let o = output.ok_or_else(|| {
                let err_str = last_error.map(|e| e.to_string());
                ProxyError::AllDeploymentsFailed {
                    model: schema.id.clone(),
                    last_error: err_str,
                }
            })?;
            (schema.provider.clone(), o)
        };

        let latency_ms = start.elapsed().as_millis() as u64;

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
        let produced = data.len().max(1) as f64;
        let base_cost = schema.pricing.base_cost_usd * produced;
        let (markup, total) = apply_markup(base_cost, self.config.cost_markup_percent);
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
                    Ok(Err(e)) => {
                        warn!(
                            provider = %deployment.provider,
                            error = %e,
                            retryable = e.is_retryable(),
                            "Image provider error"
                        );
                        self.circuit_breaker.record_failure(&deployment.provider).await;
                        if !e.is_retryable() {
                            last_error = Some(e);
                            break; // Skip to next deployment
                        }
                        last_error = Some(e);
                    }
                    Err(_) => {
                        warn!(
                            provider = %deployment.provider,
                            timeout_s = deployment.timeout_seconds,
                            "Image request timed out"
                        );
                        self.circuit_breaker.record_failure(&deployment.provider).await;
                        last_error = Some(ProviderError::Timeout {
                            timeout_ms: deployment.timeout_seconds * 1000,
                        });
                    }
                }
            }
        }

        Err(ProxyError::AllDeploymentsFailed {
            model: schema.id.clone(),
            last_error: last_error.map(|e| e.to_string()),
        })
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
                    std::time::Duration::from_secs(120),
                    provider.generate(schema, base, extras, materialized),
                ).await {
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
                        last_error = Some(ProviderError::Timeout { timeout_ms: 120_000 });
                    }
                }
            }

            let h = handle.ok_or_else(|| {
                let err_str = last_error.map(|e| e.to_string());
                ProxyError::AllDeploymentsFailed {
                    model: schema.id.clone(),
                    last_error: err_str,
                }
            })?;
            (schema.provider.clone(), h)
        };

        // Build cost from schema pricing. This path submits exactly one
        // provider job and returns exactly one video (a single video_url), so
        // billing must reflect the one video delivered — NOT the requested `n`,
        // which is meaningless for video and unbounded on the wire. (Same
        // reasoning as the image path above, which bills per delivered image.)
        let produced = 1.0;
        let base_cost = schema.pricing.base_cost_usd * produced;
        let (_, total) = apply_markup(base_cost, self.config.cost_markup_percent);
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
        })
    }

    /// Look up an in-flight video generation by local ID and poll its provider.
    #[tracing::instrument(skip(self), fields(id = %id))]
    pub async fn get_video_status(&self, id: &str) -> Result<VideoGenerationResponse, ProxyError> {
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

        if matches!(
            poll.status,
            GenerationStatus::Completed | GenerationStatus::Failed | GenerationStatus::Cancelled
        ) {
            self.video_jobs.write().await.remove(id);
        }

        Ok(VideoGenerationResponse {
            id: id.to_string(),
            status: poll.status,
            model: handle.model.clone(),
            provider: handle.provider.clone(),
            video_url: poll.video_url,
            progress: poll.progress,
            error: poll.error,
            // The triple quoted at submit, not `None`: the consumer's cost path
            // reads `usage` off the poll response it actually waits for.
            usage,
            created: chrono::Utc::now().timestamp(),
        })
    }

    // ─── 3D Model Generation ────────────────────────────────────────────

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
                Err(_) => last_error = Some(ProviderError::Timeout { timeout_ms: 120_000 }),
            }
        }

        let handle = handle.ok_or_else(|| ProxyError::AllDeploymentsFailed {
            model: schema.id.clone(),
            last_error: last_error.map(|e| e.to_string()),
        })?;

        // One submitted job produces one mesh, so billing is flat per generation
        // — `n` is meaningless here, exactly as it is for video.
        let base_cost = schema.pricing.base_cost_usd;
        let (_, total) = apply_markup(base_cost, self.config.cost_markup_percent);
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
        self.model3d_jobs.write().await.insert(
            local_id.clone(),
            Model3dJob { handle, usage: usage_info.clone(), requested_formats },
        );

        Ok(Model3dGenerationResponse {
            id: local_id,
            status: GenerationStatus::Pending,
            model: schema.id.clone(),
            provider: schema.provider.clone(),
            assets: Vec::new(),
            progress: 0,
            error: None,
            usage: usage_info,
            created: chrono::Utc::now().timestamp(),
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
    #[tracing::instrument(skip(self, app_store), fields(id = %id))]
    pub async fn get_model3d_status(
        &self,
        id: &str,
        app_store: Option<Arc<dyn crate::proxy::storage::ImageStorage>>,
        app_prefix: Option<&str>,
    ) -> Result<Model3dGenerationResponse, ProxyError> {
        let job = {
            let jobs = self.model3d_jobs.read().await;
            jobs.get(id).cloned()
        };
        let Model3dJob { handle, usage, requested_formats } =
            job.ok_or_else(|| ProxyError::NotFound(format!("3d job '{}' not found", id)))?;

        let provider = self
            .registry
            .model3d_provider_for(&handle.provider)
            .await
            .ok_or_else(|| ProxyError::ProviderNotConfigured(handle.provider.clone()))?;

        let poll = provider.poll_status(&handle).await.map_err(|e| ProxyError::ProviderError {
            provider: handle.provider.clone(),
            error: e.to_string(),
            retryable: e.is_retryable(),
        })?;

        let is_terminal = matches!(
            poll.status,
            GenerationStatus::Completed | GenerationStatus::Failed | GenerationStatus::Cancelled
        );

        // Re-host in the same call that saw completion — provider URLs expire.
        let assets = if poll.status == GenerationStatus::Completed && !poll.files.is_empty() {
            rehost_model3d_files(
                app_store.as_ref().unwrap_or(&self.model3d_store),
                app_prefix,
                id,
                &poll.files,
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
            created: chrono::Utc::now().timestamp(),
        };
        enforce_requested_formats(&mut resp, &requested_formats);
        Ok(resp)
    }

    /// Drop an in-flight 3D job WITHOUT polling it.
    ///
    /// Called when the generation is cancelled. The persisted row is terminal at
    /// that point, but the router still held the job, so the next
    /// `GET /v1/models3d/{id}` polled the provider anyway, saw `Completed`,
    /// re-hosted the assets and wrote `completed` over the cancelled row —
    /// i.e. cancellation did not stop the job, it just delayed it by one poll.
    /// Returns whether a job was actually tracked (false for an already-terminal
    /// or poller-owned generation, which is not an error).
    pub async fn forget_model3d_job(&self, id: &str) -> bool {
        self.model3d_jobs.write().await.remove(id).is_some()
    }

    /// Video's counterpart to [`forget_model3d_job`](Self::forget_model3d_job).
    pub async fn forget_video_job(&self, id: &str) -> bool {
        self.video_jobs.write().await.remove(id).is_some()
    }

    /// Provider job id for a submitted 3D generation (for the DB row).
    pub async fn get_model3d_provider_job_id(&self, local_id: &str) -> Option<String> {
        self.model3d_jobs.read().await.get(local_id).map(|j| j.handle.provider_job_id.clone())
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

        let markup = self.config.cost_markup_percent;
        if markup > 0.0 {
            let (m, total) = apply_markup(est.base_cost_usd, markup);
            est.markup_usd = m;
            est.total_cost_usd = total;
            est.tokens_required = crate::providers::usd_to_tokens(total, USD_PER_TOKEN);
        }
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
                    Ok(Err(e)) => {
                        warn!(
                            provider = %deployment.provider,
                            error = %e,
                            "Video provider error"
                        );
                        self.circuit_breaker.record_failure(&deployment.provider).await;
                        if !e.is_retryable() {
                            last_error = Some(e);
                            break;
                        }
                        last_error = Some(e);
                    }
                    Err(_) => {
                        self.circuit_breaker.record_failure(&deployment.provider).await;
                        last_error = Some(ProviderError::Timeout {
                            timeout_ms: deployment.timeout_seconds * 1000,
                        });
                    }
                }
            }
        }

        Err(ProxyError::AllDeploymentsFailed {
            model: schema.id.clone(),
            last_error: last_error.map(|e| e.to_string()),
        })
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

        // Apply global markup
        let markup = self.config.cost_markup_percent;
        if markup > 0.0 {
            let (m, total) = apply_markup(est.base_cost_usd, markup);
            est.markup_usd = m;
            est.total_cost_usd = total;
            est.tokens_required = crate::providers::usd_to_tokens(total, USD_PER_TOKEN);
        }
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

        let markup = self.config.cost_markup_percent;
        if markup > 0.0 {
            let (m, total) = apply_markup(est.base_cost_usd, markup);
            est.markup_usd = m;
            est.total_cost_usd = total;
            est.tokens_required = crate::providers::usd_to_tokens(total, USD_PER_TOKEN);
        }
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
        for route_cfg in &self.config.model_routes {
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

    let generation_id = uuid::Uuid::new_v4().to_string();

    // Try to upload to configured storage backend (S3, etc.)
    let stored_url = image_store
        .store(&output.data, &output.content_type, &generation_id)
        .await
        .ok();

    let (url, b64_json) = if let Some(url) = stored_url {
        // Image was uploaded to object storage — return URL
        if extras.response_format == "b64_json" {
            let b64 = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &output.data,
            );
            (Some(url), Some(b64))
        } else {
            (Some(url), None)
        }
    } else {
        // No storage configured — return base64 inline
        let b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &output.data,
        );
        (None, Some(b64))
    };

    vec![ImageResult {
        url,
        b64_json,
        revised_prompt,
        content_type: output.content_type.clone(),
        index: 0,
    }]
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
        });
    }
    Ok(assets)
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

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Not found: {0}")]
    NotFound(String),
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
        }
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
        }
    }

    #[tokio::test]
    async fn rehost_writes_each_file_under_a_predictable_key_and_returns_absolute_urls() {
        let store: Arc<dyn crate::proxy::storage::ImageStorage> =
            Arc::new(LocalModel3dStorage::new("https://cdn.example.com".into()));

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
            Arc::new(LocalModel3dStorage::new("https://cdn.example.com".into()));
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
            Arc::new(LocalModel3dStorage::new("https://cdn.example.com".into()));
        let assets = rehost_model3d_files(&store, Some("tenant-9/meshes"), "g1", &[mesh_file()]).await.unwrap();
        assert!(assets[0].url.contains("/tenant-9/meshes/g1/model.glb"), "got {}", assets[0].url);
    }
}
