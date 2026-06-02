use std::sync::Arc;
use axum::http::HeaderValue;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

use litegen::api;
use litegen::api::middleware::AppState;
use litegen::config::{self, load_config};
use litegen::observability::init_tracing;
use litegen::proxy::{GenerationCache, ProxyRouter, ProviderRegistry, build_image_store, spawn_poller};
use litegen::proxy::materializer::{Materializer, StorageAdapter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env file if present
    let _ = dotenvy::dotenv();

    // Load configuration
    let config = load_config().unwrap_or_else(|e| {
        eprintln!("Warning: config load error ({e}), using defaults");
        config::AppConfig::default()
    });
    let config = Arc::new(config);

    // Initialize Prometheus metrics recorder (no built-in HTTP listener;
    // served via the axum router at GET /metrics).
    litegen::api::init_prometheus();

    // Install W3C TraceContext propagator so outbound HTTP calls carry
    // `traceparent` / `tracestate` headers for distributed tracing.
    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );

    // Initialize tracing (and optionally OTel OTLP export if
    // OTEL_EXPORTER_OTLP_ENDPOINT is set).
    init_tracing(&config.observability.log_level, config.observability.json_logs);

    info!(
        host = %config.server.host,
        port = config.server.port,
        "Starting LiteGen proxy"
    );

    // Connect to database
    let db: Arc<dyn litegen::db::DatabaseStore> =
        Arc::from(litegen::db::connect(&config.database_url).await?);

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
    if cache.is_enabled() {
        info!("Generation cache enabled");
    }

    // Initialize image storage backend
    let image_store = build_image_store(&config.image_storage);

    // Build proxy router
    let router = Arc::new(ProxyRouter::new(
        registry.clone(),
        cache.clone(),
        config.clone(),
        image_store.clone(),
    ));

    // Build materializer (uses storage adapter for temp ref image uploads)
    let storage_adapter = Arc::new(StorageAdapter::new(
        Arc::new(litegen::proxy::storage::LocalStorage)
    ));
    let materializer = Arc::new(Materializer::new(
        storage_adapter,
        reqwest::Client::new(),
    ));

    // Load capability registry
    let cap_registry = std::sync::Arc::new(
        litegen::capabilities::CapabilityRegistry::from_dir(&litegen::models_dir_path())
            .unwrap_or_else(|e| {
                eprintln!("failed to load model registry: {e}");
                std::process::exit(1);
            })
    );

    // Build OAuth config from environment
    let oauth = litegen::auth::oauth::OAuthConfig::from_env();
    if !oauth.enabled_providers().is_empty() {
        info!(providers = ?oauth.enabled_providers(), "OAuth providers enabled");
    }

    // Build app state
    let state = Arc::new(AppState {
        router: router.clone(),
        db: db.clone(),
        master_key: config.master_key.clone(),
        registry: cap_registry,
        materializer,
        rate_limiter: std::sync::Arc::new(litegen::api::middleware::rate_limit::RateLimiter::new()),
        in_flight: std::sync::Arc::new(
            litegen::api::middleware::backpressure::InFlightLimit::new(
                config.backpressure.max_in_flight,
            ),
        ),
        oauth,
    });

    // Shutdown coordination: one signal fans out to the poller and to
    // axum's graceful_shutdown so SIGINT/SIGTERM cleanly drains in-flight work.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let poller_shutdown = {
        let mut rx = shutdown_rx.clone();
        async move { let _ = rx.changed().await; }
    };
    let server_shutdown = {
        let mut rx = shutdown_rx.clone();
        async move { let _ = rx.changed().await; }
    };

    // Spawn background video-generation poller
    let poller_handle = spawn_poller(
        db.clone(),
        registry.clone(),
        reqwest::Client::new(),
        poller_shutdown,
    );

    // Install signal handlers
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        info!("ctrl-c received, shutting down gracefully");
        let _ = shutdown_tx.send(true);
    });

    // Build CORS layer from config
    let cors_layer = {
        use axum::http::Method;

        let allowed_methods = [
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ];
        let allowed_headers = [
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderName::from_static("x-litegen-strict"),
            axum::http::HeaderName::from_static("x-csrf-token"),
        ];

        // Also check env var for CSV override (env takes precedence over config file).
        // Use LITEGEN_CORS_ORIGINS (single underscore) to avoid the config library's
        // LITEGEN__CORS__ALLOWED_ORIGINS double-underscore nesting parser, which
        // cannot deserialize a CSV string directly into Vec<String>.
        let origins_from_env: Option<String> = std::env::var("LITEGEN_CORS_ORIGINS").ok()
            .filter(|s| !s.is_empty());
        let effective_origins: Vec<String> = if let Some(csv) = origins_from_env {
            csv.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
        } else {
            config.cors.allowed_origins.clone()
        };

        let layer = if effective_origins.iter().any(|o| o == "*") {
            // Wildcard — allow any origin (dev convenience)
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(allowed_methods.to_vec())
                .allow_headers(allowed_headers.to_vec())
        } else if effective_origins.is_empty() {
            // Default-deny: no origins allowed
            CorsLayer::new()
                .allow_methods(allowed_methods.to_vec())
                .allow_headers(allowed_headers.to_vec())
        } else {
            // Explicit allowlist
            let parsed: Vec<HeaderValue> = effective_origins
                .iter()
                .filter_map(|o| HeaderValue::from_str(o).ok())
                .collect();
            CorsLayer::new()
                .allow_origin(parsed)
                .allow_methods(allowed_methods.to_vec())
                .allow_headers(allowed_headers.to_vec())
        };

        if config.cors.allow_credentials {
            layer.allow_credentials(true)
        } else {
            layer
        }
    };

    // Build axum router
    let app = api::create_router(state)
        .layer(cors_layer)
        .layer(TraceLayer::new_for_http());

    // Start server
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    info!(addr = %addr, "LiteGen proxy listening");
    println!();
    println!("  🚀 LiteGen proxy ready at http://{}", addr);
    println!("  📊 Health at http://{}/health", addr);
    println!("  📈 Metrics at http://{}/metrics", addr);
    println!();

    axum::serve(listener, app)
        .with_graceful_shutdown(server_shutdown)
        .await?;

    // After axum exits, wait for the poller to drain.
    info!("axum shut down, awaiting poller drain");
    let _ = poller_handle.await;
    info!("LiteGen proxy stopped");

    Ok(())
}
