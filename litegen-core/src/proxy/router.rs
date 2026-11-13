use std::collections::{HashMap, VecDeque};
use crate::proxy::cache::{CacheOutcome, CachePolicy};
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
// USD per internal token, the one rate: every `UsageInfo` the router mints,
// submit and poll alike, converts at the rate a row answer uses, or the same
// generation would quote two different token counts.
use crate::types::generation::USD_PER_TOKEN;
// Now in types (engine boundary, D4); kept at this path for its callers.
pub use crate::types::generation::enforce_requested_formats;
// Now in util (engine boundary, D6); kept at this path for its callers.
pub use crate::util::content::{image_content_type, sha256_hex};

/// Maximum number of latency samples retained per provider.
const LATENCY_HISTORY_CAP: usize = 16;

/// How long the router keeps a submitted video or 3D job in memory. The poller
/// reaps a generation still active after 2 hours, so a job older than this has
/// a terminal row and nothing left to poll it through.
const JOB_MAX_AGE_SECS: i64 = 3 * 60 * 60;

/// An in-flight video job, with the provider instance that took it and the
/// `usage` triple quoted when it was submitted.
///
/// - **`provider`:** an org's own key when it brought one, which a client's
///   poll must use too (the platform's key knows nothing of the job). As
///   `Model3dJob.provider`.
/// - **`usage`:** the consumer contract reads it off the POLL response, not
///   the submit response (a client that submits and polls from different
///   processes only ever sees the poll). Carrying the submit-time triple lets
///   every poll report the identical `cost_usd` / `tokens` / `cost_source`
///   the submit quoted and the generations row persisted, with no DB round
///   trip and no dependence on the row insert (which the submit handler
///   spawns) having landed yet.
#[derive(Clone)]
struct VideoJob {
    handle: VideoGenerationHandle,
    usage: Option<UsageInfo>,
    provider: Arc<dyn crate::providers::VideoProvider>,
    /// When it was submitted (tokio's clock, so tests can age it): a job past
    /// [`JOB_MAX_AGE_SECS`] is let go at the next submit.
    submitted: tokio::time::Instant,
}

/// A submitted video: the provider's name, its handle, and the instance that took it.
type SubmittedVideo = (String, VideoGenerationHandle, Arc<dyn crate::providers::VideoProvider>);

/// The calling org's own provider credentials (BYO) for one request, each
/// under the provider it was stored for.
///
/// A request reaches a vendor directly (the model's provider) or through a
/// model route's deployments, which may name other providers. Each gets only
/// the credential the org stored for its own provider, or none, and then the
/// platform's key, as a direct request to that provider would (registry
/// BYO-ROUTE, code review 2026-11-11). `Debug` names the providers only:
/// the credentials are secrets.
#[derive(Clone, Default)]
pub struct OrgCredentials(HashMap<String, ProviderCredentials>);

impl OrgCredentials {
    /// One provider's credential, as a direct request resolves it.
    pub fn of(provider: &str, creds: Option<ProviderCredentials>) -> Self {
        let mut all = Self::default();
        if let Some(c) = creds {
            all.insert(provider, c);
        }
        all
    }

    /// Add the org's credential for `provider`.
    pub fn insert(&mut self, provider: &str, creds: ProviderCredentials) {
        self.0.insert(provider.to_string(), creds);
    }

    /// The org's credential for `provider`, if it stored one.
    pub fn get(&self, provider: &str) -> Option<ProviderCredentials> {
        self.0.get(provider).cloned()
    }

    /// Whether the org stored a credential for `provider`.
    pub fn has(&self, provider: &str) -> bool {
        self.0.contains_key(provider)
    }
}

/// Which providers can serve one caller, and so whether a generation of a
/// model would reach a vendor for them (L14; FU-ROUTE, follow-ups
/// 2026-11-13). The generation and quote routes ask it before anything is
/// reserved, and `GET /v1/models` serves its answer as `is_available`, so the
/// two agree. Each provider's answer is kept per medium, so a listing asks
/// each once.
pub struct Reach<'a, F> {
    router: &'a ProxyRouter,
    /// Whether the caller's org stored its own credential for a provider.
    org_has: F,
    known: HashMap<(String, &'static str), bool>,
}

impl<'a, F: Fn(&str) -> bool> Reach<'a, F> {
    pub fn new(router: &'a ProxyRouter, org_has: F) -> Self {
        Self { router, org_has, known: HashMap::new() }
    }

    /// Ok when a generation of `schema` would reach a vendor for this caller,
    /// else the 400 `provider_not_configured` it would answer.
    ///
    /// - **A model route in effect** ([`ProxyRouter::dispatch_route`]: image
    ///   generations and videos, never segmentation or 3D): the route alone
    ///   decides, as dispatch sends the model to its deployments only. Served
    ///   when one of them can be; the refusal names the route's providers, as
    ///   the router's own answer does when it finds none it can use.
    /// - **Otherwise** the model's own provider, on the org's key or the
    ///   gateway's instance: the test generations have always made.
    pub async fn serves(&mut self, schema: &ModelSchema) -> Result<(), ProxyError> {
        let Some(deployments) = self.router.dispatch_route(schema) else {
            let own = (self.org_has)(&schema.provider) || self.router.has_provider(schema.media_type, &schema.provider).await;
            return if own { Ok(()) } else { Err(ProxyError::not_configured(&schema.provider)) };
        };
        for provider in &deployments {
            if self.can_serve(schema.media_type, provider).await {
                return Ok(());
            }
        }
        Err(not_configured_route(&schema.id, deployments.into_iter().map(|p| (p, Skip::NotConfigured(None))).collect()))
    }

    /// Whether a route's deployment on `provider` can serve a `medium` request
    /// for this caller, as dispatch decides (`*_provider_for_request`): on the
    /// org's key when it stored one, if the provider has an adapter for the
    /// medium; otherwise on the gateway's instance, if it holds one.
    async fn can_serve(&mut self, medium: crate::capabilities::MediaType, provider: &str) -> bool {
        let key = (provider.to_string(), medium.as_str());
        if let Some(&known) = self.known.get(&key) {
            return known;
        }
        let known = if (self.org_has)(provider) {
            adapts(medium, provider)
        } else {
            self.router.has_provider(medium, provider).await
        };
        self.known.insert(key, known);
        known
    }
}

/// Whether `provider` has an adapter for `medium`, which is what an org's own
/// key builds for it (`proxy::registry::build_*_provider`, mirrored by the
/// registry's lists).
fn adapts(medium: crate::capabilities::MediaType, provider: &str) -> bool {
    use crate::capabilities::MediaType as M;
    use crate::proxy::registry::{IMAGE_PROVIDERS, MODEL3D_PROVIDERS, VIDEO_PROVIDERS};
    match medium {
        M::Image => IMAGE_PROVIDERS,
        M::Video => VIDEO_PROVIDERS,
        M::Model3d => MODEL3D_PROVIDERS,
    }
    .contains(&provider)
}

