use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

use crate::capabilities::ModelSchema;
use crate::config::AppConfig;
use crate::proxy::circuit_breaker::CircuitBreaker;
use crate::providers::{apply_markup, GenerationOutput, ImageExtras, ProviderError, VideoExtras, VideoGenerationHandle};
use crate::proxy::cache::GenerationCache;
use crate::proxy::materializer::MaterializedRequest;
use crate::proxy::registry::ProviderRegistry;
use crate::proxy::storage::ImageStore;
use crate::types::*;

/// Maximum number of latency samples retained per provider.
const LATENCY_HISTORY_CAP: usize = 16;

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
    video_jobs: Arc<tokio::sync::RwLock<HashMap<String, VideoGenerationHandle>>>,
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
        Self {
            registry,
            cache,
            config,
            image_store,
            latency_history: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            video_jobs: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
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
        skip(self, schema, base, extras, materialized),
        fields(model = %schema.id, provider = %schema.provider)
    )]
    pub async fn generate_image(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Result<ImageGenerationResponse, ProxyError> {
        let start = Instant::now();

        // 1. Check cache
        if let Some(cached) = self.cache.get_image(&schema.id, base, extras).await {
            info!(model = %schema.id, "Cache hit for image generation");
            return Ok(cached);
        }

        // 2. Find a route or use single-provider direct dispatch
        let (provider_name, output) = if let Some(route) = self.find_model_route(&schema.id) {
            self.execute_route_image(schema, base, extras, materialized, &route).await?
        } else {
            // Direct dispatch: use schema.provider
            let provider = self.registry
                .image_provider_for(&schema.provider)
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

        // 3. Build cost from schema pricing
        let n = base.n.max(1) as f64;
        let base_cost = schema.pricing.base_cost_usd * n;
        let (markup, total) = apply_markup(base_cost, self.config.cost_markup_percent);
        let _ = markup;
        let usage = Some(UsageInfo {
            cost_usd: total,
            tokens: crate::providers::usd_to_tokens(total, 0.001),
            cost_source: CostSource::Estimated,
        });

        let response = ImageGenerationResponse {
            created: chrono::Utc::now().timestamp(),
            data: build_image_results(&output, extras, &self.image_store).await,
            model: schema.id.clone(),
            provider: provider_name.clone(),
            usage,
            id: format!("litegen-img-{}", uuid::Uuid::new_v4()),
        };

        // 4. Store in cache
        self.cache
            .put_image(&schema.id, base, extras, &response)
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
    ) -> Result<(String, GenerationOutput), ProxyError> {
        match route.strategy {
            RoutingStrategy::Fallback => {
                self.fallback_image(schema, base, extras, materialized, &route.deployments).await
            }
            RoutingStrategy::WeightedRoundRobin => {
                self.weighted_image(schema, base, extras, materialized, &route.deployments).await
            }
            RoutingStrategy::LowestCost => {
                self.lowest_cost_image(schema, base, extras, materialized, &route.deployments).await
            }
            RoutingStrategy::LowestLatency => {
                self.lowest_latency_image(schema, base, extras, materialized, &route.deployments).await
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

            let provider = match self.registry.image_provider_for(&deployment.provider).await {
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
        self.fallback_image(schema, base, extras, materialized, &ordered_owned).await
    }

    /// Sort deployments by estimated cost (ascending) then fall back in that order.
    async fn lowest_cost_image(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
        deployments: &[Deployment],
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
            let cost = if let Some(p) = self.registry.image_provider_for(&d.provider).await {
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
        self.fallback_image(schema, base, extras, materialized, &sorted).await
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
    ) -> Result<(String, GenerationOutput), ProxyError> {
        let mut lat_sorted: Vec<(u64, &Deployment)> = Vec::new();
        for d in deployments {
            let avg = self.avg_latency(&d.provider).await;
            lat_sorted.push((avg, d));
        }
        lat_sorted.sort_by_key(|(avg, _)| *avg);
        let sorted: Vec<Deployment> = lat_sorted.into_iter().map(|(_, d)| d.clone()).collect();
        self.fallback_image(schema, base, extras, materialized, &sorted).await
    }

    // ─── Video Generation ───────────────────────────────────────────────

    /// Start a video generation through the proxy.
    /// If a model route is configured, dispatches through route strategy.
    /// Otherwise falls back to single-provider direct dispatch using schema.provider.
    #[tracing::instrument(
        skip(self, schema, base, extras, materialized),
        fields(model = %schema.id, provider = %schema.provider)
    )]
    pub async fn generate_video(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
    ) -> Result<VideoGenerationResponse, ProxyError> {
        // Find route or use direct dispatch
        let (provider_name, handle) = if let Some(route) = self.find_model_route(&schema.id) {
            self.execute_route_video(schema, base, extras, materialized, &route).await?
        } else {
            // Direct dispatch
            let provider = self.registry
                .video_provider_for(&schema.provider)
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

        // Build cost from schema pricing
        let n = base.n.max(1) as f64;
        let base_cost = schema.pricing.base_cost_usd * n;
        let (_, total) = apply_markup(base_cost, self.config.cost_markup_percent);
        let usage_info = Some(UsageInfo {
            cost_usd: total,
            tokens: crate::providers::usd_to_tokens(total, 0.001),
            cost_source: CostSource::Estimated,
        });

        let local_id = format!("litegen-vid-{}", uuid::Uuid::new_v4());
        self.video_jobs.write().await.insert(local_id.clone(), handle);

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
        let handle = {
            let jobs = self.video_jobs.read().await;
            jobs.get(id).cloned()
        };
        let handle = handle.ok_or_else(|| ProxyError::NotFound(format!("video job '{}' not found", id)))?;

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
            usage: None,
            created: chrono::Utc::now().timestamp(),
        })
    }

    /// Dispatch a video request through a configured model route.
    async fn execute_route_video(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
        route: &ModelRoute,
    ) -> Result<(String, VideoGenerationHandle), ProxyError> {
        match route.strategy {
            RoutingStrategy::Fallback => {
                self.fallback_video(schema, base, extras, materialized, &route.deployments).await
            }
            RoutingStrategy::WeightedRoundRobin => {
                self.weighted_video(schema, base, extras, materialized, &route.deployments).await
            }
            RoutingStrategy::LowestCost => {
                self.lowest_cost_video(schema, base, extras, materialized, &route.deployments).await
            }
            RoutingStrategy::LowestLatency => {
                self.lowest_latency_video(schema, base, extras, materialized, &route.deployments).await
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

            let provider = match self.registry.video_provider_for(&deployment.provider).await {
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
        self.fallback_video(schema, base, extras, materialized, &ordered_owned).await
    }

    /// Sort video deployments by estimated cost (ascending) then fall back in that order.
    async fn lowest_cost_video(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
        deployments: &[Deployment],
    ) -> Result<(String, VideoGenerationHandle), ProxyError> {
        let dummy_req = VideoGenerationRequest {
            base: base.clone(),
            duration_seconds: extras.duration_seconds,
            aspect_ratio: extras.aspect_ratio.clone(),
            resolution: extras.resolution.clone(),
            fps: extras.fps,
        };
        let mut cost_sorted: Vec<(f64, &Deployment)> = Vec::new();
        for d in deployments {
            let cost = if let Some(p) = self.registry.video_provider_for(&d.provider).await {
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
        self.fallback_video(schema, base, extras, materialized, &sorted).await
    }

    /// Sort video deployments by average measured latency (ascending), fall back in that order.
    async fn lowest_latency_video(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
        deployments: &[Deployment],
    ) -> Result<(String, VideoGenerationHandle), ProxyError> {
        let mut lat_sorted: Vec<(u64, &Deployment)> = Vec::new();
        for d in deployments {
            let avg = self.avg_latency(&d.provider).await;
            lat_sorted.push((avg, d));
        }
        lat_sorted.sort_by_key(|(avg, _)| *avg);
        let sorted: Vec<Deployment> = lat_sorted.into_iter().map(|(_, d)| d.clone()).collect();
        self.fallback_video(schema, base, extras, materialized, &sorted).await
    }

    // ─── Cost Estimation ────────────────────────────────────────────────

    /// Estimate cost for an image generation request.
    /// Uses the provider declared in `schema.provider` from the capability registry.
    pub async fn estimate_image_cost(
        &self,
        schema: &ModelSchema,
        request: &ImageGenerationRequest,
    ) -> Result<CostEstimate, ProxyError> {
        let provider = self.registry
            .image_provider_for(&schema.provider)
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
            est.tokens_required = crate::providers::usd_to_tokens(total, 0.001);
        }
        Ok(est)
    }

    /// Estimate cost for a video generation request.
    /// Uses the provider declared in `schema.provider` from the capability registry.
    pub async fn estimate_video_cost(
        &self,
        schema: &ModelSchema,
        request: &VideoGenerationRequest,
    ) -> Result<CostEstimate, ProxyError> {
        let provider = self.registry
            .video_provider_for(&schema.provider)
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
            est.tokens_required = crate::providers::usd_to_tokens(total, 0.001);
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
            .map(|h| h.provider_job_id.clone())
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
            Self::ProviderNotConfigured(_) => 503,
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
