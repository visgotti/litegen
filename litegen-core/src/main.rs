use std::collections::HashMap;
use std::sync::Arc;
use base64::Engine as _;
use tower_http::trace::TraceLayer;
use tracing::info;

use litegen::api;
use litegen::api::middleware::AppState;
use litegen::config::{self, Mode};
use litegen::observability::init_tracing;
use litegen::proxy::{GenerationCache, ProxyRouter, ProviderRegistry, build_image_store, spawn_poller};
use litegen::proxy::materializer::{Materializer, StorageAdapter};

/// Decode a base64-encoded 32-byte secrets key. Returns `Some([u8;32])` only
/// when the input decodes to exactly 32 bytes; otherwise `None`.
fn decode_secrets_key(s: &Option<String>) -> Option<[u8; 32]> {
    let raw = s.as_ref()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .ok()?;
    bytes.try_into().ok()
}

/// The signals that ask litegen to stop gracefully: ctrl-c (SIGINT) everywhere,
/// and on unix SIGTERM too, which is what `docker stop` and Kubernetes send.
/// Without SIGTERM a container would be killed after its grace period with no
/// drain, no poller stop and no release of its poll claims.
///
/// [`StopSignals::listen`] installs the handlers at once, so a signal that
/// arrives before [`StopSignals::wait`] is polled is not lost.
struct StopSignals {
    #[cfg(unix)]
    terminate: Option<tokio::signal::unix::Signal>,
}

impl StopSignals {
    /// Start listening. If the SIGTERM handler can't be installed, say so and
    /// go on with ctrl-c alone.
    fn listen() -> Self {
        #[cfg(unix)]
        {
            let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|e| tracing::warn!(error = %e, "could not listen for SIGTERM; only ctrl-c (SIGINT) will stop litegen gracefully"))
                .ok();
            Self { terminate }
        }
        #[cfg(not(unix))]
        {
            Self {}
        }
    }

    /// Wait for the first of the signals, and name it for the log.
    async fn wait(self) -> &'static str {
        #[cfg(unix)]
        if let Some(mut terminate) = self.terminate {
            return tokio::select! {
                _ = tokio::signal::ctrl_c() => "ctrl-c (SIGINT)",
                _ = terminate.recv() => "SIGTERM",
            };
        }
        let _ = tokio::signal::ctrl_c().await;
        "ctrl-c (SIGINT)"
    }
}

/// litegen's entry point. `.env` and the configuration are read here, before
/// the async runtime starts its worker threads: `dotenvy` sets variables in
/// the process environment, which is only sound while litegen is one thread.
fn main() -> anyhow::Result<()> {
    // The command line: `litegen [--config <file>]`. Anything else is refused,
    // never ignored.
    let args = config::parse_args(std::env::args_os().skip(1)).unwrap_or_else(|e| {
        eprintln!("litegen: {e}\n\n{}", config::USAGE);
        std::process::exit(2)
    });
    if args.help {
        print!("{}", config::USAGE);
        return Ok(());
    }

    // Load .env file if present
    let _ = dotenvy::dotenv();
    let env: HashMap<String, String> = std::env::vars().collect();
    let cwd = std::env::current_dir()?;

    // The config file (docs/configuration.md), read once. One that is there
    // but can't be read is a refusal, never a silent start on the defaults.
    let file = config::config_file(&cwd, args.config.as_deref(), &env).unwrap_or_else(|e| {
        eprintln!("litegen's configuration did not load: {e}");
        std::process::exit(1)
    });
    // A config that doesn't load is a refusal too: the defaults would drop
    // everything it set, from the master key to the database.
    let config = config::load_config_with(&cwd, file.as_ref(), &env).unwrap_or_else(|e| refuse_to_start(file.as_ref(), &env, e));

    // What `#[tokio::main]` built, now that the environment is settled.
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(serve(config))
}