impl std::fmt::Debug for OrgCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut providers: Vec<&str> = self.0.keys().map(String::as_str).collect();
        providers.sort_unstable();
        f.debug_tuple("OrgCredentials").field(&providers).finish()
    }
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
    video_jobs: Arc<tokio::sync::RwLock<HashMap<String, VideoJob>>>,
    /// In-flight 3D generation jobs, keyed by the local `litegen-3d-...` ID.
    model3d_jobs: Arc<tokio::sync::RwLock<HashMap<String, Model3dJob>>>,
    /// Storage for re-hosted 3D assets (S3 when configured, local otherwise).
    pub model3d_store: Arc<dyn crate::proxy::storage::ImageStorage>,
    /// Signs links to litegen's own stored files (`docs/storage-access.md`),
    /// keyed from the secrets key when it is set, otherwise from the key stored
    /// once in `server_secrets`. `new` builds a placeholder (keyed from the
    /// config's secrets key, or randomly without one) that `main` replaces
    /// through `with_link_signer` with the signer
    /// `access::signing::link_signer_for` picks.
    pub link_signer: Arc<crate::access::signing::LinkSigner>,
    /// Circuit breaker tracking consecutive failures per provider.
    pub circuit_breaker: Arc<CircuitBreaker>,
    /// Settings changed at runtime through the management API. Each set field
    /// overrides the matching `config` value for every request that follows.
    overrides: std::sync::RwLock<RuntimeOverrides>,
    /// The cache, retry, timeout and breaker settings in force, recomputed
    /// whenever the overrides change (`docs/gateway-settings.md`).
    settings: std::sync::RwLock<crate::management::settings::EffectiveSettings>,
    /// Bumped by every change of the overrides, so a settings refresh can
    /// tell a save landed while it was reading the database.
    overrides_version: std::sync::atomic::AtomicU64,
    /// The Redis several replicas share, when one is set (`docs/scaling.md`).
    shared: Option<Arc<crate::shared::SharedRedis>>,
    /// Which stored runtime models and provider configs this replica has
    /// applied, so a refresh can tell what changed in the database.
    applied_runtime: std::sync::Mutex<crate::management::runtime::AppliedRuntime>,
    /// Test only: a refresh's sync of a part stops right after its database
    /// read (see [`Self::pause_next_sync`]).
    #[cfg(test)]
    sync_pauses: std::sync::Mutex<HashMap<&'static str, Arc<crate::proxy::cache::LookupPause>>>,
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
        let link_signer = Arc::new(crate::access::signing::LinkSigner::from_secrets_key(config.secrets_key.as_deref()));
        let settings = crate::management::settings::effective(&config, &Default::default());
        cache.reconfigure(settings.cache);
        Self {
            registry,
            cache,
            config,
            image_store,
            latency_history: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            video_jobs: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            model3d_jobs: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            model3d_store,
            link_signer,
            circuit_breaker: cb,
            overrides: std::sync::RwLock::new(RuntimeOverrides::default()),
            settings: std::sync::RwLock::new(settings),
            overrides_version: std::sync::atomic::AtomicU64::new(0),
            shared: None,
            applied_runtime: std::sync::Mutex::new(Default::default()),
            #[cfg(test)]
            sync_pauses: Default::default(),
        }
    }

    /// Share state with the other replicas through `shared`.
    pub fn with_shared(mut self, shared: Arc<crate::shared::SharedRedis>) -> Self {
        // The breaker's streaks move to Redis. Called right after `new`, so the
        // settings are the config's; gateway settings reconfigure it later.
        self.circuit_breaker = Arc::new(CircuitBreaker::with_store(
            self.config.circuit_breaker.threshold,
            std::time::Duration::from_secs(self.config.circuit_breaker.open_for_seconds),
            Arc::new(crate::shared::provider_breaker::RedisBreakerStore::new(shared.clone())),
        ));
        // The cache gets a shared tier behind its own memory.
        self.cache.attach_shared(shared.clone());
        self.shared = Some(shared);
        self
    }

    /// Sign and verify links with `signer`, not the one `new` made from the
    /// config's secrets key. Called right after `new`, before the router is
    /// shared: `main` passes the key `access::signing::link_signer_for` picks,
    /// which may be the one stored in the database.
    pub fn with_link_signer(mut self, signer: Arc<crate::access::signing::LinkSigner>) -> Self {
        self.link_signer = signer;
        self
    }

    /// The Redis shared with the other replicas; `None` is a single replica.
    pub fn shared(&self) -> Option<&Arc<crate::shared::SharedRedis>> {
        self.shared.as_ref()
    }

    /// Test only: the next refresh's sync of `part` (`"runtime_models"` or
    /// `"model_states"`) stops right after its database read, and says so on
    /// `reached`. Per router, so tests running side by side don't trip each
    /// other.
    #[cfg(test)]
    pub(crate) fn pause_next_sync(&self, part: &'static str) -> Arc<crate::proxy::cache::LookupPause> {
        let pause = Arc::new(crate::proxy::cache::LookupPause { reached: tokio::sync::Notify::new(), resume: tokio::sync::Notify::new() });
        self.sync_pauses.lock().unwrap().insert(part, pause.clone());
        pause
    }

    #[cfg(test)]
    pub(crate) async fn pause_after_sync_read(&self, part: &'static str) {
        let pause = self.sync_pauses.lock().unwrap().remove(part);
        if let Some(pause) = pause {
            pause.reached.notify_one();
            pause.resume.notified().await;
        }
    }

    /// What this replica has applied of the stored runtime models and provider
    /// configs (`management::runtime::refresh`). Never hold it across an await.
    pub(crate) fn applied_runtime(&self) -> std::sync::MutexGuard<'_, crate::management::runtime::AppliedRuntime> {
        self.applied_runtime.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The cache, retry, timeout and breaker settings in force.
    pub fn settings(&self) -> crate::management::settings::EffectiveSettings {
        *self.settings.read().unwrap_or_else(|p| p.into_inner())
    }

    /// After attempt `attempt` (counted from 0, at most `max_retries` more
    /// follow) failed with `err`: whether to try again, having waited as the
    /// retry settings say (`retry_delay`). `may_resend` decides whether at all.
    async fn wait_to_retry(&self, attempt: u32, max_retries: u32, err: &ProviderError, vendor_accepted: bool) -> bool {
        if attempt >= max_retries || !may_resend(err, vendor_accepted) {
            return false;
        }
        match retry_delay(attempt + 1, err, &self.settings().retries, rand::random()) {
            Some(wait) => {
                if !wait.is_zero() {
                    info!(wait_ms = wait.as_millis() as u64, "waiting before a retry");
                    tokio::time::sleep(wait).await;
                }
                true
            }
            None => false,
        }
    }

    /// A snapshot of the runtime overrides.
    pub fn overrides(&self) -> RuntimeOverrides {
        self.overrides.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Replace the runtime overrides; applies to every request that follows.
    /// A change to one of them goes through [`Self::update_overrides`].
    pub fn set_overrides(&self, overrides: RuntimeOverrides) {
        self.apply_overrides(None, |current| *current = overrides);
    }

    /// Change the runtime overrides in place, for every request that follows:
    /// `change` runs under their write lock, so two changes at once to
    /// different fields both land (FU-LOCK). Reading them and then setting
    /// them whole let the one that wrote last put back the other's old value.
    pub fn update_overrides(&self, change: impl FnOnce(&mut RuntimeOverrides)) {
        self.apply_overrides(None, change);
    }

    /// How many times the overrides have changed.
    pub fn overrides_version(&self) -> u64 {
        self.overrides_version.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// [`Self::set_overrides`], unless they changed since `version` was read:
    /// a refresh whose database read raced a save never undoes the save.
    /// Whether it applied.
    pub fn set_overrides_unless_changed(&self, version: u64, overrides: RuntimeOverrides) -> bool {
        self.apply_overrides(Some(version), |current| *current = overrides)
    }

    /// Apply `change` to the overrides, unless they changed since version
    /// `expected`, and put the settings it makes in force. All of it happens
    /// under the overrides' write lock, the cache and breaker reconfiguring
    /// included, so two changes at once neither lose a field nor leave the
    /// cache or the breaker set by the older one.
    fn apply_overrides(&self, expected: Option<u64>, change: impl FnOnce(&mut RuntimeOverrides)) -> bool {
        use std::sync::atomic::Ordering;
        let mut guard = self.overrides.write().unwrap_or_else(|p| p.into_inner());
        if expected.is_some_and(|v| v != self.overrides_version.load(Ordering::SeqCst)) {
            return false;
        }
        change(&mut guard);
        let settings = crate::management::settings::effective(&self.config, &guard.gateway);
        self.overrides_version.fetch_add(1, Ordering::SeqCst);
        let previous = std::mem::replace(&mut *self.settings.write().unwrap_or_else(|p| p.into_inner()), settings);
        if settings.cache != previous.cache {
            self.cache.reconfigure(settings.cache);
        }
        if settings.breaker != previous.breaker {
            self.circuit_breaker.reconfigure(settings.breaker.threshold, settings.breaker.open_for);
        }
        true
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
    #[allow(clippy::too_many_arguments)]
    pub async fn generate_image(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
        app_creds: OrgCredentials,
        app_store: Option<Arc<dyn ImageStore>>,
        // Tenant scope for the cache key (app id, falling back to org id). The
        // cache is process-global, so this prevents cross-tenant cache hits.
        tenant: Option<&str>,
    ) -> Result<ImageGenerationResponse, ProxyError> {
        self.generate_image_cached(schema, base, extras, materialized, app_creds, app_store, tenant, CachePolicy::default())
            .await
            .map(|(response, _)| response)
    }

    /// [`Self::generate_image`], as far as `policy` (the request's
    /// `Cache-Control`) lets the cache act, and what the cache did.
    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(
        // `app_creds` carries decrypted BYO provider secrets — see the note on
        // `generate_model3d`; an instrumented argument becomes a span attribute.
        skip(self, schema, base, extras, materialized, app_creds, app_store, policy),
        fields(model = %schema.id, provider = %schema.provider)
    )]
    pub async fn generate_image_cached(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
        app_creds: OrgCredentials,
        app_store: Option<Arc<dyn ImageStore>>,
        tenant: Option<&str>,
        policy: CachePolicy,
    ) -> Result<(ImageGenerationResponse, CacheOutcome), ProxyError> {
        let start = Instant::now();
        let route = self.find_model_route(&schema.id);

        // 1. Check cache: on for this model as its route says, else as the
        //    gateway's settings say (`docs/gateway-settings.md`).
        let (cache_on, _) = self.cache_for(route.as_ref());
        let mut outcome = CacheOutcome::Bypass;
        if cache_on && policy.lookup {
            if let Some(cached) = self.cache.get_image(tenant, &schema.id, base, extras, materialized).await {
                info!(model = %schema.id, "Cache hit for image generation");
                return Ok((cached, CacheOutcome::Hit));
            }
            outcome = CacheOutcome::Miss;
        }

        // 2. Find a route or use single-provider direct dispatch
        let (provider_name, output) = if let Some(route) = route {
            self.execute_route_image(schema, base, extras, materialized, &route, app_creds).await?
        } else {
            // Direct dispatch: use schema.provider
            let provider = self.registry
                .image_provider_for_request(&schema.provider, app_creds.get(&schema.provider))
                .await
                .ok_or_else(|| ProxyError::not_configured(&schema.provider))?;

            let max_retries = self.settings().retries.max_retries;
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
                let limit = self.settings().vendor_timeout;
                // A caller that asked to wait less (`Prefer: wait`,
                // `respond-async`) hears back sooner; the job goes on.
                let deadline = attempt_start + crate::providers::call_wait().map_or(limit, |w| w.min(limit));
                match tokio::time::timeout(
                    limit,
                    crate::providers::with_deadline(deadline, provider.generate(schema, base, extras, materialized)),
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
                        let again = self.wait_to_retry(attempt, max_retries, &e, extras.vendor_job.get().is_some()).await;
                        last_error = Some(e);
                        if !again { break; }
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
        // A multiview model's views, and a directions request's facings
        // (PixelGenie L16), come from one billed vendor call.
        let one_call = schema.task == crate::capabilities::ModelTask::Multiview || extras.directions.is_some();
        let produced = if one_call { 1.0 } else { data.len().max(1) as f64 };
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
            metadata: None,
        };

        // 4. Store in cache, if it is still on: the cache may have been
        //    turned off (or the route changed) while the vendor worked, and
        //    an answer that comes back after a switch-off must not be kept,
        //    or a later switch-on would serve it. The time to live in force
        //    now applies to what is stored now.
        if cache_on && policy.store {
            let (still_on, ttl) = self.cache_for(self.find_model_route(&schema.id).as_ref());
            if still_on {
                self.cache.put_image(tenant, &schema.id, base, extras, materialized, &response, ttl).await;
            }
        }

        info!(
            model = %schema.id,
            provider = %provider_name,
            latency_ms = latency_ms,
            "Image generation completed"
        );

        Ok((response, outcome))
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
            .ok_or_else(|| ProxyError::not_configured(&schema.provider))?;
        let mut last_error: Option<ProviderError> = None;
        let mut vendor_masks = None;
        let max_retries = self.settings().retries.max_retries;
        for attempt in 0..=max_retries {
            if attempt > 0 {
                info!(provider = %schema.provider, attempt, "Retrying segmentation");
            }
            let attempt_start = Instant::now();
            match tokio::time::timeout(self.settings().vendor_timeout, provider.segment(schema, input)).await {
                Ok(Ok(m)) => {
                    self.record_latency(&schema.provider, attempt_start.elapsed().as_millis() as u64).await;
                    vendor_masks = Some(m);
                    break;
                }
                Ok(Err(ProviderError::Timeout { .. })) | Err(_) => {
                    return Err(ProxyError::VendorTimeout { model: schema.id.clone(), provider: schema.provider.clone(), vendor_job_id: None });
                }
                Ok(Err(e)) => {
                    let again = self.wait_to_retry(attempt, max_retries, &e, false).await;
                    last_error = Some(e);
                    if !again {
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
                    retryable: false,
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
            metadata: None,
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
        app_creds: OrgCredentials,
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
        app_creds: OrgCredentials,
    ) -> Result<(String, GenerationOutput), ProxyError> {
        let mut attempts = Attempts::default();

        for deployment in deployments {
            // ── Circuit breaker: skip provider if breaker is open ──
            if let Some(left) = self.circuit_breaker.open_remaining(&deployment.provider).await {
                warn!(
                    provider = %deployment.provider,
                    "Circuit breaker open — skipping deployment"
                );
                attempts.skipped_open(&deployment.provider, left);
                continue;
            }

            // The org's credential for this deployment's own provider, never
            // another vendor's; none, and the platform's key, as a direct
            // request to that provider would (BYO-ROUTE). A 401, 402, 403 or
            // 429 says the deployment is unwell only to the platform's own
            // credential, never to a tenant's (E9).
            let creds = app_creds.get(&deployment.provider);
            let platform_credential = creds.is_none();
            let provider = match self.registry.image_provider_for_request(&deployment.provider, creds).await {
                Some(p) => p,
                None => {
                    warn!(provider = %deployment.provider, "Image provider not configured, skipping deployment");
                    attempts.skipped_unconfigured(&deployment.provider);
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
                    crate::providers::with_deadline(
                        attempt_start + std::time::Duration::from_secs(deployment.timeout_seconds),
                        provider.generate(schema, base, extras, materialized),
                    ),
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
                        if e.says_the_deployment_is_unhealthy(platform_credential) {
                            self.circuit_breaker.record_failure(&deployment.provider).await;
                        }
                        if let Some(job) = extras.vendor_job.get() {
                            // The vendor took the job: no other deployment may
                            // be sent it too.
                            return Err(failure(&schema.id, &deployment.provider, Some(e), Some(job)));
                        }
                        let again = self.wait_to_retry(attempt, deployment.max_retries, &e, false).await;
                        attempts.failed(&deployment.provider, e);
                        if !again {
                            break; // Skip to next deployment
                        }
                    }
                }
            }
        }

        Err(attempts.into_error(&schema.id))
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
        app_creds: OrgCredentials,
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
        app_creds: OrgCredentials,
    ) -> Result<(String, GenerationOutput), ProxyError> {
        let mut cost_sorted: Vec<(f64, &Deployment)> = Vec::new();
        // Build a minimal ImageGenerationRequest for cost estimation
        let dummy_req = ImageGenerationRequest {
            base: base.clone(),
            size: extras.size.clone(),
            resolution: None, aspect_ratio: extras.aspect_ratio.clone(),
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
            palette: None, directions: None,
        };
        for d in deployments {
            let cost = if let Some(p) = self.registry.image_provider_for_request(&d.provider, app_creds.get(&d.provider)).await {
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
        app_creds: OrgCredentials,
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
        app_creds: OrgCredentials,
    ) -> Result<VideoGenerationResponse, ProxyError> {
        // Find route or use direct dispatch
        let (provider_name, handle, provider) = if let Some(route) = self.find_model_route(&schema.id) {
            self.execute_route_video(schema, base, extras, materialized, &route, app_creds).await?
        } else {
            // Direct dispatch
            let provider = self.registry
                .video_provider_for_request(&schema.provider, app_creds.get(&schema.provider))
                .await
                .ok_or_else(|| ProxyError::not_configured(&schema.provider))?;

            let max_retries = self.settings().retries.max_retries;
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
                    self.settings().vendor_timeout,
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
                        let again = self.wait_to_retry(attempt, max_retries, &e, false).await;
                        last_error = Some(e);
                        if !again { break; }
                    }
                }
            }

            let h = handle.ok_or_else(|| failure(&schema.id, &schema.provider, last_error, None))?;
            (schema.provider.clone(), h, provider)
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
        {
            let mut jobs = self.video_jobs.write().await;
            // As for 3D jobs: a client's GET on this replica, a cancel, or
            // this replica's poller ending the job lets it go; one ended
            // elsewhere (another replica, a webhook-only caller's job polled
            // by another replica's poller) would stay here, provider instance
            // and all, until a restart. Past the reap age no observer needs it.
            let max_age = std::time::Duration::from_secs(JOB_MAX_AGE_SECS as u64);
            jobs.retain(|_, j| j.submitted.elapsed() < max_age);
            jobs.insert(
                local_id.clone(),
                VideoJob { handle, usage: usage_info.clone(), provider, submitted: tokio::time::Instant::now() },
            );
        }

        Ok(VideoGenerationResponse {
            video_url_expires_at: None,
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
            metadata: None,
        })
    }

    /// Look up an in-flight video generation by local ID and poll its provider.
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
        keep: bool,
        db: &Arc<dyn crate::db::DatabaseStore>,
    ) -> Result<(VideoGenerationResponse, Option<serde_json::Value>), ProxyError> {
        // Nothing here writes the outcome, so the claim goes at once.
        self.get_video_status_claimed(id, app_store, app_prefix, keep, db).await.map(|(resp, assets, _claim)| (resp, assets))
    }

    /// [`get_video_status`](Self::get_video_status), also handing back the
    /// claim when this call kept the finished video. The caller holds it until
    /// it has written the outcome to the row, so no other observer keeps the
    /// video again in between. Without the claim, a finished video is
    /// reported still processing and nothing is downloaded or kept. The poll
    /// runs in a span named `get_video_status` with the job's `id`, as the
    /// handler's polls did before the claim was handed back.
    #[allow(clippy::type_complexity)]
    #[tracing::instrument(name = "get_video_status", skip(self, app_store, db), fields(id = %id))]
    pub async fn get_video_status_claimed(
        &self,
        id: &str,
        app_store: Option<Arc<dyn crate::proxy::storage::ImageStorage>>,
        app_prefix: Option<&str>,
        keep: bool,
        db: &Arc<dyn crate::db::DatabaseStore>,
    ) -> Result<(VideoGenerationResponse, Option<serde_json::Value>, Option<crate::proxy::delivery_lease::DeliveryClaim>), ProxyError> {
        let job = {
            let jobs = self.video_jobs.read().await;
            jobs.get(id).cloned()
        };
        let VideoJob { handle, usage, provider, .. } =
            job.ok_or_else(|| ProxyError::NotFound(format!("video job '{}' not found", id)))?;

        let (mut poll, claim) = poll_video_claimed(&provider, &handle, keep, db, id, crate::proxy::delivery_lease::Observer::Client)
            .await
            .map_err(|e| ProxyError::ProviderError {
                provider: handle.provider.clone(),
                error: e.to_string(),
                retryable: e.is_retryable(),
            })?;

        let (video_url, assets) = if poll.status == GenerationStatus::Completed {
            // The bytes move on: no copy of a video that may be 1 GiB.
            let data = poll.video_data.take();
            let keeping = rehost_video(
                app_store.as_ref().unwrap_or(&self.model3d_store),
                app_prefix,
                id,
                data,
                poll.video_url.as_deref(),
                poll.content_type.as_deref(),
                keep,
                !self.config.storage_access.unchecked_vendor_hosts,
            );
            let stored = match claim.as_ref() {
                Some(claim) => claim.renewing_while(keeping).await,
                None => keeping.await,
            };
            match stored {
                Ok(stored) => (Some(stored.url), Some(serde_json::json!({ "assets": [stored.asset] }))),
                // Nothing to keep: the generation has failed, decided now.
                Err(RehostError::Permanent(why)) => {
                    poll.status = GenerationStatus::Failed;
                    poll.error = Some(format!("the video finished but could not be kept: {why}"));
                    (None, None)
                }
                Err(RehostError::Transient(why)) => {
                    return Err(ProxyError::ProviderError {
                        provider: handle.provider.clone(),
                        error: format!("the video finished but could not be kept yet: {why}"),
                        retryable: true,
                    });
                }
            }
        } else {
            (poll.video_url.clone(), None)
        };
        let media = assets.as_ref().and_then(crate::types::VideoMedia::from_assets);

        if matches!(
            poll.status,
            GenerationStatus::Completed | GenerationStatus::Failed | GenerationStatus::Cancelled
        ) {
            self.video_jobs.write().await.remove(id);
        }

        Ok((
            VideoGenerationResponse {
                video_url_expires_at: None,
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
                metadata: None,
            },
            assets,
            claim,
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
            .ok_or_else(|| ProxyError::not_configured(&schema.provider))?;
        let limit = self.settings().vendor_timeout;
        match tokio::time::timeout(limit, provider.preflight(schema, extras)).await {
            Ok(Ok(p)) => Ok(p),
            Ok(Err(e)) => Err(ProxyError::PreflightFailed {
                provider: schema.provider.clone(),
                error: e.to_string(),
                retryable: e.is_retryable(),
            }),
            Err(_) => Err(ProxyError::PreflightFailed {
                provider: schema.provider.clone(),
                error: format!("it did not finish within {}s", limit.as_secs()),
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
        self.generate_model3d_keeping(schema, base, extras, materialized, app_creds, true).await
    }

    /// [`generate_model3d`](Self::generate_model3d), keeping the outputs or,
    /// with `keep` false, handing out the vendor's own files (`keep_outputs`).
    pub async fn generate_model3d_keeping(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &Model3dExtras,
        materialized: &MaterializedRequest,
        app_creds: Option<ProviderCredentials>,
        keep: bool,
    ) -> Result<Model3dGenerationResponse, ProxyError> {
        let provider = self
            .registry
            .model3d_provider_for_request(&schema.provider, app_creds)
            .await
            .ok_or_else(|| ProxyError::not_configured(&schema.provider))?;

        let max_retries = self.settings().retries.max_retries;
        let mut last_error: Option<ProviderError> = None;
        let mut handle: Option<Model3dGenerationHandle> = None;

        for attempt in 0..=max_retries {
            if attempt > 0 {
                info!(provider = %schema.provider, attempt, "Retrying 3D generation");
            }
            let attempt_start = Instant::now();
            let limit = self.settings().vendor_timeout;
            match tokio::time::timeout(
                limit,
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
                    // Never after the adapter's own timeout either (`may_resend`):
                    // the vendor may have queued the job.
                    let again = self.wait_to_retry(attempt, max_retries, &e, false).await;
                    last_error = Some(e);
                    if !again { break; }
                }
                Err(_) => {
                    // A queue submit is not idempotent: dropping the future does
                    // not withdraw a job the vendor may already have accepted,
                    // so re-POSTing here can queue (and bill) a second and a
                    // third job that nothing ever tracks or cancels.
                    last_error = Some(ProviderError::Timeout { timeout_ms: limit.as_millis() as u64 });
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
            // A client GET that itself observes completion, a cancel, or this
            // replica's poller ending the job removes it; one ended on another
            // replica would otherwise stay here, provider instance and all,
            // until a restart. Past the poller's 2 h reap age no observer can
            // need it.
            jobs.retain(|_, j| created - j.created < JOB_MAX_AGE_SECS);
            jobs.insert(
                local_id.clone(),
                Model3dJob {
                    handle,
                    usage: usage_info.clone(),
                    delivery: crate::proxy::model3d_deliver::DeliveryPlan {
                        keep_nothing: !keep,
                        ..crate::proxy::model3d_deliver::DeliveryPlan::for_request(
                            schema,
                            &Model3dExtras { output_formats: requested_formats.clone(), ..extras.clone() },
                        )
                    },
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
            metadata: None,
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
    pub async fn get_model3d_status(
        &self,
        id: &str,
        app_store: Option<Arc<dyn crate::proxy::storage::ImageStorage>>,
        app_prefix: Option<&str>,
        db: &Arc<dyn crate::db::DatabaseStore>,
    ) -> Result<Model3dGenerationResponse, ProxyError> {
        // Nothing here writes the outcome, so the claim goes at once.
        self.get_model3d_status_claimed(id, app_store, app_prefix, db).await.map(|(resp, _claim)| resp)
    }

    /// [`get_model3d_status`](Self::get_model3d_status), also handing back the
    /// delivery claim when this call delivered the job. The caller holds it
    /// until it has written the outcome to the row, so no other observer
    /// delivers the job again in between. The poll runs in a span named
    /// `get_model3d_status` with the job's `id` (payloads review #8).
    #[tracing::instrument(name = "get_model3d_status", skip(self, app_store, db), fields(id = %id))]
    pub async fn get_model3d_status_claimed(
        &self,
        id: &str,
        app_store: Option<Arc<dyn crate::proxy::storage::ImageStorage>>,
        app_prefix: Option<&str>,
        db: &Arc<dyn crate::db::DatabaseStore>,
    ) -> Result<(Model3dGenerationResponse, Option<crate::proxy::delivery_lease::DeliveryClaim>), ProxyError> {
        let job = {
            let jobs = self.model3d_jobs.read().await;
            jobs.get(id).cloned()
        };
        let Model3dJob { mut handle, usage, delivery, provider, created } =
            job.ok_or_else(|| ProxyError::NotFound(format!("3d job '{}' not found", id)))?;
        let requested_formats = delivery.requested_formats.clone();

        // The instance that submitted the job — see `Model3dJob`. An adapter
        // that downloads a finished job's files asks the gate first, so only
        // the observer that delivers the job downloads them.
        let gate = crate::proxy::delivery_lease::DeliveryGate::new(db, id, crate::proxy::delivery_lease::Observer::Client);
        let mut poll = provider.poll_status_gated(&handle, &gate).await.map_err(|e| ProxyError::ProviderError {
            provider: handle.provider.clone(),
            error: e.to_string(),
            retryable: e.is_retryable(),
        })?;

        // A vendor flow's next billed stage, or its new continuation state:
        // the same bookkeeping the poller applies, so whichever observer sees
        // a stage boundary first handles it — once.
        let before = handle.stage_context.clone();
        crate::proxy::model3d_stages::advance(db, &provider, id, &mut handle, &mut poll, "client").await;
        if handle.stage_context != before {
            if let Some(job) = self.model3d_jobs.write().await.get_mut(id) {
                job.handle.stage_context = handle.stage_context.clone();
            }
        }

        // A finished job is delivered by one observer: this one only if it
        // claims the delivery. Otherwise it reports the job still processing,
        // and the holder's terminal write answers the next poll. The adapter
        // asked the gate before downloading; one that never asks (it
        // downloads nothing) is claimed for here, after the poll.
        let claim = match gate.into_outcome() {
            crate::proxy::delivery_lease::GateState::Claimed(claim) => Some(claim),
            crate::proxy::delivery_lease::GateState::Refused => {
                crate::proxy::model3d_deliver::stand_aside(&mut poll);
                None
            }
            crate::proxy::delivery_lease::GateState::NotAsked if crate::proxy::model3d_deliver::needs_delivery(&poll) => {
                let claim = crate::proxy::delivery_lease::claim_delivery(db, id, crate::proxy::delivery_lease::Observer::Client).await;
                if claim.is_none() {
                    crate::proxy::model3d_deliver::stand_aside(&mut poll);
                }
                claim
            }
            crate::proxy::delivery_lease::GateState::NotAsked => None,
        };

        // Normalise, measure and fill in the promised containers BEFORE
        // rehost, so a delivered file is stored and served like a native one:
        // the step the poller runs too, so whichever observer wins the race
        // produces the same assets.
        let keep = !delivery.keep_nothing;
        crate::proxy::model3d_deliver::deliver_poll(&mut poll, delivery, id).await;
        // Keeping nothing: the vendor's own files are the answer, or there is none.
        let handed_out = if !keep && poll.status == GenerationStatus::Completed {
            match keep_nothing_model3d_assets(&poll.files) {
                Ok(a) => Some(a),
                Err(why) => {
                    poll.status = GenerationStatus::Failed;
                    poll.error = Some(why);
                    None
                }
            }
        } else {
            None
        };
        let is_terminal = matches!(
            poll.status,
            GenerationStatus::Completed | GenerationStatus::Failed | GenerationStatus::Cancelled
        );
        // The files move on to the store: no copy of what may be 128 MiB each.
        let files = std::mem::take(&mut poll.files);

        // Re-host in the same call that saw completion — provider URLs expire.
        let assets = if let Some(a) = handed_out {
            a
        } else if poll.status == GenerationStatus::Completed && !files.is_empty() {
            rehost_model3d_files(
                app_store.as_ref().unwrap_or(&self.model3d_store),
                app_prefix,
                id,
                files,
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
            metadata: None,
        };
        enforce_requested_formats(&mut resp, &requested_formats);
        Ok((resp, claim))
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

    /// [`forget_video_job`](Self::forget_video_job), handing back the job's
    /// handle and the instance that submitted it, as `forget_model3d_job`
    /// does: a deleted org's job is stopped through it (SEC-INMEM).
    pub async fn take_video_job(&self, id: &str) -> Option<(VideoGenerationHandle, Arc<dyn crate::providers::VideoProvider>)> {
        self.video_jobs.write().await.remove(id).map(|j| (j.handle, j.provider))
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
    ///   `provider_not_configured` for a model it can generate with.
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
            .ok_or_else(|| ProxyError::not_configured(&schema.provider))?;
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
        app_creds: OrgCredentials,
    ) -> Result<SubmittedVideo, ProxyError> {
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
        app_creds: OrgCredentials,
    ) -> Result<SubmittedVideo, ProxyError> {
        let mut attempts = Attempts::default();

        for deployment in deployments {
            // ── Circuit breaker: skip provider if breaker is open ──
            if let Some(left) = self.circuit_breaker.open_remaining(&deployment.provider).await {
                warn!(
                    provider = %deployment.provider,
                    "Circuit breaker open — skipping video deployment"
                );
                attempts.skipped_open(&deployment.provider, left);
                continue;
            }

            // This deployment's own provider's credential only (BYO-ROUTE),
            // and the breaker judged by it (E9): see `fallback_image`.
            let creds = app_creds.get(&deployment.provider);
            let platform_credential = creds.is_none();
            let provider = match self.registry.video_provider_for_request(&deployment.provider, creds).await {
                Some(p) => p,
                None => {
                    warn!(provider = %deployment.provider, "Video provider not configured, skipping deployment");
                    attempts.skipped_unconfigured(&deployment.provider);
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
                        return Ok((deployment.provider.clone(), handle, provider));
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
                        if e.says_the_deployment_is_unhealthy(platform_credential) {
                            self.circuit_breaker.record_failure(&deployment.provider).await;
                        }
                        let again = self.wait_to_retry(attempt, deployment.max_retries, &e, false).await;
                        attempts.failed(&deployment.provider, e);
                        if !again {
                            break;
                        }
                    }
                }
            }
        }

        Err(attempts.into_error(&schema.id))
    }

    /// Weighted round-robin for video deployments.
    async fn weighted_video(
        &self,
        schema: &ModelSchema,
        base: &BaseGenerationRequest,
        extras: &VideoExtras,
        materialized: &MaterializedRequest,
        deployments: &[Deployment],
        app_creds: OrgCredentials,
    ) -> Result<SubmittedVideo, ProxyError> {
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
        app_creds: OrgCredentials,
    ) -> Result<SubmittedVideo, ProxyError> {
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
            let cost = if let Some(p) = self.registry.video_provider_for_request(&d.provider, app_creds.get(&d.provider)).await {
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
        app_creds: OrgCredentials,
    ) -> Result<SubmittedVideo, ProxyError> {
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

    /// Estimate cost for an image generation request: the catalog row's price,
    /// through the adapter of the model's own provider when this caller can
    /// be served by it, else of the first deployment of its model route that
    /// can, in the route's order (FU-ROUTE): a quote answers wherever the
    /// generation would be served. Each adapter gets only the org's
    /// credential for its own provider (BYO-ROUTE).
    pub async fn estimate_image_cost(
        &self,
        schema: &ModelSchema,
        request: &ImageGenerationRequest,
        org: &OrgCredentials,
    ) -> Result<CostEstimate, ProxyError> {
        let mut quoting = None;
        for name in self.quoting_providers(schema) {
            if let Some(p) = self.registry.image_provider_for_request(&name, org.get(&name)).await {
                quoting = Some((name, p));
                break;
            }
        }
        let (name, provider) = quoting.ok_or_else(|| ProxyError::not_configured(&schema.provider))?;

        let mut est = provider
            .estimate_cost(schema, request)
            .await
            .map_err(|e| ProxyError::ProviderError {
                provider: name,
                error: e.to_string(),
                retryable: false,
            })?;

        price_estimate(&mut est, schema, &request.base, request, self.cost_markup_percent());
        Ok(est)
    }

    /// Estimate cost for a video generation request, through the adapter
    /// [`Self::estimate_image_cost`] would pick.
    pub async fn estimate_video_cost(
        &self,
        schema: &ModelSchema,
        request: &VideoGenerationRequest,
        org: &OrgCredentials,
    ) -> Result<CostEstimate, ProxyError> {
        let mut quoting = None;
        for name in self.quoting_providers(schema) {
            if let Some(p) = self.registry.video_provider_for_request(&name, org.get(&name)).await {
                quoting = Some((name, p));
                break;
            }
        }
        let (name, provider) = quoting.ok_or_else(|| ProxyError::not_configured(&schema.provider))?;

        let mut est = provider
            .estimate_cost(schema, request)
            .await
            .map_err(|e| ProxyError::ProviderError {
                provider: name,
                error: e.to_string(),
                retryable: false,
            })?;

        price_estimate(&mut est, schema, &request.base, request, self.cost_markup_percent());
        Ok(est)
    }

    /// The providers whose adapter a quote for `schema` may go through, in
    /// order: the model's own, then its route's deployments'.
    fn quoting_providers(&self, schema: &ModelSchema) -> Vec<String> {
        let mut names = vec![schema.provider.clone()];
        for name in self.dispatch_route(schema).unwrap_or_default() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }

    /// Whether every vendor a video request for `schema` may reach hands out
    /// a link to the finished video (`VideoProvider::hands_out_links`), which
    /// keeping nothing needs: with a model route in effect, each of its
    /// deployments that can be served, each with the org's credential for its
    /// own provider or the gateway's (FU-ROUTE); otherwise the model's own
    /// provider. One that can't be served takes no job, so it isn't asked.
    pub async fn video_hands_out_links(&self, schema: &ModelSchema, org: &OrgCredentials) -> bool {
        let reached = self.dispatch_route(schema).unwrap_or_else(|| vec![schema.provider.clone()]);
        for name in reached {
            if let Some(p) = self.registry.video_provider_for_request(&name, org.get(&name)).await {
                if !p.hands_out_links(schema) {
                    return false;
                }
            }
        }
        true
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

    /// Whether the gateway holds an instance of provider `name` for `medium`.
    async fn has_provider(&self, medium: crate::capabilities::MediaType, name: &str) -> bool {
        use crate::capabilities::MediaType as M;
        match medium {
            M::Image => self.has_image_provider(name).await,
            M::Video => self.has_video_provider(name).await,
            M::Model3d => self.has_model3d_provider(name).await,
        }
    }

    /// The providers of the model route a generation of `schema` is sent
    /// through, each once in the route's order (FU-ROUTE). `None` when the
    /// model's own provider serves it: no route names it, the route has no
    /// deployments (the router's own answer stands), or it is a segmentation
    /// or 3D model, which routes never apply to (`segment_image`,
    /// `generate_model3d`).
    pub fn dispatch_route(&self, schema: &ModelSchema) -> Option<Vec<String>> {
        use crate::capabilities::MediaType as M;
        let routed = match schema.media_type {
            M::Image => schema.task != crate::capabilities::ModelTask::Segment,
            M::Video => true,
            M::Model3d => false,
        };
        if !routed {
            return None;
        }
        let mut providers: Vec<String> = Vec::new();
        for d in self.find_model_route(&schema.id)?.deployments {
            if !providers.contains(&d.provider) {
                providers.push(d.provider);
            }
        }
        (!providers.is_empty()).then_some(providers)
    }

    /// The providers of the model route that serves `model`, each once, or
    /// none when no route does: the handlers resolve the org's credential
    /// for each one (BYO-ROUTE).
    pub fn route_providers(&self, model: &str) -> Vec<String> {
        let mut providers: Vec<String> = self
            .find_model_route(model)
            .map(|r| r.deployments.into_iter().map(|d| d.provider).collect())
            .unwrap_or_default();
        providers.sort();
        providers.dedup();
        providers
    }

    // ─── Helpers ────────────────────────────────────────────────────────

    /// Whether the cache is on for a model whose route (if any) is `route`,
    /// and the time to live of what it stores: as the route says, else as the
    /// gateway's settings say (`docs/gateway-settings.md`).
    fn cache_for(&self, route: Option<&ModelRoute>) -> (bool, std::time::Duration) {
        match route.and_then(|r| r.cache.as_ref()) {
            Some(c) => (c.enabled, std::time::Duration::from_secs(c.ttl_seconds)),
            None => {
                let settings = self.settings().cache;
                (settings.enabled, settings.ttl)
            }
        }
    }

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

/// Whether a `model_routes` pattern names `model`: `*` names every model,
/// `<prefix>/*` every model under that prefix, anything else the one id
/// (case-insensitively).
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
        let direction = image.metadata.get("direction").and_then(|v| v.as_str()).map(String::from);
        // The size it came back at, from its header (3D doc §4: an expanded
        // canvas is reported as delivered).
        let size = image::ImageReader::new(std::io::Cursor::new(&image.data))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok());
        results.push(ImageResult {
            expires_at: None,
            url,
            b64_json,
            revised_prompt: revised_prompt.clone(),
            view,
            yaw_deg,
            direction,
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
        ProxyError::VendorFailed { model: schema.id.clone(), provider: schema.provider.clone(), vendor: VendorError::from_provider(&unreadable), vendor_job_id: None, retryable: false }
    })
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
/// A generation that keeps nothing hands out the vendor's own files: each one's
/// `source_url`, unconverted, as a `kept: false` asset. A file with no http(s)
/// link (bytes alone) has nothing to hand out, so the generation fails.
pub fn keep_nothing_model3d_assets(files: &[crate::providers::Model3dFile]) -> Result<Vec<Model3dAsset>, String> {
    files
        .iter()
        .map(|f| {
            let url = f
                .source_url
                .as_deref()
                .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
                .ok_or_else(|| {
                    let what = match f.kind {
                        Model3dAssetKind::Mesh => "mesh",
                        Model3dAssetKind::Texture => "texture",
                        Model3dAssetKind::Preview => "preview",
                        Model3dAssetKind::Original => "original file",
                        Model3dAssetKind::Package => "package",
                    };
                    format!("this generation keeps nothing (keep_outputs: false), and the vendor's {what} came without an http(s) link to hand out")
                })?;
            Ok(Model3dAsset {
                expires_at: None,
                kind: f.kind,
                url: url.to_string(),
                format: crate::proxy::storage::sanitize_model3d_format(&f.format),
                size_bytes: None,
                polycount: f.polycount,
                width: f.width,
                height: f.height,
                sha256: None,
                scene: None,
                kept: false,
            })
        })
        .collect()
}

pub async fn rehost_model3d_files(
    store: &Arc<dyn crate::proxy::storage::ImageStorage>,
    path_prefix: Option<&str>,
    generation_id: &str,
    files: Vec<crate::providers::Model3dFile>,
) -> Result<Vec<Model3dAsset>, crate::proxy::storage::ImageStoreError> {
    use crate::proxy::storage::{model3d_asset_key, model3d_content_type, sanitize_model3d_format};

    let mut seen: HashMap<(String, String), u32> = HashMap::new();
    let mut assets = Vec::with_capacity(files.len());

    for f in files {
        // The vendor's spelling reaches a storage key and a public URL, so it is
        // normalised here rather than trusted.
        let format = sanitize_model3d_format(&f.format);
        // The stored file's name and extension.
        let (stem, ext): (String, String) = match f.kind {
            Model3dAssetKind::Mesh => ("model".into(), format.clone()),
            Model3dAssetKind::Texture => ("texture".into(), format.clone()),
            Model3dAssetKind::Preview => ("preview".into(), format.clone()),
            Model3dAssetKind::Original => ("original".into(), format.clone()),
            // A zip of the container and its sidecars: `package_obj.zip`.
            Model3dAssetKind::Package => (format!("package_{format}"), "zip".into()),
        };
        let n = seen.entry((stem.clone(), ext.clone())).or_insert(0);
        let name = if *n == 0 { stem } else { format!("{stem}_{n}") };
        *n += 1;

        let key = model3d_asset_key(path_prefix, generation_id, &name, &ext);
        // Measured first: the bytes then move into the store, never copied.
        let (size, sha256) = (f.bytes.len() as u64, sha256_hex(&f.bytes));
        let url = store
            .put(&key, &bytes::Bytes::from(f.bytes), model3d_content_type(&ext, &f.content_type))
            .await?;

        assets.push(Model3dAsset {
            expires_at: None,
            kind: f.kind,
            url,
            format,
            size_bytes: Some(size),
            polycount: f.polycount,
            width: f.width,
            height: f.height,
            sha256: Some(sha256),
            scene: f.manifest,
            kept: true,
        });
    }
    Ok(assets)
}

/// How a stored video was named, and what a generation row records about it.
pub struct StoredVideo {
    pub url: String,
    pub asset: GenerationAsset,
}

/// Largest vendor video litegen downloads to keep.
const MAX_VIDEO_BYTES: usize = 1024 * 1024 * 1024;

/// Why a finished video could not be kept, and whether trying again could help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RehostError {
    /// The vendor handed over nothing litegen can keep: no bytes and no URL, a
    /// URL that isn't public http(s), one that answers 4xx (gone, or needing
    /// the vendor's key), or a file over litegen's limit. The generation has
    /// failed; asking again changes nothing.
    Permanent(String),
    /// A passing failure: the URL's host could not be looked up, the download
    /// broke off, the host answered 5xx, 408 or 429, or litegen's own store
    /// failed. The next poll tries again.
    Transient(String),
}

impl std::fmt::Display for RehostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RehostError::Permanent(why) | RehostError::Transient(why) => f.write_str(why),
        }
    }
}

/// A failed download of the vendor's video, classified.
fn download_failure(e: crate::util::ssrf::CappedDownloadError) -> RehostError {
    use crate::util::ssrf::CappedDownloadError;
    match e {
        CappedDownloadError::Transport(why) => RehostError::Transient(format!("download failed: {why}")),
        CappedDownloadError::Status(status) if status.is_client_error() && !matches!(status.as_u16(), 408 | 429) => {
            RehostError::Permanent(format!("download answered HTTP {status}"))
        }
        CappedDownloadError::Status(status) => RehostError::Transient(format!("download answered HTTP {status}")),
        CappedDownloadError::TooLarge { .. } => RehostError::Permanent("the video is larger than litegen keeps".into()),
    }
}

/// The container a video's link or label names: its path's extension, else
/// its content type, else mp4.
fn video_format_of(url: &str, content_type: Option<&str>) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let ext = path.rsplit_once('/').map_or(path, |(_, last)| last).rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match (ext.as_deref(), content_type.and_then(|c| c.split(';').next()).map(str::trim)) {
        (Some("webm"), _) | (_, Some("video/webm")) => "webm",
        (Some("mov"), _) | (_, Some("video/quicktime")) => "mov",
        (Some("gif"), _) | (_, Some("image/gif")) => "gif",
        _ => "mp4",
    }
}

/// The client a vendor's video or file is downloaded with: one for the
/// process (its resolver holds no state; its pool is keyed by host). Public
/// addresses only, no redirects, no proxy; a transfer may run 600 s, and one
/// silent for 60 s is cut off.
pub(crate) fn vendor_download_client() -> reqwest::Client {
    static CLIENT: once_cell::sync::Lazy<reqwest::Client> = once_cell::sync::Lazy::new(|| {
        crate::util::ssrf::public_only_download_client(std::time::Duration::from_secs(600), std::time::Duration::from_secs(60))
    });
    CLIENT.clone()
}

/// What an observer without the claim reports for a finished video: still
/// processing, short of done, with no link, no bytes and no error.
fn stand_aside_video(poll: &mut crate::providers::VideoGenerationPollResult) {
    poll.status = GenerationStatus::Processing;
    poll.progress = poll.progress.min(99);
    poll.video_url = None;
    poll.video_data = None;
    poll.error = None;
}

/// Poll a video job as `observer` (the client's poll or the poller) and, for
/// a finished video that is kept, claim the right to keep it before any of it
/// is downloaded: an adapter that downloads in its poll (Veo, Bedrock, Sora)
/// asks the gate; for one that names a link, the claim is taken here, after
/// the poll and before [`rehost_video`] downloads it. An observer without the
/// claim stands aside. Keeping nothing (`keep` false) claims nothing: the
/// vendor's link is handed out and nothing is downloaded.
///
/// A claim the poller waited [`REFRESH_LINKS_AFTER`](crate::providers::REFRESH_LINKS_AFTER)
/// or more for reads the job again, since a vendor's link (Replicate's) can
/// expire while it waits.
pub(crate) async fn poll_video_claimed(
    provider: &Arc<dyn crate::providers::VideoProvider>,
    handle: &VideoGenerationHandle,
    keep: bool,
    db: &Arc<dyn crate::db::DatabaseStore>,
    id: &str,
    observer: crate::proxy::delivery_lease::Observer,
) -> Result<(crate::providers::VideoGenerationPollResult, Option<crate::proxy::delivery_lease::DeliveryClaim>), ProviderError> {
    use crate::proxy::delivery_lease::{claim_video_delivery, DeliveryGate, GateState};
    if !keep {
        return Ok((provider.poll_status(handle).await?, None));
    }
    let gate = DeliveryGate::for_video(db, id, observer);
    let mut poll = provider.poll_status_gated(handle, &gate).await?;
    let claim = match gate.into_outcome() {
        GateState::Claimed(claim) => Some(claim),
        GateState::Refused => None,
        GateState::NotAsked if poll.status == GenerationStatus::Completed => {
            let started = tokio::time::Instant::now();
            let claim = claim_video_delivery(db, id, observer).await;
            if claim.is_some() && poll.video_data.is_none() && started.elapsed() >= crate::providers::REFRESH_LINKS_AFTER {
                match provider.poll_status(handle).await {
                    Ok(again) if again.status == GenerationStatus::Completed => poll = again,
                    _ => {}
                }
            }
            claim
        }
        GateState::NotAsked => None,
    };
    if claim.is_none() && poll.status == GenerationStatus::Completed {
        stand_aside_video(&mut poll);
    }
    Ok((poll, claim))
}

/// Keep a finished video on litegen storage (PixelGenie L3): the vendor's own
/// URL may expire within the hour (Replicate), need the vendor's key (Veo),
/// or vanish with the job. Uses the bytes the adapter already downloaded when
/// it has them (moved into the store, hashed and probed on a blocking
/// thread), else fetches `url`, which must be a public http(s) URL: streamed
/// to a spool file (the store's [`spool_dir`](crate::proxy::storage::ImageStorage::spool_dir),
/// else [`video_spool_dir`](crate::proxy::storage::video_spool_dir)), hashed
/// as it arrives, probed from disk and handed to the store from there, so a
/// video of up to 1 GiB costs a chunk of memory. Every local disk failure is
/// passing (the next poll tries again).
///
/// With `keep` false (`keep_outputs: false`), hand out the vendor's own link
/// and store nothing. A video that came back as bytes alone has no link to
/// hand out, so keeping nothing fails it.
///
/// `public_only` is false only where `storage_access.unchecked_vendor_hosts`
/// says so (a test's vendor on loopback): the link is then not checked.
#[allow(clippy::too_many_arguments)]
pub async fn rehost_video(
    store: &Arc<dyn crate::proxy::storage::ImageStorage>,
    path_prefix: Option<&str>,
    generation_id: &str,
    data: Option<Vec<u8>>,
    url: Option<&str>,
    content_type: Option<&str>,
    keep: bool,
    public_only: bool,
) -> Result<StoredVideo, RehostError> {
    if !keep {
        let url = url
            .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
            .ok_or_else(|| RehostError::Permanent(
                "this generation keeps nothing (keep_outputs: false), and the vendor returned no http(s) link to the video to hand out".into(),
            ))?;
        return Ok(StoredVideo {
            asset: GenerationAsset {
                expires_at: None,
                index: 0,
                kind: "video".into(),
                format: video_format_of(url, content_type).into(),
                url: url.to_string(),
                size_bytes: None,
                sha256: None,
                width: None,
                height: None,
                duration_s: None,
                fps: None,
                has_audio: None,
                kept: false,
            },
            url: url.to_string(),
        });
    }
    let format_of = |ct: &str| match ct.split(';').next().unwrap_or("").trim() {
        "image/gif" => "gif",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        _ => "mp4",
    };
    let key_of = |format: &str| {
        format!("{}/{generation_id}/video.{format}", path_prefix.unwrap_or(crate::proxy::storage::VIDEO_PATH_PREFIX).trim_matches('/'))
    };
    let Some(bytes) = data else {
        let url = url.ok_or_else(|| RehostError::Permanent("the vendor returned neither the video nor its URL".into()))?;
        if public_only {
            crate::util::ssrf::check_public_url(url).await.map_err(|e| match e {
                // A resolver blip, not a verdict on the video: as the 3D download.
                crate::util::ssrf::UrlCheckError::Lookup(why) => RehostError::Transient(format!("the video's host could not be looked up: {why}")),
                crate::util::ssrf::UrlCheckError::Refused(why) => RehostError::Permanent(why),
            })?;
        }
        // Streamed to a spool file on disk, hashed as it arrives, then handed
        // to the store from there: the video is never held in memory.
        let spool = SpoolFile::create(store.spool_dir().unwrap_or_else(crate::proxy::storage::video_spool_dir)).await?;
        // The path is the log's (payloads review #3), as in `SpoolFile::create`.
        let mut file = tokio::fs::File::create(&spool.0).await.map_err(|e| {
            tracing::warn!(dir = %spool.dir().display(), error = %e, "could not open a video spool file");
            RehostError::Transient("could not open a spool file for the video".into())
        })?;
        let got = crate::util::ssrf::download_to_file(&vendor_download_client(), url, MAX_VIDEO_BYTES as u64, &mut file)
            .await
            .map_err(download_failure)?;
        drop(file);
        let ct = got.content_type.or_else(|| content_type.map(str::to_string)).unwrap_or_else(|| "video/mp4".into());
        let format = format_of(&ct);
        let path = spool.0.clone();
        let media = tokio::task::spawn_blocking(move || crate::proxy::video_probe::probe_file(&path))
            .await
            .map_err(|e| RehostError::Transient(format!("could not read the video: {e}")))?;
        let url = store
            .put_file(&key_of(format), &spool.0, crate::proxy::storage::model3d_content_type(format, &ct))
            .await
            .map_err(|e| RehostError::Transient(format!("could not store the video: {e}")))?;
        return Ok(StoredVideo { asset: kept_video_asset(format, &url, got.len, got.sha256, media), url });
    };
    let ct = content_type.unwrap_or("video/mp4").to_string();
    let format = format_of(&ct);
    let key = key_of(format);
    // What came back, read from the bytes (PixelGenie L10), and its hash: on
    // a blocking thread, since a video may be 1 GiB. The bytes then move into
    // the store, never copied.
    let (bytes, sha256, media) = tokio::task::spawn_blocking(move || {
        let sha256 = sha256_hex(&bytes);
        let media = crate::proxy::video_probe::probe(&bytes);
        (bytes, sha256, media)
    })
    .await
    .map_err(|e| RehostError::Transient(format!("could not read the video: {e}")))?;
    let size = bytes.len() as u64;
    let url = store
        .put(&key, &bytes::Bytes::from(bytes), crate::proxy::storage::model3d_content_type(format, &ct))
        .await
        .map_err(|e| RehostError::Transient(format!("could not store the video: {e}")))?;
    Ok(StoredVideo { asset: kept_video_asset(format, &url, size, sha256, media), url })
}

/// What a generation row records about a video litegen kept.
fn kept_video_asset(format: &str, url: &str, size: u64, sha256: String, media: crate::types::VideoMedia) -> GenerationAsset {
    GenerationAsset {
        expires_at: None,
        index: 0,
        kind: "video".into(),
        format: format.into(),
        url: url.to_string(),
        size_bytes: Some(size),
        sha256: Some(sha256),
        width: media.width,
        height: media.height,
        duration_s: media.duration_s,
        fps: media.fps,
        has_audio: media.has_audio,
        kept: true,
    }
}

/// A file a kept video waits in between its download and the store
/// (`litegen-video-<uuid>.part` in the spool), removed when dropped: on every
/// path, a failure included. After a rename into place there is nothing left
/// to remove. A crash leaves it to `proxy::storage::sweep_spool`.
struct SpoolFile(std::path::PathBuf);

impl SpoolFile {
    async fn create(dir: std::path::PathBuf) -> Result<Self, RehostError> {
        // The path is the log's: this text can end up as the generation's
        // stored error, which its caller reads (payloads review #3).
        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            tracing::warn!(dir = %dir.display(), error = %e, "could not make the video spool directory");
            RehostError::Transient("could not make the spool directory for the video".into())
        })?;
        let name = format!(
            "{}{}{}",
            crate::proxy::storage::SPOOL_FILE_PREFIX,
            uuid::Uuid::new_v4().simple(),
            crate::proxy::storage::SPOOL_FILE_SUFFIX
        );
        Ok(Self(dir.join(name)))
    }

    fn dir(&self) -> &std::path::Path {
        self.0.parent().unwrap_or(&self.0)
    }
}

impl Drop for SpoolFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// ─── Proxy Error ────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("Model not found: {model}")]
    ModelNotFound { model: String },

    /// The provider has no usable credential: no instance at all, or the
    /// adapter's own verdict on the one it has (`detail`, e.g. Kling's "store
    /// a Kling API key instead").
    #[error("Provider not configured: {provider}{}", detail.as_ref().map(|d| format!(" ({d})")).unwrap_or_default())]
    ProviderNotConfigured { provider: String, detail: Option<String> },

    #[error("No deployments configured for model: {model}")]
    NoDeployments { model: String },

    /// No vendor of a route was called, and the request may work later: its
    /// deployments were skipped for different reasons (a breaker open, a
    /// provider missing), or one refused it after another was skipped.
    /// `last_error` says why each did not serve it.
    #[error("All deployments failed for model {model}{}", last_error.as_ref().map(|e| format!(": {e}")).unwrap_or_default())]
    AllDeploymentsFailed {
        model: String,
        last_error: Option<String>,
    },

    /// Every deployment of a route was skipped by its open circuit breaker,
    /// so nothing was asked (handlers review #5). The same request may work
    /// once the first of them closes, `retry_after_secs` from now.
    #[error("Every deployment of model {model} is cooling down after failures ({skipped}); retry in {retry_after_secs} s")]
    CoolingDown {
        model: String,
        retry_after_secs: u64,
        skipped: String,
    },

    /// The adapter refused the request before calling its vendor (E9): a
    /// combination the vendor can't make, a model it doesn't serve, a price
    /// it can't work out. The caller's to change; nothing was sent, so
    /// nothing was billed and an `Idempotency-Key` isn't spent.
    #[error("{message}")]
    Refused { model: String, provider: String, message: String },

    #[error("Provider {provider} error: {error}")]
    ProviderError {
        provider: String,
        error: String,
        retryable: bool,
    },

    /// The vendor's free preflight (3D doc §3B) failed: nothing was
    /// submitted, so nothing was billed.
    #[error("{provider} preflight failed: {error}")]
    PreflightFailed {
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
        /// Whether the same request may work later: by the vendor's status,
        /// or, with none, by the adapter's verdict; never once the vendor
        /// accepted the job.
        retryable: bool,
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
    /// `provider` has no instance this request can use.
    pub fn not_configured(provider: &str) -> Self {
        Self::ProviderNotConfigured { provider: provider.to_string(), detail: None }
    }

    /// The provider whose deployment this error came from, when it names
    /// one. On a model route that is the deployment that failed, which need
    /// not be the model's own provider (handlers review #8).
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::ProviderNotConfigured { provider, .. }
            | Self::Refused { provider, .. }
            | Self::ProviderError { provider, .. }
            | Self::PreflightFailed { provider, .. }
            | Self::VendorRejected { provider, .. }
            | Self::VendorFailed { provider, .. }
            | Self::VendorTimeout { provider, .. } => Some(provider),
            _ => None,
        }
    }

    /// The error as a record a tenant can read keeps it (a failed request's
    /// log row and error artifact, `/v1/logs`): its own text, except an
    /// internal error's, which can carry a panic's (its file, line and
    /// message). That one is kept as "internal server error", as the answer
    /// says it; the server log has the detail (handlers review #7, the
    /// SEC-DBERR class).
    pub fn logged_message(&self) -> String {
        match self {
            Self::Internal(_) => "internal server error".to_string(),
            other => other.to_string(),
        }
    }

    pub fn status_code(&self) -> u16 {
        match self {
            Self::ModelNotFound { .. } => 404,
            // The model's provider has no usable credential (no per-app BYO key and
            // no platform default): a configuration problem, not a transient outage,
            // so not retryable. The documented BYO answer (README), the same 400
            // the handlers give when they notice it first (E8).
            Self::ProviderNotConfigured { .. } | Self::Refused { .. } => 400,
            Self::NoDeployments { .. } | Self::CoolingDown { .. } => 503,
            Self::AllDeploymentsFailed { .. } => 502,
            Self::ProviderError { retryable, .. } | Self::PreflightFailed { retryable, .. } => {
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

    /// Whether the same request may work if sent again (PixelGenie L15) —
    /// never once the vendor accepted a job, which may still run and bill.
    pub fn retryable(&self) -> bool {
        match self {
            Self::VendorTimeout { vendor_job_id, .. } => vendor_job_id.is_none(),
            Self::VendorFailed { retryable, .. } => *retryable,
            Self::VendorRejected { .. } => false,
            Self::ProviderError { retryable, .. } | Self::PreflightFailed { retryable, .. } => *retryable,
            Self::NoDeployments { .. } | Self::AllDeploymentsFailed { .. } | Self::CoolingDown { .. } | Self::Internal(_) => true,
            Self::ModelNotFound { .. } | Self::ProviderNotConfigured { .. } | Self::Refused { .. } | Self::NotFound(_) => false,
        }
    }

    /// How many seconds until the same request may work, when litegen knows
    /// (`Retry-After`): a route whose every breaker is open.
    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::CoolingDown { retry_after_secs, .. } => Some(*retry_after_secs),
            _ => None,
        }
    }

    /// Whether this failure happened after a vendor was called — i.e. whether
    /// the vendor may have done (and billed) work. An `Idempotency-Key`
    /// records such an outcome; a refusal before any vendor call does not
    /// spend the key.
    pub fn reached_vendor(&self) -> bool {
        match self {
            Self::ModelNotFound { .. } | Self::ProviderNotConfigured { .. } | Self::NoDeployments { .. } => false,
            // The adapter's own verdict, before anything was sent (E9).
            Self::Refused { .. } => false,
            // Built only when no vendor was called: deployments skipped (a
            // breaker open, a provider missing) or refused before any call.
            Self::AllDeploymentsFailed { .. } | Self::CoolingDown { .. } | Self::PreflightFailed { .. } => false,
            Self::ProviderError { .. }
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
            ProviderError::RateLimited { provider_error, message, .. } => (Some(429), provider_error.clone(), message.clone()),
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
/// timeout a timeout. The adapter's own refusal before any call (E9) is the
/// caller's to fix, [`ProxyError::Refused`], or the provider's missing
/// credential, [`ProxyError::ProviderNotConfigured`] with the adapter's
/// advice: neither reached a vendor. Once the vendor took a job, though, any
/// error is a vendor attempt that names it: the job may still bill.
fn failure(model: &str, provider: &str, last: Option<ProviderError>, vendor_job_id: Option<String>) -> ProxyError {
    let Some(err) = last else {
        return ProxyError::AllDeploymentsFailed { model: model.to_string(), last_error: None };
    };
    let (model, provider) = (model.to_string(), provider.to_string());
    if err.before_any_call() && vendor_job_id.is_none() {
        return match err {
            ProviderError::NotConfigured(detail) => ProxyError::ProviderNotConfigured { provider, detail: Some(detail) },
            ProviderError::InvalidRequest(message) => ProxyError::Refused { model, provider, message },
            other => ProxyError::Refused { model, provider, message: other.to_string() },
        };
    }
    match &err {
        ProviderError::Timeout { .. } => ProxyError::VendorTimeout { model, provider, vendor_job_id },
        ProviderError::RequestFailed { status_code: Some(s), .. } if (400..500).contains(s) && !matches!(s, 408 | 429) => {
            ProxyError::VendorRejected { model, provider, vendor: VendorError::from_provider(&err), vendor_job_id }
        }
        ProviderError::RequestFailed { .. } | ProviderError::RateLimited { .. } => {
            let vendor = VendorError::from_provider(&err);
            // A status says; with none (a moderation verdict, a task that
            // failed) the adapter's own verdict does. A job the vendor took
            // may still run and bill, so it is never one to send again.
            let retryable = vendor_job_id.is_none() && if vendor.status.is_some() { vendor.retryable() } else { err.is_retryable() };
            ProxyError::VendorFailed { model, provider, vendor, vendor_job_id, retryable }
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

/// How long to wait before retry `retry` (counted from 1) of a call that
/// failed with `err` (`docs/gateway-settings.md`): a random share (`jitter`,
/// 0 to 1) of `min(max_backoff, backoff · 2^(retry−1))`, "full jitter", and
/// never less than the vendor's `Retry-After`. `None`: don't retry, as the
/// vendor asked for longer than the longest wait; the caller backs off
/// instead of litegen holding the request open.
fn retry_delay(retry: u32, err: &ProviderError, settings: &crate::management::settings::RetrySettings, jitter: f64) -> Option<std::time::Duration> {
    let doubling = 2u32.saturating_pow(retry.saturating_sub(1).min(31));
    let cap = settings.backoff.saturating_mul(doubling).min(settings.max_backoff);
    let wait = cap.mul_f64(jitter.clamp(0.0, 1.0));
    match err {
        ProviderError::RateLimited { retry_after_ms, .. } => {
            let floor = std::time::Duration::from_millis(*retry_after_ms);
            (floor <= settings.max_backoff).then(|| wait.max(floor))
        }
        _ => Some(wait),
    }
}

/// What a route's deployments did, for the error a caller sees when none
/// made the result (E9): which were passed over and why, the last failure
/// that reached a vendor, and the last refusal before any call.
#[derive(Default)]
struct Attempts {
    skipped: Vec<(String, Skip)>,
    last_reached: Option<(String, ProviderError)>,
    last_refused: Option<(String, ProviderError)>,
}

/// Why a route's deployment was passed over without a vendor call.
enum Skip {
    /// Its breaker is open, for this much longer.
    Open(std::time::Duration),
    /// Its provider has no instance this request can use, or its adapter
    /// found the credential unusable, with the adapter's advice (E9).
    NotConfigured(Option<String>),
}

impl Attempts {
    /// `provider`'s breaker is open, for `left` more.
    fn skipped_open(&mut self, provider: &str, left: std::time::Duration) {
        self.skipped.push((provider.to_string(), Skip::Open(left)));
    }

    /// `provider` has no instance this request can use.
    fn skipped_unconfigured(&mut self, provider: &str) {
        self.skipped.push((provider.to_string(), Skip::NotConfigured(None)));
    }

    fn failed(&mut self, provider: &str, err: ProviderError) {
        // An adapter that finds no usable credential says what the registry
        // says when it has no instance: the deployment is not configured
        // (E8: one condition, one answer).
        if let ProviderError::NotConfigured(advice) = err {
            self.skipped.push((provider.to_string(), Skip::NotConfigured(Some(advice))));
            return;
        }
        let slot = if err.before_any_call() { &mut self.last_refused } else { &mut self.last_reached };
        *slot = Some((provider.to_string(), err));
    }

    /// The answer when no deployment made the result (E9, handlers review
    /// #4 and #5):
    /// - a vendor attempt wins: it may have billed, so the caller's key must
    ///   record it;
    /// - with nothing passed over, the last refusal: the caller's to fix;
    /// - every deployment not configured: what a direct request to its
    ///   provider answers, the 400 `provider_not_configured`;
    /// - every deployment's breaker open: a retryable 503 for when the first
    ///   one closes (`Retry-After`);
    /// - any other mix (both kinds of skip, or a refusal after a skip, which
    ///   the skipped deployment may serve once its breaker closes): a
    ///   retryable 502 that names why each deployment did not serve it.
    ///
    /// None of these but the first reached a vendor.
    fn into_error(self, model: &str) -> ProxyError {
        if let Some((provider, err)) = self.last_reached {
            return failure(model, &provider, Some(err), None);
        }
        if self.skipped.is_empty() {
            return match self.last_refused {
                Some((provider, err)) => failure(model, &provider, Some(err), None),
                None => ProxyError::AllDeploymentsFailed { model: model.to_string(), last_error: None },
            };
        }
        let named = self
            .skipped
            .iter()
            .map(|(provider, skip)| skip.named(provider))
            .chain(self.last_refused.iter().map(|(provider, err)| format!("{provider}: {err}")))
            .collect::<Vec<_>>()
            .join("; ");
        if self.last_refused.is_none() {
            if self.skipped.iter().all(|(_, skip)| matches!(skip, Skip::NotConfigured(_))) {
                return not_configured_route(model, self.skipped);
            }
            let open: Option<Vec<std::time::Duration>> =
                self.skipped.iter().map(|(_, skip)| if let Skip::Open(left) = skip { Some(*left) } else { None }).collect();
            if let Some(shortest) = open.and_then(|lefts| lefts.into_iter().min()) {
                let retry_after_secs = u64::try_from(shortest.as_millis()).unwrap_or(u64::MAX).div_ceil(1000).max(1);
                return ProxyError::CoolingDown { model: model.to_string(), retry_after_secs, skipped: named };
            }
        }
        ProxyError::AllDeploymentsFailed { model: model.to_string(), last_error: Some(named) }
    }
}

impl Skip {
    /// `provider: why`, as an error names a passed-over deployment.
    fn named(&self, provider: &str) -> String {
        match self {
            Skip::Open(_) => format!("{provider}: circuit open"),
            Skip::NotConfigured(Some(advice)) if advice != provider => format!("{provider}: not configured ({advice})"),
            Skip::NotConfigured(_) => format!("{provider}: not configured"),
        }
    }
}

/// A route none of whose deployments is configured answers as a direct
/// request to its provider would (handlers review #5): 400
/// `provider_not_configured`, naming the first deployment's provider and its
/// adapter's advice when it is the only one, and every provider when there
/// are several. `skipped` holds only [`Skip::NotConfigured`], at least one.
fn not_configured_route(model: &str, skipped: Vec<(String, Skip)>) -> ProxyError {
    let mut providers: Vec<String> = Vec::new();
    let mut advice = None;
    for (provider, skip) in skipped {
        if let Skip::NotConfigured(Some(a)) = skip {
            advice.get_or_insert(a);
        }
        if !providers.contains(&provider) {
            providers.push(provider);
        }
    }
    let detail = match providers.len() {
        0 | 1 => advice,
        _ => Some(format!(
            "no deployment of model {model}'s route is configured ({}); add a provider credential in the dashboard",
            providers.join(", ")
        )),
    };
    ProxyError::ProviderNotConfigured { provider: providers.into_iter().next().unwrap_or_default(), detail }
}

/// `last_error`'s text, naming the vendor job when one was accepted.
fn with_vendor_job(error: String, vendor_job_id: Option<String>) -> String {
    match vendor_job_id {
        Some(j) => format!("{error} (vendor job {j})"),
        None => error,
    }
}

#[cfg(test)]
mod rehost_tests {
    use super::*;
    use crate::proxy::storage::{ImageStorage, LocalModel3dStorage};
    use crate::util::ssrf::CappedDownloadError;

    /// Review 2026-11-11 P4: a video kept from its link reports the hash and
    /// the media of what was stored, and leaves no spool file behind.
    #[tokio::test]
    async fn a_kept_video_reports_the_hash_and_media_of_what_was_stored() {
        use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
        let gif = crate::providers::video::visual_mock::generate_sprite_gif(2.0, 4, true);
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_raw(gif.clone(), "image/gif")).mount(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn ImageStorage> = Arc::new(LocalModel3dStorage::new(dir.path(), "https://h".into()));
        let url = format!("{}/v.gif", server.uri());
        let Ok(kept) = rehost_video(&store, None, "g", None, Some(&url), None, true, false).await else { panic!("kept") };
        assert_eq!(kept.url, "https://h/v1/models3d/assets/litegen/video/g/video.gif");
        let a = &kept.asset;
        assert_eq!((a.format.as_str(), a.size_bytes, a.kept), ("gif", Some(gif.len() as u64), true));
        assert_eq!(a.sha256.as_deref(), Some(sha256_hex(&gif).as_str()));
        let media = crate::proxy::video_probe::probe(&gif);
        assert_eq!((a.width, a.height, a.duration_s, a.fps, a.has_audio), (media.width, media.height, media.duration_s, media.fps, media.has_audio));
        assert!(a.duration_s.is_some(), "the media was read: {a:?}");
        assert_eq!(std::fs::read(dir.path().join("litegen/video/g/video.gif")).unwrap(), gif);
        let left: Vec<_> = std::fs::read_dir(dir.path().join("litegen/.spool")).unwrap().flatten().collect();
        assert!(left.is_empty(), "the spool file is gone: {left:?}");

        // A download that fails leaves nothing in the spool either.
        let gone = format!("{}/missing", MockServer::start().await.uri());
        assert!(rehost_video(&store, None, "g2", None, Some(&gone), None, true, false).await.is_err());
        assert_eq!(std::fs::read_dir(dir.path().join("litegen/.spool")).unwrap().count(), 0);
    }

    /// Payloads review #3: a spool failure's text names no server path. A
    /// keep that keeps failing ends at the reaper, which stores this text as
    /// the generation's error for its caller to read; the path is the log's.
    #[tokio::test]
    async fn a_spool_failure_names_no_server_path() {
        let link = Some("http://127.0.0.1:9/v.mp4");
        // `litegen` is a file, so the spool directory under it can't be made.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("litegen"), b"not a directory").unwrap();
        let store: Arc<dyn ImageStorage> = Arc::new(LocalModel3dStorage::new(dir.path(), "https://h".into()));
        let failed = rehost_video(&store, None, "g", None, link, None, true, false).await;
        let Err(RehostError::Transient(why)) = &failed else { panic!("a passing failure: {:?}", failed.as_ref().err()) };
        let root = dir.path().display().to_string();
        assert!(!why.contains(&root), "the spool directory's failure names a server path: {why}");

        // A spool directory litegen may not write in (not for root, whom no
        // mode stops: then this half can't be set up).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let spool = dir.path().join("litegen/.spool");
            std::fs::create_dir_all(&spool).unwrap();
            std::fs::set_permissions(&spool, std::fs::Permissions::from_mode(0o555)).unwrap();
            if std::fs::File::create(spool.join("probe")).is_ok() {
                eprintln!("the spool's mode doesn't stop this user (root): the unwritable-spool half is skipped");
            } else {
                let store: Arc<dyn ImageStorage> = Arc::new(LocalModel3dStorage::new(dir.path(), "https://h".into()));
                let failed = rehost_video(&store, None, "g", None, link, None, true, false).await;
                let _ = std::fs::set_permissions(&spool, std::fs::Permissions::from_mode(0o755));
                let Err(RehostError::Transient(why)) = &failed else { panic!("a passing failure: {:?}", failed.as_ref().err()) };
                let root = dir.path().display().to_string();
                assert!(!why.contains(&root), "the spool file's failure names a server path: {why}");
            }
            let _ = std::fs::set_permissions(&spool, std::fs::Permissions::from_mode(0o755));
        }
    }

    /// A store that counts what it keeps, by bytes or by file.
    struct Counting {
        inner: Arc<dyn ImageStorage>,
        puts: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ImageStorage for Counting {
        async fn put(&self, key: &str, bytes: &bytes::Bytes, content_type: &str) -> Result<String, crate::proxy::storage::ImageStoreError> {
            self.puts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.put(key, bytes, content_type).await
        }
        async fn put_file(&self, key: &str, path: &std::path::Path, content_type: &str) -> Result<String, crate::proxy::storage::ImageStoreError> {
            self.puts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.put_file(key, path, content_type).await
        }
        async fn delete(&self, key: &str) -> Result<(), crate::proxy::storage::ImageStoreError> {
            self.inner.delete(key).await
        }
    }

    /// Review 2026-11-11 P4: the client's poll and the poller both see the
    /// video finish; one of them keeps it, and the row completes.
    #[tokio::test]
    async fn two_observers_keep_a_finished_video_exactly_once() {
        use crate::providers::VideoProvider;
        let db: Arc<dyn crate::db::DatabaseStore> = Arc::new(crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let mut mock = crate::providers::video::mock::MockVideoProvider::new();
        mock.configure(crate::providers::ProviderInstanceConfig::default());
        let provider: Arc<dyn VideoProvider> = Arc::new(mock);
        let registry = Arc::new(crate::proxy::registry::ProviderRegistry::new());
        registry.register_mock_video(provider.clone()).await;
        let router = ProxyRouter::new(
            registry.clone(),
            Arc::new(crate::proxy::cache::GenerationCache::new(&crate::config::CacheGlobalConfig::default())),
            Arc::new(crate::config::AppConfig::default()),
            Arc::new(crate::proxy::storage::LocalStore),
        );
        let id = "g-vid-twice";
        let handle = VideoGenerationHandle { provider_job_id: "mock-video-job-1".into(), provider: "mock".into(), model: "mock/video-gen".into() };
        router.video_jobs.write().await.insert(id.into(), VideoJob { handle, usage: None, provider, submitted: tokio::time::Instant::now() });
        db.insert_generation(id, None, "mock/video-gen", "mock", "video", Some("mock-video-job-1"), 0.0, None, None).await.unwrap();
        let counting = Arc::new(Counting {
            inner: Arc::new(LocalModel3dStorage::new(tempfile::tempdir().unwrap().keep(), "https://h".into())),
            puts: Default::default(),
        });
        let store: Arc<dyn ImageStorage> = counting.clone();

        let (client, _) = tokio::join!(
            router.get_video_status_claimed(id, Some(store.clone()), None, true, &db),
            crate::proxy::poller::poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store),
        );
        let (client, assets, claim) = client.expect("the client's poll answers");
        if client.status == GenerationStatus::Completed {
            // The client won: its handler writes the outcome while it holds
            // the claim, as here.
            assert!(claim.is_some());
            db.update_generation_if_active(id, "completed", 100, client.video_url.as_deref(), None, Some(chrono::Utc::now()), assets.as_ref())
                .await
                .unwrap();
        } else {
            assert_eq!(client.status, GenerationStatus::Processing, "{client:?}");
            assert!(client.video_url.is_none());
        }
        drop(claim);
        assert_eq!(counting.puts.load(std::sync::atomic::Ordering::SeqCst), 1, "one observer kept the video");
        let row = db.get_generation(id).await.unwrap().unwrap();
        assert_eq!(row.status, GenerationStatus::Completed, "{:?}", row.error_message);
    }

    #[test]
    fn download_failures_are_permanent_or_transient() {
        let permanent = |e: CappedDownloadError| matches!(download_failure(e), RehostError::Permanent(_));
        assert!(permanent(CappedDownloadError::Status(reqwest::StatusCode::NOT_FOUND)), "gone");
        assert!(permanent(CappedDownloadError::Status(reqwest::StatusCode::FORBIDDEN)), "needs the vendor's key");
        assert!(permanent(CappedDownloadError::TooLarge { bytes: 9, max: 1 }));
        assert!(!permanent(CappedDownloadError::Status(reqwest::StatusCode::SERVICE_UNAVAILABLE)));
        assert!(!permanent(CappedDownloadError::Status(reqwest::StatusCode::TOO_MANY_REQUESTS)));
        assert!(!permanent(CappedDownloadError::Status(reqwest::StatusCode::REQUEST_TIMEOUT)));
        assert!(!permanent(CappedDownloadError::Transport("connection reset".into())));
    }

    #[tokio::test]
    async fn keeping_nothing_hands_out_the_vendor_link_and_stores_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn ImageStorage> = Arc::new(LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000".into()));
        let url = "https://cdn.vendor.example/out/clip.webm?sig=abc";
        let Ok(kept) = rehost_video(&store, None, "g", None, Some(url), Some("video/webm"), false, true).await else { panic!("a link to hand out") };
        assert_eq!((kept.url.as_str(), kept.asset.url.as_str()), (url, url));
        assert!(!kept.asset.kept, "litegen kept no copy");
        assert_eq!(kept.asset.format, "webm");
        assert!(kept.asset.size_bytes.is_none() && kept.asset.sha256.is_none(), "nothing was read");
        // Bytes and a link: the link, and still nothing written.
        let Ok(kept) = rehost_video(&store, None, "g", Some(b"GIF89a".to_vec()), Some("https://cdn.vendor.example/a"), Some("image/gif"), false, true).await else { panic!("the link") };
        assert_eq!(kept.asset.format, "gif");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "nothing written");
    }

    #[tokio::test]
    async fn keeping_nothing_fails_a_video_without_an_http_link() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn ImageStorage> = Arc::new(LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000".into()));
        match rehost_video(&store, None, "g", Some(b"GIF89a".to_vec()), None, Some("image/gif"), false, true).await {
            Err(RehostError::Permanent(why)) => assert!(why.contains("keeps nothing"), "{why}"),
            Err(RehostError::Transient(why)) => panic!("transient: {why}"),
            Ok(_) => panic!("bytes alone have no link to hand out"),
        }
        for url in [Some("javascript:alert(1)"), Some("/mock/video/1"), None] {
            assert!(matches!(rehost_video(&store, None, "g", None, url, None, false, true).await, Err(RehostError::Permanent(_))), "{url:?}");
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "nothing written");
    }

    #[tokio::test]
    async fn nothing_to_keep_or_a_refused_url_is_permanent_and_a_lookup_or_store_failure_transient() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn ImageStorage> = Arc::new(LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000".into()));
        for (url, what) in [(None, "no video, no URL"), (Some("http://127.0.0.1:9/v.mp4"), "a private URL"), (Some("ftp://example.com/v.mp4"), "not http")] {
            assert!(matches!(rehost_video(&store, None, "g", None, url, None, true, true).await, Err(RehostError::Permanent(_))), "{what}");
        }
        // A host that can't be looked up right now is a passing failure (a DNS
        // blip), not a verdict on the video: `.invalid` never resolves (RFC 6761).
        assert!(
            matches!(rehost_video(&store, None, "g", None, Some("https://video.invalid/v.mp4"), None, true, true).await, Err(RehostError::Transient(_))),
            "a failed lookup is retried"
        );
        // litegen's own store failing is litegen's problem, not the video's.
        let file = dir.path().join("not-a-dir");
        std::fs::write(&file, b"x").unwrap();
        let broken: Arc<dyn ImageStorage> = Arc::new(LocalModel3dStorage::new(&file, "http://127.0.0.1:4000".into()));
        assert!(matches!(
            rehost_video(&broken, None, "g", Some(b"GIF89a".to_vec()), None, Some("image/gif"), true, true).await,
            Err(RehostError::Transient(_))
        ));
    }
}

#[cfg(test)]
mod overrides_tests {
    use super::*;
    use crate::config::{AppConfig, CacheGlobalConfig};
    use crate::proxy::storage::LocalStore;

    fn router() -> ProxyRouter {
        ProxyRouter::new(
            Arc::new(ProviderRegistry::new()),
            Arc::new(GenerationCache::new(&CacheGlobalConfig::default())),
            Arc::new(AppConfig::default()),
            Arc::new(LocalStore),
        )
    }

    /// Every signing and verifying path reads `router.link_signer`, so the
    /// signer `with_link_signer` installs is the one they use.
    #[test]
    fn with_link_signer_replaces_the_signer() {
        let key = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [5u8; 32]);
        let signer = Arc::new(crate::access::signing::LinkSigner::from_secrets_key(Some(&key)));
        let r = router().with_link_signer(signer.clone());
        assert!(Arc::ptr_eq(&r.link_signer, &signer));
        assert_eq!(r.link_signer.sign("k", 1), signer.sign("k", 1));
        assert!(r.link_signer.persistent());
    }

    fn with_retries(n: u32) -> RuntimeOverrides {
        RuntimeOverrides {
            model_routes: None,
            gateway: crate::management::types::GatewaySettings { retry_max_retries: Some(n), ..Default::default() },
        }
    }

    /// A settings refresh read the database before a save landed: its stale
    /// read must not undo the save (`docs/gateway-settings.md`).
    #[test]
    fn a_refresh_that_raced_a_save_does_not_undo_it() {
        let r = router();
        let read_at = r.overrides_version();
        r.set_overrides(with_retries(4)); // the save, landing mid-refresh
        assert!(!r.set_overrides_unless_changed(read_at, with_retries(0)), "the stale read is dropped");
        assert_eq!(r.settings().retries.max_retries, 4);
        let now = r.overrides_version();
        assert!(r.set_overrides_unless_changed(now, with_retries(1)), "an up-to-date read applies");
        assert_eq!(r.settings().retries.max_retries, 1);
    }
}

#[cfg(test)]
mod failure_tests {
    use super::*;

    /// E8: a provider with no usable credential is the documented 400
    /// `provider_not_configured` (README, the BYO contract), wherever it is
    /// noticed: the router used to answer 424.
    #[test]
    fn provider_not_configured_is_the_documented_400() {
        assert_eq!(ProxyError::not_configured("x").status_code(), 400);
    }

    /// Handlers review #5: a route none of whose deployments is configured
    /// answers as a direct request to its provider would: 400
    /// `provider_not_configured`, not retryable, no vendor reached. It
    /// answered a retryable 502, though no retry can help.
    #[test]
    fn a_route_with_no_configured_deployment_answers_as_a_direct_request() {
        let mut one = Attempts::default();
        one.skipped_unconfigured("fal");
        let e = one.into_error("m");
        assert!(matches!(&e, ProxyError::ProviderNotConfigured { provider, .. } if provider == "fal"), "{e:?}");
        assert_eq!((e.status_code(), e.retryable(), e.reached_vendor()), (400, false, false), "{e}");
        let mut two = Attempts::default();
        two.skipped_unconfigured("fal");
        two.skipped_unconfigured("replicate");
        let e = two.into_error("m");
        assert!(matches!(&e, ProxyError::ProviderNotConfigured { .. }), "{e:?}");
        assert_eq!((e.status_code(), e.retryable()), (400, false), "{e}");
        assert!(e.to_string().contains("fal") && e.to_string().contains("replicate"), "names both: {e}");
    }

    /// Handlers review #5: a route whose every deployment's breaker is open
    /// answers 503, retryable, after the shortest wait left (`Retry-After`,
    /// whole seconds rounded up): when the first deployment is back. It
    /// answered a 502 that said nothing of when to retry.
    #[test]
    fn a_route_whose_every_breaker_is_open_answers_503_after_the_shortest_wait() {
        let mut a = Attempts::default();
        a.skipped_open("fal", std::time::Duration::from_secs(30));
        a.skipped_open("replicate", std::time::Duration::from_millis(4200));
        let e = a.into_error("m");
        assert_eq!((e.status_code(), e.retryable(), e.reached_vendor()), (503, true, false), "{e}");
        assert_eq!(e.retry_after_secs(), Some(5), "{e}");
        assert!(e.to_string().contains("fal") && e.to_string().contains("replicate"), "{e}");
    }

    /// Handlers review #4: a refusal after a skipped deployment is no
    /// reason to give up: the skipped one may serve the request once its
    /// breaker closes. The answer is the retryable 502 that names the skip
    /// and the refusal. With nothing skipped, the refusal stands (E9).
    #[test]
    fn a_refusal_wins_only_when_nothing_was_skipped() {
        let refusal = || ProviderError::InvalidRequest("cannot make 4K".into());
        let mut skipped = Attempts::default();
        skipped.skipped_open("fal", std::time::Duration::from_secs(10));
        skipped.failed("replicate", refusal());
        let e = skipped.into_error("m");
        assert_eq!((e.status_code(), e.retryable(), e.reached_vendor()), (502, true, false), "{e}");
        assert!(e.to_string().contains("fal: circuit open") && e.to_string().contains("cannot make 4K"), "{e}");
        let mut none_skipped = Attempts::default();
        none_skipped.failed("replicate", refusal());
        let e = none_skipped.into_error("m");
        assert!(matches!(e, ProxyError::Refused { .. }), "{e:?}");
        assert_eq!((e.status_code(), e.retryable()), (400, false), "{e}");
    }

    /// Skips of both kinds (one breaker open, one provider missing) answer
    /// the retryable 502 that names each, as before.
    #[test]
    fn mixed_skips_answer_a_retryable_502_that_names_each() {
        let mut a = Attempts::default();
        a.skipped_open("fal", std::time::Duration::from_secs(10));
        a.skipped_unconfigured("replicate");
        let e = a.into_error("m");
        assert_eq!((e.status_code(), e.retryable(), e.reached_vendor()), (502, true, false), "{e}");
        assert!(e.to_string().contains("fal: circuit open") && e.to_string().contains("replicate: not configured"), "{e}");
    }

    /// A vendor attempt wins over any skip: it may have billed, so the
    /// caller's key must record it (E9).
    #[test]
    fn a_vendor_attempt_wins_over_skips() {
        let mut a = Attempts::default();
        a.skipped_open("fal", std::time::Duration::from_secs(10));
        a.failed("replicate", ProviderError::RequestFailed { message: "boom".into(), status_code: Some(500), provider_error: None, retryable: true });
        let e = a.into_error("m");
        assert!(e.reached_vendor(), "{e:?}");
        assert!(matches!(&e, ProxyError::VendorFailed { provider, .. } if provider == "replicate"), "{e:?}");
    }

    /// Handlers review #7: what a failed request's tenant-readable records
    /// (its log row, a segmentation's row, the error artifact) keep of an
    /// error: an internal error's fixed text, any other error's own. The
    /// segmentation row and `log_failed_generation` both read it here.
    #[test]
    fn a_record_keeps_an_internal_errors_fixed_text_and_any_other_errors_own() {
        let internal = ProxyError::Internal("preservation stopped: task 3 panicked at src/proxy/preserve.rs:40".into());
        assert_eq!(internal.logged_message(), "internal server error");
        let refused = ProxyError::Refused { model: "m".into(), provider: "p".into(), message: "cannot make 4K".into() };
        assert_eq!(refused.logged_message(), refused.to_string());
    }

    /// E9: an adapter's refusal before any call is the caller's to fix: a
    /// 400 that reached no vendor, so it spends no `Idempotency-Key`.
    #[test]
    fn a_refusal_before_any_call_is_not_a_vendor_attempt() {
        for err in [ProviderError::InvalidRequest("x".into()), ProviderError::ModelNotSupported { model: "m".into(), provider: "p".into() }] {
            let e = failure("m", "p", Some(err), None);
            assert!(!e.reached_vendor(), "{e}");
            assert_eq!(e.status_code(), 400, "{e}");
            assert!(!e.retryable(), "{e}");
        }
    }

    /// A job the vendor took can still bill: a refusal after it stays a
    /// vendor attempt, and names the job.
    #[test]
    fn a_refusal_after_the_vendor_took_the_job_still_names_it() {
        let e = failure("m", "p", Some(ProviderError::InvalidRequest("x".into())), Some("job-1".into()));
        assert!(e.reached_vendor(), "{e}");
        assert!(e.to_string().contains("job-1"), "{e}");
    }

    /// E9: an adapter that finds no usable credential at call time is the
    /// provider's `provider_not_configured`, naming the provider and keeping
    /// the adapter's advice. It used to be a 400 `invalid_request_error`
    /// that spent the key.
    #[test]
    fn a_missing_credential_is_provider_not_configured() {
        let advice = "kling: this credential is an access/secret key pair, which works only on Kling's legacy API; store a Kling API key instead";
        let e = failure("m", "kling", Some(ProviderError::NotConfigured(advice.into())), None);
        assert!(!e.reached_vendor(), "{e}");
        match e {
            ProxyError::ProviderNotConfigured { provider, detail } => assert_eq!((provider.as_str(), detail.as_deref()), ("kling", Some(advice))),
            other => panic!("not provider_not_configured: {other:?}"),
        }
    }
}

#[cfg(test)]
mod breaker_credential_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// E9: a 401 or 403 counts toward a deployment's breaker only when the
    /// call used the platform's own credential. A tenant's own revoked key
    /// must not cut the provider off for every tenant.
    #[tokio::test]
    async fn a_tenants_refused_key_does_not_open_the_breaker_for_everyone() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/fal-ai/flux/dev"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({ "detail": "Invalid key" })))
            .mount(&server)
            .await;
        let mut config = crate::config::AppConfig::default();
        config.providers.insert(
            "fal".into(),
            serde_json::from_value(serde_json::json!({ "api_key": "operator-key", "api_base": server.uri() })).unwrap(),
        );
        config.model_routes = vec![crate::config::ModelRouteConfig {
            model: "fal/flux-dev".into(),
            deployments: vec![crate::config::DeploymentConfig { provider: "fal".into(), weight: 1, max_retries: 0, timeout_seconds: 30, rpm_limit: 0 }],
            strategy: Some("fallback".into()),
            cache: None,
        }];
        config.circuit_breaker = crate::config::CircuitBreakerConfig { threshold: 1, open_for_seconds: 60 };
        let registry = Arc::new(crate::proxy::registry::ProviderRegistry::new());
        registry.init_from_config(&config).await;
        let router = ProxyRouter::new(
            registry,
            Arc::new(crate::proxy::cache::GenerationCache::new(&crate::config::CacheGlobalConfig::default())),
            Arc::new(config),
            Arc::new(crate::proxy::storage::LocalStore),
        );
        let mut models = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        models.pop();
        models.push("models");
        let schema = crate::capabilities::CapabilityRegistry::from_dir(&models).unwrap().get("fal/flux-dev").unwrap().clone();
        let base = crate::types::BaseGenerationRequest {
            prompt: "a lighthouse".into(),
            model: "fal/flux-dev".into(),
            n: 1,
            negative_prompt: None,
            seed: None,
            reference_images: vec![],
            strict: true,
            extra: None,
            metadata: None,
        };
        let extras = ImageExtras {
            vendor_job: Default::default(),
            size: None,
            resolution: None,
            aspect_ratio: None,
            quality: None,
            style: None,
            steps: None,
            guidance_scale: None,
            strength: None,
            response_format: "url".to_string(),
            edit: None,
            views: Vec::new(),
            extra: None,
            output_format: None,
            background: None,
            expand: None,
            layout: None,
            mask_prompt: None,
            preserve_unmasked: None,
            feather_px: None,
            palette: None,
            directions: None,
        };
        let materialized = MaterializedRequest { refs: vec![], cleanup: crate::proxy::materializer::Cleanup::empty() };
        let tenants_key = ProviderCredentials { api_key: Some("a-tenants-revoked-key".into()), ..Default::default() };

        let refused = router.generate_image(&schema, &base, &extras, &materialized, OrgCredentials::of("fal", Some(tenants_key)), None, None).await;
        assert!(refused.is_err(), "the vendor refused the tenant's key");
        assert!(!router.circuit_breaker.is_open("fal").await, "one tenant's revoked key opened the breaker for every tenant");

        let refused = router.generate_image(&schema, &base, &extras, &materialized, OrgCredentials::default(), None, None).await;
        assert!(refused.is_err(), "the vendor refused the platform's key");
        assert!(router.circuit_breaker.is_open("fal").await, "a refused platform key counts toward the breaker");
    }

    /// A router whose route for `model` tries `deployments` in order, once
    /// each, over the providers `providers` (name, config) registered as the
    /// platform's, with a breaker that opens after `threshold` failures.
    async fn routed(model: &str, deployments: &[&str], providers: &[(&str, serde_json::Value)], threshold: u32) -> ProxyRouter {
        routed_with("fallback", model, deployments, providers, threshold).await
    }

    /// [`routed`], with the route's `strategy` (`fallback`,
    /// `weighted_round_robin`, `lowest_cost` or `lowest_latency`).
    async fn routed_with(strategy: &str, model: &str, deployments: &[&str], providers: &[(&str, serde_json::Value)], threshold: u32) -> ProxyRouter {
        let mut config = crate::config::AppConfig::default();
        for (name, cfg) in providers {
            config.providers.insert(name.to_string(), serde_json::from_value(cfg.clone()).unwrap());
        }
        config.model_routes = vec![crate::config::ModelRouteConfig {
            model: model.into(),
            deployments: deployments
                .iter()
                .map(|p| crate::config::DeploymentConfig { provider: p.to_string(), weight: 1, max_retries: 0, timeout_seconds: 30, rpm_limit: 0 })
                .collect(),
            strategy: Some(strategy.into()),
            cache: None,
        }];
        config.circuit_breaker = crate::config::CircuitBreakerConfig { threshold, open_for_seconds: 60 };
        let registry = Arc::new(crate::proxy::registry::ProviderRegistry::new());
        registry.init_from_config(&config).await;
        ProxyRouter::new(
            registry,
            Arc::new(crate::proxy::cache::GenerationCache::new(&crate::config::CacheGlobalConfig::default())),
            Arc::new(config),
            Arc::new(crate::proxy::storage::LocalStore),
        )
    }

    /// One `fal/flux-dev` image through `router` on `creds`.
    async fn flux_dev(router: &ProxyRouter, creds: OrgCredentials) -> Result<ImageGenerationResponse, ProxyError> {
        let mut models = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        models.pop();
        models.push("models");
        let schema = crate::capabilities::CapabilityRegistry::from_dir(&models).unwrap().get("fal/flux-dev").unwrap().clone();
        let base: BaseGenerationRequest = serde_json::from_value(serde_json::json!({ "prompt": "a lighthouse", "model": "fal/flux-dev" })).unwrap();
        let extras = ImageExtras {
            vendor_job: Default::default(),
            size: None, resolution: None, aspect_ratio: None, quality: None, style: None, steps: None,
            guidance_scale: None, strength: None, response_format: "b64_json".to_string(),
            edit: None, views: Vec::new(), extra: None, output_format: None, background: None,
            expand: None, layout: None, mask_prompt: None, preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
        };
        let materialized = MaterializedRequest { refs: vec![], cleanup: crate::proxy::materializer::Cleanup::empty() };
        router.generate_image(&schema, &base, &extras, &materialized, creds, None, None).await
    }

    /// The `Authorization` each request to `server` carried, in order.
    async fn authorizations(server: &MockServer) -> Vec<String> {
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .map(|r| r.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string())
            .collect()
    }

    fn key(k: &str) -> ProviderCredentials {
        ProviderCredentials { api_key: Some(k.into()), ..Default::default() }
    }

    /// BYO-ROUTE (code review 2026-11-11, security): a route's deployment
    /// gets the org's credential for its own provider only. The credential
    /// resolved for the model's provider (fal here) went to every
    /// deployment, so a route that sends a fal model to OpenAI sent the
    /// org's fal key to OpenAI.
    #[tokio::test]
    async fn a_routes_deployment_gets_only_its_own_providers_org_key() {
        let openai = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "created": 1, "data": [{ "b64_json": "aW1n" }] })))
            .mount(&openai)
            .await;
        let platform = serde_json::json!({ "api_key": "platform-openai-key", "api_base": format!("{}/v1", openai.uri()) });
        let router = routed("fal/flux-dev", &["openai"], &[("openai", platform)], 5).await;
        // The org stored a fal key only: the OpenAI deployment runs on the
        // platform's OpenAI key, as a direct OpenAI request would.
        let r = flux_dev(&router, OrgCredentials::of("fal", Some(key("org-fal-key")))).await;
        assert!(r.is_ok(), "{:?}", r.err());
        // With an OpenAI key of its own stored too, on that one.
        let mut both = OrgCredentials::of("fal", Some(key("org-fal-key")));
        both.insert("openai", key("org-openai-key"));
        let r = flux_dev(&router, both).await;
        assert!(r.is_ok(), "{:?}", r.err());
        let sent = authorizations(&openai).await;
        assert!(!sent.iter().any(|a| a.contains("org-fal-key")), "the org's fal key reached the route's OpenAI deployment: {sent:?}");
        assert_eq!(sent, ["Bearer platform-openai-key", "Bearer org-openai-key"]);
    }

    /// Registry review #8: BYO-ROUTE holds whatever the route's strategy.
    /// Each strategy orders the deployments its own way (`lowest_cost` quotes
    /// each through `app_creds.get(&d.provider)`) and then dispatches; every
    /// one hands a deployment only the org's key for its own provider. The
    /// fallback strategy is the test above.
    #[tokio::test]
    async fn every_route_strategy_gives_an_image_deployment_only_its_own_providers_org_key() {
        for strategy in ["weighted_round_robin", "lowest_cost", "lowest_latency"] {
            let openai = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/images/generations"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "created": 1, "data": [{ "b64_json": "aW1n" }] })))
                .mount(&openai)
                .await;
            let platform = serde_json::json!({ "api_key": "platform-openai-key", "api_base": format!("{}/v1", openai.uri()) });
            let router = routed_with(strategy, "fal/flux-dev", &["openai"], &[("openai", platform)], 5).await;
            let r = flux_dev(&router, OrgCredentials::of("fal", Some(key("org-fal-key")))).await;
            assert!(r.is_ok(), "{strategy}: {:?}", r.err());
            let mut both = OrgCredentials::of("fal", Some(key("org-fal-key")));
            both.insert("openai", key("org-openai-key"));
            let r = flux_dev(&router, both).await;
            assert!(r.is_ok(), "{strategy}: {:?}", r.err());
            assert_eq!(authorizations(&openai).await, ["Bearer platform-openai-key", "Bearer org-openai-key"], "{strategy}");
        }
    }

    /// One `luma/ray-2` video through `router` on `creds`.
    async fn ray_2(router: &ProxyRouter, creds: OrgCredentials) -> Result<VideoGenerationResponse, ProxyError> {
        let mut models = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        models.pop();
        models.push("models");
        let schema = crate::capabilities::CapabilityRegistry::from_dir(&models).unwrap().get("luma/ray-2").unwrap().clone();
        let base: BaseGenerationRequest = serde_json::from_value(serde_json::json!({ "prompt": "a lighthouse", "model": "luma/ray-2" })).unwrap();
        let extras = crate::providers::VideoExtras {
            duration_seconds: 5.0,
            aspect_ratio: None,
            resolution: None,
            fps: None,
            extra: None,
            camera: None, audio: None, r#loop: None,
            source: None, direction: None, time_range: None, replace: None,
        };
        let materialized = MaterializedRequest { refs: vec![], cleanup: crate::proxy::materializer::Cleanup::empty() };
        router.generate_video(&schema, &base, &extras, &materialized, creds).await
    }

    /// Registry review #8: the video twin of
    /// `a_routes_deployment_gets_only_its_own_providers_org_key`, for every
    /// strategy (`fallback_video`, and the `lowest_cost` quotes). A route
    /// sends `luma/ray-2` to an OpenAI (Sora) deployment: the org's Luma key
    /// never reaches OpenAI; the deployment runs on the org's OpenAI key when
    /// it stored one, else on the platform's, as a direct request would.
    #[tokio::test]
    async fn a_video_routes_deployment_gets_only_its_own_providers_org_key() {
        for strategy in ["fallback", "weighted_round_robin", "lowest_cost", "lowest_latency"] {
            let openai = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/videos"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "id": "video_1", "status": "queued", "model": "sora-2" })))
                .mount(&openai)
                .await;
            let platform = serde_json::json!({ "api_key": "platform-openai-key", "api_base": format!("{}/v1", openai.uri()) });
            let router = routed_with(strategy, "luma/ray-2", &["openai"], &[("openai", platform)], 5).await;
            let r = ray_2(&router, OrgCredentials::of("luma", Some(key("org-luma-key")))).await;
            assert!(r.is_ok(), "{strategy}: {:?}", r.err());
            let mut both = OrgCredentials::of("luma", Some(key("org-luma-key")));
            both.insert("openai", key("org-openai-key"));
            let r = ray_2(&router, both).await;
            assert!(r.is_ok(), "{strategy}: {:?}", r.err());
            let sent = authorizations(&openai).await;
            assert!(!sent.iter().any(|a| a.contains("org-luma-key")), "{strategy}: the org's Luma key reached the route's OpenAI deployment: {sent:?}");
            assert_eq!(sent, ["Bearer platform-openai-key", "Bearer org-openai-key"], "{strategy}");
        }
    }

    /// BYO-ROUTE with E9: whether a deployment's 401 counts toward its
    /// breaker is judged by the credential that deployment used. A tenant's
    /// revoked OpenAI key on a route's OpenAI deployment must not open that
    /// breaker for every tenant, though the model's own provider (fal) would
    /// run on the platform's key.
    #[tokio::test]
    async fn a_tenants_revoked_key_on_a_routes_deployment_does_not_open_its_breaker() {
        let openai = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/images/generations"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({ "error": { "message": "Incorrect API key provided" } })))
            .mount(&openai)
            .await;
        let platform = serde_json::json!({ "api_key": "platform-openai-key", "api_base": format!("{}/v1", openai.uri()) });
        let router = routed("fal/flux-dev", &["openai"], &[("openai", platform)], 1).await;
        let r = flux_dev(&router, OrgCredentials::of("openai", Some(key("org-revoked-openai-key")))).await;
        assert!(r.is_err(), "the vendor refused the tenant's key");
        assert!(!router.circuit_breaker.is_open("openai").await, "one tenant's revoked key on a route's deployment opened its breaker for every tenant");
        assert_eq!(authorizations(&openai).await, ["Bearer org-revoked-openai-key"], "the deployment ran on the org's own OpenAI key");
        // The platform's own key refused still counts, as for a direct request.
        let r = flux_dev(&router, OrgCredentials::default()).await;
        assert!(r.is_err(), "the vendor refused the platform's key");
        assert!(router.circuit_breaker.is_open("openai").await, "a refused platform key counts toward the breaker");
    }

    /// Ruled 2026-11-12 (handlers review should-fix 1): a vendor's 429 counts
    /// toward the breaker only when the call used the platform's credential,
    /// as a 401 or 403 does since E9. A tenant spending its own key's rate
    /// limit says nothing about the provider for everyone else.
    #[tokio::test]
    async fn a_tenants_429s_leave_the_breaker_closed_and_the_platforms_open_it() {
        let fal = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/fal-ai/flux/dev"))
            .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({ "detail": "Rate limit exceeded" })))
            .mount(&fal)
            .await;
        let platform = serde_json::json!({ "api_key": "operator-key", "api_base": fal.uri() });
        let router = routed("fal/flux-dev", &["fal"], &[("fal", platform)], 2).await;
        for i in 0..2 {
            let r = flux_dev(&router, OrgCredentials::of("fal", Some(key("a-tenants-key")))).await;
            assert!(r.is_err(), "call {i}: the vendor rate-limited the tenant's key");
        }
        assert!(!router.circuit_breaker.is_open("fal").await, "two 429s to one tenant's own key opened the breaker for every tenant");
        for i in 0..2 {
            let r = flux_dev(&router, OrgCredentials::default()).await;
            assert!(r.is_err(), "call {i}: the vendor rate-limited the platform's key");
        }
        assert!(router.circuit_breaker.is_open("fal").await, "the platform's key rate-limited counts toward the breaker");
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    use crate::management::settings::RetrySettings;
    use std::time::Duration;

    fn settings() -> RetrySettings {
        RetrySettings { max_retries: 5, backoff: Duration::from_millis(500), max_backoff: Duration::from_secs(3) }
    }

    fn refused() -> ProviderError {
        ProviderError::RequestFailed { message: "busy".into(), status_code: Some(503), provider_error: None, retryable: true }
    }

    fn limited(ms: u64) -> ProviderError {
        ProviderError::RateLimited { provider: "acme".into(), retry_after_ms: ms, message: "slow down".into(), provider_error: None }
    }

    #[test]
    fn the_backoff_grows_with_full_jitter_up_to_the_ceiling() {
        let s = settings();
        // At full jitter the wait is the cap itself: the first backoff, doubled each retry, capped.
        let caps: Vec<_> = (1..=5).map(|n| retry_delay(n, &refused(), &s, 1.0)).collect();
        let ms = |v: u64| Some(Duration::from_millis(v));
        assert_eq!(caps, vec![ms(500), ms(1000), ms(2000), ms(3000), ms(3000)]);
        // Jitter spreads each wait over 0 to its cap.
        assert_eq!(retry_delay(2, &refused(), &s, 0.0), ms(0));
        assert_eq!(retry_delay(2, &refused(), &s, 0.5), ms(500));
        assert_eq!(retry_delay(40, &refused(), &s, 1.0), ms(3000), "no overflow far out");
    }

    #[test]
    fn retry_after_is_a_floor_and_past_the_ceiling_ends_the_retries() {
        let s = settings();
        let ms = |v: u64| Some(Duration::from_millis(v));
        assert_eq!(retry_delay(1, &limited(1200), &s, 0.0), ms(1200), "the vendor's wait beats a short jitter");
        assert_eq!(retry_delay(3, &limited(100), &s, 1.0), ms(2000), "the backoff beats a short Retry-After");
        assert_eq!(retry_delay(1, &limited(3000), &s, 0.0), ms(3000), "exactly the ceiling is waited for");
        assert_eq!(retry_delay(1, &limited(3001), &s, 0.0), None, "longer than the ceiling: not retried");
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
            source_url: None,
        }
    }

    #[test]
    fn keeping_nothing_lists_the_vendors_3d_files_and_refuses_one_without_a_link() {
        let mut mesh = mesh_file();
        mesh.source_url = Some("https://cdn.vendor.example/a/model.GLB?sig=1".into());
        let mut preview = preview_file();
        preview.source_url = Some("https://cdn.vendor.example/a/preview.png".into());
        let assets = keep_nothing_model3d_assets(&[mesh.clone(), preview]).expect("both have links");
        assert_eq!(assets.len(), 2);
        assert_eq!(assets[0].url, "https://cdn.vendor.example/a/model.GLB?sig=1");
        assert_eq!(assets[0].format, "glb", "the format is normalised, the link is not");
        assert!(assets.iter().all(|a| !a.kept && a.size_bytes.is_none() && a.sha256.is_none() && a.scene.is_none()));
        mesh.source_url = None;
        assert!(keep_nothing_model3d_assets(&[mesh.clone()]).unwrap_err().contains("keeps nothing"));
        mesh.source_url = Some("javascript:alert(1)".into());
        assert!(keep_nothing_model3d_assets(&[mesh]).is_err(), "only http(s) links are handed out");
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
            source_url: None,
        }
    }

    /// A router whose job `id` is the finished Tripo rig of
    /// `TripoModel3dProvider::finished_rig_for_tests`, with its row in `db`.
    async fn tripo_job(
        db: &Arc<dyn crate::db::DatabaseStore>,
        tripo: crate::providers::model3d::tripo::TripoModel3dProvider,
        id: &str,
    ) -> (Arc<crate::proxy::registry::ProviderRegistry>, ProxyRouter) {
        let registry = Arc::new(crate::proxy::registry::ProviderRegistry::new());
        let provider: Arc<dyn crate::providers::Model3dProvider> = Arc::new(tripo);
        registry.register_mock_model3d(provider.clone()).await;
        let router = ProxyRouter::new(
            registry.clone(),
            Arc::new(crate::proxy::cache::GenerationCache::new(&crate::config::CacheGlobalConfig::default())),
            Arc::new(crate::config::AppConfig::default()),
            Arc::new(crate::proxy::storage::LocalStore),
        );
        router.model3d_jobs.write().await.insert(id.to_string(), Model3dJob {
            handle: Model3dGenerationHandle {
                provider_job_id: "rig-1".into(),
                provider: "mock".into(),
                model: "tripo/rig".into(),
                stage_context: Some(serde_json::json!({ "task_id": "rig-1" })),
            },
            usage: None,
            delivery: crate::proxy::model3d_deliver::DeliveryPlan::default(),
            provider,
            created: chrono::Utc::now().timestamp(),
        });
        db.insert_generation(id, None, "tripo/rig", "mock", "model3d", Some("rig-1"), 0.0, None, None).await.unwrap();
        (registry, router)
    }

    fn local_store() -> Arc<dyn crate::proxy::storage::ImageStorage> {
        Arc::new(LocalModel3dStorage::new(tempfile::tempdir().unwrap().keep(), "https://assets.test".into()))
    }

    /// Review 2026-11-11 P5: a client's poll that does not hold the delivery
    /// downloads nothing.
    #[tokio::test]
    async fn a_client_poll_downloads_nothing_while_another_observer_delivers() {
        use crate::providers::model3d::tripo::TripoModel3dProvider;
        let db: Arc<dyn crate::db::DatabaseStore> = Arc::new(crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let (server, tripo) = TripoModel3dProvider::finished_rig_for_tests().await;
        let (_registry, router) = tripo_job(&db, tripo, "g-tripo").await;
        assert!(db.lease_generation_work("g-tripo", "delivery", "elsewhere", chrono::Duration::minutes(5)).await.unwrap());

        let resp = router.get_model3d_status("g-tripo", Some(local_store()), None, &db).await.unwrap();
        assert_eq!(resp.status, GenerationStatus::Processing, "{resp:?}");
        let n = TripoModel3dProvider::rig_downloads_for_tests(&server).await;
        assert_eq!(n, 0, "the mesh was downloaded {n} time(s) while another observer held the delivery");
    }

    /// Review 2026-11-11 P5: the client's poll and the poller both see the job
    /// finish; only the one that delivers it downloads it.
    #[tokio::test]
    async fn a_finished_job_is_downloaded_once_by_the_observer_that_delivers_it() {
        use crate::providers::model3d::tripo::TripoModel3dProvider;
        let db: Arc<dyn crate::db::DatabaseStore> = Arc::new(crate::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let (server, tripo) = TripoModel3dProvider::finished_rig_for_tests().await;
        let (registry, router) = tripo_job(&db, tripo, "g-tripo").await;
        let store = local_store();

        let (client, _) = tokio::join!(
            router.get_model3d_status_claimed("g-tripo", Some(store.clone()), None, &db),
            crate::proxy::poller::poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store),
        );
        let (client, claim) = client.expect("the client's poll answers");
        if client.status == GenerationStatus::Completed {
            // The client won: its handler writes the outcome while it holds
            // the claim, as here.
            let url = client.mesh().map(|m| m.url.clone());
            db.update_generation_if_active("g-tripo", "completed", 100, url.as_deref(), None, Some(chrono::Utc::now()), None)
                .await
                .unwrap();
        }
        drop(claim);
        let n = TripoModel3dProvider::rig_downloads_for_tests(&server).await;
        assert_eq!(n, 1, "the finished job's mesh was downloaded {n} times; only the observer that delivers it should");
        let row = db.get_generation("g-tripo").await.unwrap().unwrap();
        assert_eq!(row.status, GenerationStatus::Completed, "{:?}", row.error_message);
    }

    #[tokio::test]
    async fn rehost_writes_each_file_under_a_predictable_key_and_returns_absolute_urls() {
        let store: Arc<dyn crate::proxy::storage::ImageStorage> =
            Arc::new(LocalModel3dStorage::new(tempfile::tempdir().unwrap().keep(), "https://cdn.example.com".into()));

        let assets = rehost_model3d_files(&store, None, "litegen-3d-42", vec![mesh_file(), preview_file()])
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
        let assets = rehost_model3d_files(&store, None, "litegen-3d-7", vec![mesh_file(), a, b]).await.unwrap();
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
        let assets = rehost_model3d_files(&store, Some("tenant-9/meshes"), "g1", vec![mesh_file()]).await.unwrap();
        assert!(assets[0].url.contains("/tenant-9/meshes/g1/model.glb"), "got {}", assets[0].url);
    }

    /// A package is stored as `package_<container>.zip`, beside the mesh of
    /// the same container, and lists the container as its format.
    #[tokio::test]
    async fn rehost_stores_a_package_as_a_zip_named_for_its_container() {
        let store: Arc<dyn crate::proxy::storage::ImageStorage> =
            Arc::new(LocalModel3dStorage::new(tempfile::tempdir().unwrap().keep(), "https://cdn.example.com".into()));
        let mut obj = mesh_file();
        obj.format = "obj".into();
        let package = |format: &str| Model3dFile {
            kind: Model3dAssetKind::Package,
            format: format.into(),
            content_type: "application/zip".into(),
            bytes: b"PK\x03\x04 a zip".to_vec(),
            ..mesh_file()
        };
        let assets = rehost_model3d_files(&store, None, "g3", vec![obj, package("obj"), package("dae")]).await.unwrap();
        let urls: Vec<&str> = assets.iter().map(|a| a.url.as_str()).collect();
        assert!(urls[0].ends_with("/g3/model.obj"), "{urls:?}");
        assert!(urls[1].ends_with("/g3/package_obj.zip"), "{urls:?}");
        assert!(urls[2].ends_with("/g3/package_dae.zip"), "{urls:?}");
        assert_eq!((assets[1].kind, assets[1].format.as_str()), (Model3dAssetKind::Package, "obj"));
        assert_eq!(assets[1].sha256.as_deref(), Some(sha256_hex(b"PK\x03\x04 a zip").as_str()));
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
            skeleton: None, joint_naming: None, height_m: None, preflight: None, fbx_color_space: None, fbx_light_units: None, usd_light_units: None, dae_morphs: None, usdz_root: None, usd_clips: None, format_source: None,output_packages: None, ktx2_textures: None,
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
            prompt: crate::capabilities::PromptSpec { required: true, min_length: None, max_length: None, required_for_text_to_video: false },
            params: Default::default(), ref_inputs: None, extra_allowlist: vec![], extra_params: Default::default(), litegen_params: Default::default(), tags: vec![],
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
            skeleton: None, joint_naming: None, height_m: None, preflight: None, fbx_color_space: None, fbx_light_units: None, usd_light_units: None, dae_morphs: None, usdz_root: None, usd_clips: None, format_source: None,output_packages: None, ktx2_textures: None,
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

    /// A vendor whose free preflight is down.
    struct PreflightOutage;

    #[async_trait::async_trait]
    impl crate::providers::Model3dProvider for PreflightOutage {
        fn name(&self) -> &str { "mock" }
        fn configure(&mut self, _c: crate::providers::ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool { true }
        async fn preflight(
            &self,
            _m: &crate::capabilities::ModelSchema,
            _e: &crate::providers::Model3dExtras,
        ) -> Result<crate::providers::Preflight, crate::providers::ProviderError> {
            Err(crate::providers::ProviderError::RequestFailed {
                message: "rig-check answered HTTP 503".into(), status_code: Some(503), provider_error: None, retryable: true,
            })
        }
        async fn generate(
            &self,
            _m: &crate::capabilities::ModelSchema,
            _b: &crate::types::BaseGenerationRequest,
            _e: &crate::providers::Model3dExtras,
            _r: &crate::proxy::materializer::MaterializedRequest,
        ) -> Result<crate::providers::Model3dGenerationHandle, crate::providers::ProviderError> {
            unreachable!("a failed preflight submits nothing")
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

    /// The preflight bills nothing (3D doc §3B) and a request no deployment
    /// was asked to run reached no vendor, so neither failure spends an
    /// `Idempotency-Key`: the client's retry runs again instead of replaying
    /// the failure for 24 hours.
    #[tokio::test]
    async fn a_failed_preflight_or_an_unattempted_request_never_reached_a_billed_call() {
        assert!(!ProxyError::AllDeploymentsFailed { model: "m".into(), last_error: None }.reached_vendor());
        let registry = Arc::new(crate::proxy::registry::ProviderRegistry::new());
        registry.register_mock_model3d(Arc::new(PreflightOutage)).await;
        let router = ProxyRouter::new(
            registry,
            Arc::new(crate::proxy::cache::GenerationCache::new(&crate::config::CacheGlobalConfig::default())),
            Arc::new(crate::config::AppConfig::default()),
            Arc::new(crate::proxy::storage::LocalStore),
        );
        let schema = crate::capabilities::ModelSchema {
            id: "mock/rig-3d".into(), provider: "mock".into(), media_type: crate::capabilities::MediaType::Model3d,
            task: Default::default(), native_orientation: None, preserves_unmasked: None,
            display_name: "rig".into(), description: String::new(), docs_url: None,
            pricing: crate::capabilities::ModelPricing { base_cost_usd: 0.0, variable_pricing: Vec::new() },
            capabilities: Default::default(),
            prompt: crate::capabilities::PromptSpec { required: false, min_length: None, max_length: None, required_for_text_to_video: false },
            params: Default::default(), ref_inputs: None, extra_allowlist: vec![], extra_params: Default::default(), litegen_params: Default::default(), tags: vec![],
            deprecated_at: None, retires_at: None, replaced_by: None, status: Default::default(),
        };
        let extras = crate::providers::Model3dExtras {
            output_formats: vec!["glb".into()],
            texture: None, pbr: None, target_polycount: None,
            symmetry: None, topology: None, rig: None, extra: None,
            source: None,
            animations: Vec::new(), fps: None,
            skeleton: None, joint_naming: None, height_m: None, preflight: None, fbx_color_space: None, fbx_light_units: None, usd_light_units: None, dae_morphs: None, usdz_root: None, usd_clips: None, format_source: None,output_packages: None, ktx2_textures: None,
            texture_resolution: None, keep_uvs: None,
            ground: None, center: None,
        };
        let err = router.preflight_model3d(&schema, &extras, None).await.unwrap_err();
        assert!(!err.reached_vendor(), "{err:?}");
        assert_eq!(err.status_code(), 502, "a retryable outage: {err:?}");
    }
}