/// Exit on a config that did not load, naming what the file declares that a
/// start on the defaults would drop: hooks (an auth hook would go missing) or
/// `storage_access` (the default, `public`, would open files the operator set
/// out to close).
fn refuse_to_start(file: Option<&config::ConfigFile>, env: &HashMap<String, String>, e: ::config::ConfigError) -> ! {
    // With no file, only a LITEGEN__STORAGE_ACCESS__* variable can set it.
    let name = file.map_or("litegen's environment", |f| f.name.as_str());
    match file.map_or(Ok(Vec::new()), litegen::hooks::hooks_in_config_file) {
        Ok(hooks) if hooks.is_empty() => {
            if config::storage_access_in(file, env.iter().map(|(k, v)| (k.clone(), v.clone()))) {
                eprintln!("{name} sets storage_access but did not load: {e}");
            } else {
                eprintln!("litegen's configuration did not load: {e}");
            }
        }
        Ok(_) => eprintln!("{name} defines hooks but did not load: {e}"),
        Err(hooks_err) => eprintln!("{name} defines hooks but did not load: {e} (hooks: {hooks_err})"),
    }
    std::process::exit(1)
}

/// Run litegen with `config` until a stop signal has drained it.
async fn serve(config: config::AppConfig) -> anyhow::Result<()> {
    let config = Arc::new(config);

    // Initialize Prometheus metrics recorder (no built-in HTTP listener;
    // served via the axum router at `observability.metrics_path`). With
    // metrics off, nothing is recorded and no route is mounted.
    if config.observability.metrics_enabled {
        litegen::api::init_prometheus();
    }

    // Install W3C TraceContext propagator so outbound HTTP calls carry
    // `traceparent` / `tracestate` headers for distributed tracing.
    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );

    // Initialize tracing (and optionally OTel OTLP export if
    // OTEL_EXPORTER_OTLP_ENDPOINT is set).
    init_tracing(&config.observability.log_level, config.observability.json_logs);

    // Settings that lost to a higher-precedence one.
    for warning in &config.startup_warnings {
        tracing::warn!("{warning}");
    }
    // Which config file was read, so that choice is never silent.
    match &config.config_file {
        Some(path) => info!("config file: {}", path.display()),
        None => info!("no config file: none was named, and the working directory holds no litegen.yaml"),
    }

    info!(
        host = %config.server.host,
        port = config.server.port,
        "Starting LiteGen proxy"
    );

    // Pre-bind security self-check. In single-tenant mode with no master key,
    // auth_middleware grants anonymous requests full admin scope; bound to a
    // non-loopback address that exposes the whole admin API to the network.
    // Refuse to start in that case unless the operator explicitly opts in.
    if let config::StartupSecurityCheck::UnauthenticatedExposure { host } =
        config.startup_security_check()
    {
        let allow_no_auth = std::env::var("LITEGEN__ALLOW_NO_AUTH")
            .map(|v| v.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if allow_no_auth {
            tracing::warn!(
                "SECURITY: starting with NO authentication — single-tenant mode has no \
                 LITEGEN__MASTER_KEY set and is bound to non-loopback {host}. The full admin \
                 API is reachable by anyone who can reach this port (allowed via \
                 LITEGEN__ALLOW_NO_AUTH=true)."
            );
        } else {
            eprintln!(
                "FATAL: refusing to start. Single-tenant mode has no LITEGEN__MASTER_KEY set \
                 and is bound to non-loopback address {host}, which would expose the full admin \
                 API UNAUTHENTICATED to the network. Set LITEGEN__MASTER_KEY to a strong secret \
                 (recommended), or bind to loopback (LITEGEN__SERVER__HOST=127.0.0.1) behind a \
                 trusted reverse proxy. To intentionally run with no auth, set \
                 LITEGEN__ALLOW_NO_AUTH=true."
            );
            std::process::exit(1);
        }
    }

    // A provider header litegen can't send is a config error, not a surprise
    // at the first vendor call.
    let provider_errors = config.provider_config_errors();
    if !provider_errors.is_empty() {
        for e in &provider_errors {
            eprintln!("FATAL: {e}");
        }
        std::process::exit(1);
    }

    // Connect to database
    let db: Arc<dyn litegen::db::DatabaseStore> =
        Arc::from(litegen::db::connect(&config.database_url).await?);

    // Redis, when one is set: the state replicas share (docs/scaling.md). Only
    // settings that cannot work stop litegen. A Redis that is down does not:
    // every call falls back, and the connection is retried.
    let shared = match &config.redis {
        Some(settings) => match litegen::shared::SharedRedis::connect(settings, "litegen:").await {
            Ok(shared) => {
                let target = settings.target();
                match shared.ping().await {
                    Ok(()) => info!("Redis at {target}: reachable"),
                    Err(e) => tracing::warn!("Redis at {target}: unreachable ({e}); litegen keeps running and retries"),
                }
                Some(shared)
            }
            Err(e) => {
                eprintln!("FATAL: the Redis settings cannot be used: {e}");
                std::process::exit(1);
            }
        },
        None => None,
    };
    for note in litegen::shared::startup_notes(&config) {
        if note.warn {
            tracing::warn!("{}", note.text);
        } else {
            info!("{}", note.text);
        }
    }

    // Initialize provider registry
    let registry = Arc::new(ProviderRegistry::new());
    registry.init_from_config(&config).await;

    let image_providers = registry.image_provider_names().await;
    let video_providers = registry.video_provider_names().await;
    info!(
        image_providers = ?image_providers,
        video_providers = ?video_providers,
        "Providers initialized"
    );

    // Initialize cache
    let cache = Arc::new(GenerationCache::new(&config.cache));
    if config.cache.enabled {
        info!("Generation cache enabled");
    }

    // Initialize image storage backend
    let image_store = build_image_store(&config.image_storage);

    // Build proxy router
    // The signing key: LITEGEN__SECRETS_KEY when usable, else the one stored in
    // the database, so every replica signs and verifies links alike.
    let link_signer = litegen::access::signing::link_signer_for(config.secrets_key.as_deref(), &*db).await;
    if !link_signer.persistent() {
        // A key of this process's own: no other replica, and no restart,
        // would accept the links it signs.
        eprintln!(
            "FATAL: litegen could not load its link-signing key from the database (the error is above). \
             Without it this replica would sign links that no other replica, and no restart, accepts. \
             Fix the database, or set LITEGEN__SECRETS_KEY. If the stored value is damaged, delete the \
             link_signing_key row of server_secrets (links already handed out then stop verifying)."
        );
        std::process::exit(1);
    }
    let router = ProxyRouter::new(
        registry.clone(),
        cache.clone(),
        config.clone(),
        image_store.clone(),
    )
    .with_link_signer(Arc::new(link_signer));
    let router = Arc::new(match &shared {
        Some(shared) => router.with_shared(shared.clone()),
        None => router,
    });

    // Build materializer (uses storage adapter for temp ref image uploads)
    let storage_adapter = Arc::new(StorageAdapter::new(
        Arc::new(litegen::proxy::storage::LocalStorage)
    ));
    // SSRF hardening: reference-image URLs are user-supplied and fetched
    // server-side. The client follows no redirects, so a public host can't 3xx
    // into an internal target, and connects only to public addresses, so a
    // name whose answer changes after Materializer::fetch_url's check can't
    // either (DNS rebinding). See util::ssrf.
    let materializer = Arc::new(Materializer::new(
        storage_adapter,
        litegen::util::ssrf::public_only_client(),
    ));

    // Load capability registry
    let cap_registry = std::sync::Arc::new(
        litegen::capabilities::CapabilityRegistry::from_dir(&litegen::models_dir_path())
            .unwrap_or_else(|e| {
                eprintln!("failed to load model registry: {e}");
                std::process::exit(1);
            })
    );

    // A mapping that would do nothing is a config mistake worth seeing, and so
    // is a credential that no longer authenticates (Kling's old key pair).
    for w in litegen::proxy::registry::model_mapping_warnings(&config.providers, &cap_registry) {
        tracing::warn!("{w}");
    }
    for w in config.provider_credential_warnings() {
        tracing::warn!("{w}");
    }

    // Build OAuth config from environment
    let oauth = litegen::auth::oauth::OAuthConfig::from_env();
    if !oauth.enabled_providers().is_empty() {
        info!(providers = ?oauth.enabled_providers(), "OAuth providers enabled");
    }

    // Decode the BYO-credential encryption key (base64 -> 32 bytes).
    let secrets_key = decode_secrets_key(&config.secrets_key);
    if config.mode == Mode::Hosted && secrets_key.is_none() {
        tracing::error!(
            "Hosted mode requires LITEGEN__SECRETS_KEY to be a base64-encoded 32-byte key; \
             credential encryption will be unavailable until it is set"
        );
    }

    // Build app state
    let state = Arc::new(AppState {
        router: router.clone(),
        db: db.clone(),
        master_key: config.master_key.clone(),
        registry: cap_registry,
        materializer: materializer.clone(),
        // With Redis, every replica draws from the same buckets.
        rate_limiter: std::sync::Arc::new(match &shared {
            Some(shared) => litegen::api::middleware::rate_limit::RateLimiter::with_shared(shared.clone()),
            None => litegen::api::middleware::rate_limit::RateLimiter::new(),
        }),
        // Bodies are read within a budget of max_in_flight × 25 MiB and a
        // deadline, before a slot is taken (api/middleware/body.rs).
        in_flight: std::sync::Arc::new(
            litegen::api::middleware::backpressure::InFlightLimit::with_body_timeout(
                config.backpressure.max_in_flight,
                config.server.request_body_timeout(),
            ),
        ),
        oauth,
        mode: config.mode,
        secrets_key,
        dev: config.dev.clone(),
        allow_password: config.auth.allow_password,
    });

    // Layer what the management API stored (runtime models, provider configs,
    // routes, settings) over the YAML/env config.
    match litegen::management::runtime::apply_at_boot(&state).await {
        Ok(report) => tracing::info!(?report, "management runtime config applied"),
        Err(e) => tracing::error!(error = %e, "failed to load management runtime config"),
    }
    // What another replica saves (settings, routes, models, providers) reaches
    // this one within 15 s, and within about a second when Redis is shared.
    litegen::management::runtime::spawn_runtime_sync(state.clone(), std::time::Duration::from_secs(15), true);

    // Single-tenant: the Owner is recorded twice, and transfers before
    // 2026-11-05 moved only one record. Name a pair left apart (docs/RUNBOOK.md).
    if config.mode == Mode::SingleTenant {
        match litegen::api::handlers::users::split_owner_records(&*db).await {
            Ok(Some((by_membership, by_account))) => tracing::warn!(
                ?by_membership,
                ?by_account,
                "SECURITY: the deployment's Owner is recorded differently in the default org's membership (which holds the permissions) \
                 and in the account role; one transfer to the intended Owner repairs both: \
                 POST /v1/orgs/{}/transfer-owner (see docs/RUNBOOK.md)",
                litegen::api::middleware::DEFAULT_ORG_ID
            ),
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, "could not check the Owner records"),
        }
    }

    // Shutdown coordination: one stop signal (SIGINT/ctrl-c, or SIGTERM on
    // unix; see `StopSignals`) fans out to the poller and to the server loop,
    // which drain their work within the grace (`server.shutdown_grace_secs`).
    // The channel carries the moment the signal came: the grace runs from it.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(None::<tokio::time::Instant>);
    let stopped = |rx: &tokio::sync::watch::Receiver<Option<tokio::time::Instant>>| {
        let mut rx = rx.clone();
        async move {
            let _ = rx.wait_for(Option::is_some).await;
        }
    };
    let poller_shutdown = stopped(&shutdown_rx);
    // The grace is read at the stop: the vendor timeout it follows can be
    // changed at runtime (gateway settings).
    let shutdown_grace = {
        let (config, router) = (config.clone(), router.clone());
        move || config.server.shutdown_grace(router.settings().vendor_timeout)
    };
    let server_shutdown = {
        let (until_stop, grace) = (stopped(&shutdown_rx), shutdown_grace.clone());
        async move {
            until_stop.await;
            grace()
        }
    };

    // L4: a reference for a vendor that refuses `data:` URIs is published on
    // the same asset store, at litegen's public origin.
    materializer.set_publisher(Arc::new(litegen::proxy::ref_publish::RefPublisher::new(
        router.model3d_store.clone(),
        config.server.public_base_url(),
    )));

    // Spawn background video-generation poller
    let poller_handle = spawn_poller(
        db.clone(),
        registry.clone(),
        secrets_key,
        config.mode,
        // Share the router's store rather than building a second instance, so
        // both observers of completion write through the same backend.
        router.model3d_store.clone(),
        // Webhooks carry each app's files as its access mode hands them out.
        Some(litegen::access::present::Presenting { router: router.clone(), db: db.clone(), secrets_key, mode: config.mode }),
        poller_shutdown,
    );

    // Each app's retention_days (docs/storage-access.md), in every store.
    litegen::proxy::retention::spawn_retention_sweep(litegen::access::present::Presenting {
        router: router.clone(),
        db: db.clone(),
        secrets_key,
        mode: config.mode,
    });

    // MCP OAuth keeps short-lived rows (sign-in requests, codes, tokens):
    // clear out the expired ones hourly.
    if config.mcp.enabled && config.mcp.oauth.enabled {
        litegen::mcp::oauth::spawn_purger(db.clone(), stopped(&shutdown_rx));
    }

    // The local backend also keeps assets on disk no longer than its own
    // retention (never under 7 days), whatever an app sets.
    if config.image_storage.backend != "s3" {
        // Every kept result is written here, after its vendor billed it: a
        // directory litegen cannot write is a refusal to start, not a
        // failure at every delivery.
        if let Err(e) = litegen::proxy::storage::check_local_dir_writable(&config.image_storage.local_dir()) {
            tracing::error!("{e}");
            std::process::exit(1);
        }
        litegen::proxy::storage::spawn_retention_sweeper(
            config.image_storage.local_dir(),
            config.image_storage.retention_days(),
        );
    }

    // A kept video waits on disk between its download and the store (review
    // 2026-11-11, P4): under the asset directory, in S3 mode too (the Docker
    // image's volume), and in the system temp dir only when litegen cannot
    // write there. What a stop or crash left there a day ago is removed.
    let spool = config.image_storage.local_dir().join("litegen").join(".spool");
    match litegen::proxy::storage::check_local_dir_writable(&spool) {
        Ok(()) => litegen::proxy::storage::set_video_spool_dir(spool),
        Err(e) => tracing::warn!("{e}; finished videos wait in the system temp dir ({}) instead", std::env::temp_dir().display()),
    }
    let swept = litegen::proxy::storage::sweep_spool(
        &litegen::proxy::storage::video_spool_dir(),
        std::time::Duration::from_secs(86_400),
        std::time::SystemTime::now(),
    );
    if swept > 0 {
        tracing::info!(removed = swept, "removed videos left mid-keep by an earlier run");
    }

    // Install signal handlers: ctrl-c (SIGINT) or, on unix, SIGTERM (what
    // Docker and Kubernetes send to stop a container) starts the graceful stop.
    let stop = StopSignals::listen();
    tokio::spawn(async move {
        let signal = stop.wait().await;
        info!(signal, "received the stop signal, shutting down gracefully");
        let _ = shutdown_tx.send(Some(tokio::time::Instant::now()));
    });

    // Build CORS layer from config. LITEGEN_CORS_ORIGINS (single underscore,
    // a CSV) overrides the configured origins: the config library's
    // double-underscore nesting cannot deserialize a CSV into Vec<String>.
    let cors_layer = api::cors::layer(&config.cors, std::env::var("LITEGEN_CORS_ORIGINS").ok().as_deref());

    // Start the generation call hooks (docs/hooks.md). A hook that can't
    // start stops litegen: an auth hook must never silently go missing. Hook
    // paths start in the config file's folder, as LiteLLM's do: for a
    // litegen.yaml found in the working directory, that directory.
    let hooks_dir = match config.config_file.as_deref().and_then(std::path::Path::parent) {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let hooks = match litegen::hooks::HookSet::start(
        &config.hooks,
        &hooks_dir,
        config.backpressure.max_in_flight,
        config.server.request_body_timeout(),
    )
    .await
    {
        Ok(hooks) => hooks,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    if let Some(h) = &hooks {
        info!(hooks = ?h.names(), "generation call hooks running");
    }

    // Build axum router
    let app = api::create_router_with_hooks(state, hooks)
        .layer(cors_layer)
        .layer(TraceLayer::new_for_http());

    // Start server
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    info!(addr = %addr, "LiteGen proxy listening");
    println!();
    println!("  🚀 LiteGen proxy ready at http://{}", addr);
    println!("  📊 Health at http://{}/health", addr);
    if config.observability.metrics_enabled {
        let gated = if config.observability.metrics_require_auth { " (needs a key)" } else { "" };
        println!("  📈 Metrics at http://{}{}{gated}", addr, config.observability.metrics_path);
    } else {
        println!("  📈 Metrics off (observability.metrics_enabled)");
    }
    println!();

    // Each request carries its connection's peer address (the MCP OAuth
    // endpoints limit sign-ins per address). A connection must send its
    // headers within `server.header_read_timeout_secs`, and a stop waits for
    // open requests no longer than the grace.
    let limits = litegen::serve::ServeLimits { header_read_timeout: config.server.header_read_timeout() };
    match litegen::serve::serve(listener, app, server_shutdown, limits).await? {
        litegen::serve::Stopped::Drained => info!("every connection finished; waiting for the poller's current rows"),
        litegen::serve::Stopped::GraceExpired { cut } => tracing::warn!(
            cut,
            "the shutdown grace ran out: cut {cut} connections with their requests (server.shutdown_grace_secs)"
        ),
    }

    // The poller starts nothing new after the stop; the rows it is polling
    // get what is left of the grace, then are cut. Their claims are given
    // back below either way.
    let deadline = shutdown_rx.borrow().unwrap_or_else(tokio::time::Instant::now) + shutdown_grace();
    let mut poller_handle = poller_handle;
    if tokio::time::timeout_at(deadline, &mut poller_handle).await.is_err() {
        tracing::warn!("the shutdown grace ran out while the poller was polling: its current rows are cut (server.shutdown_grace_secs)");
        poller_handle.abort();
    }

    // The poller has stopped for good, so its claims are no use to anyone.
    // Give them back: the other replicas then poll these rows at their next
    // tick, not once the 30 s hold lapses. A failure only costs that wait,
    // so it is logged and never stops the shutdown; the time limit keeps a
    // database that has gone away from using up the grace period.
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        db.release_poll_claims(litegen::shared::replica_id()),
    )
    .await
    {
        Ok(Ok(freed)) => info!(freed, "released this replica's poll claims"),
        Ok(Err(e)) => tracing::warn!(error = %e, "could not release this replica's poll claims; other replicas poll those rows once the claims lapse (30 s)"),
        Err(_) => tracing::warn!("releasing this replica's poll claims took more than 5 s; other replicas poll those rows once the claims lapse (30 s)"),
    }

    // The webhooks of jobs that ended before the stop (by the poller, a
    // client's poll or a cancel) may still be going out, or waiting in their
    // back-off: they get what is left of the grace. Nothing sends them again,
    // so any still running at the deadline are abandoned, and named.
    let abandoned = litegen::proxy::webhook::deliveries().drain(deadline).await;
    if !abandoned.is_empty() {
        tracing::warn!(
            count = abandoned.len(),
            generations = ?abandoned,
            "the shutdown grace ran out with webhook deliveries still running: they were abandoned, and these generations' webhooks were not delivered (server.shutdown_grace_secs)"
        );
    }
    info!("LiteGen proxy stopped");

    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A real SIGTERM, sent to this test process: the handler is installed by
    /// `listen()` first, so the signal is caught, not fatal. Without SIGTERM
    /// support the wait would never end, and the timeout says so.
    #[tokio::test]
    async fn sigterm_stops_litegen_as_ctrl_c_does() {
        let stop = StopSignals::listen();
        let sent = std::process::Command::new("kill").args(["-TERM", &std::process::id().to_string()]).status().expect("run kill");
        assert!(sent.success(), "kill -TERM sent");
        let which = tokio::time::timeout(std::time::Duration::from_secs(5), stop.wait())
            .await
            .expect("a SIGTERM stops litegen gracefully, like ctrl-c");
        assert_eq!(which, "SIGTERM");
    }
}
