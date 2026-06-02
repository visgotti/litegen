/// Observability setup: tracing-subscriber + optional OpenTelemetry OTLP export.
///
/// If the `OTEL_EXPORTER_OTLP_ENDPOINT` environment variable is set the
/// subscriber registry is augmented with an OTLP gRPC tracer layer via
/// `tracing_opentelemetry`.  When the variable is absent or empty only the
/// standard `tracing-subscriber` fmt layer is used.
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    runtime::Tokio,
    trace::{RandomIdGenerator, Sampler},
    Resource,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Build an OTLP gRPC `TracerProvider` and return the installed `opentelemetry::trace::Tracer`.
///
/// Returns `None` when `OTEL_EXPORTER_OTLP_ENDPOINT` is unset or empty.
/// The tracer is installed as the global OTel tracer via `opentelemetry::global::set_tracer_provider`.
pub fn build_otel_tracer() -> Option<opentelemetry_sdk::trace::Tracer> {
    let endpoint = match std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        Ok(ep) if !ep.trim().is_empty() => ep,
        _ => return None,
    };

    let resource = Resource::new(vec![KeyValue::new("service.name", "litegen")]);

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&endpoint)
        .build()
        .expect("failed to build OTLP span exporter");

    let provider = opentelemetry_sdk::trace::TracerProvider::builder()
        .with_batch_exporter(exporter, Tokio)
        .with_sampler(Sampler::AlwaysOn)
        .with_id_generator(RandomIdGenerator::default())
        .with_resource(resource)
        .build();

    let tracer = opentelemetry::trace::TracerProvider::tracer(&provider, "litegen");
    opentelemetry::global::set_tracer_provider(provider);
    Some(tracer)
}

/// Initialise the tracing subscriber registry.
///
/// Chains:
///  1. `EnvFilter` (from `RUST_LOG` env, falling back to `level`).
///  2. Optional OTLP gRPC export layer when `OTEL_EXPORTER_OTLP_ENDPOINT` is set.
///  3. `fmt` layer (pretty or JSON depending on `json`).
///
/// The OTel layer must come before the fmt layer in the chain so that it is
/// parameterised on a subscriber type that satisfies its bounds.
pub fn init_tracing(level: &str, json: bool) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level));

    let otel_layer = build_otel_tracer()
        .map(|tracer| tracing_opentelemetry::layer().with_tracer(tracer));

    if json {
        tracing_subscriber::registry()
            .with(filter)
            .with(otel_layer)
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(otel_layer)
            .with(tracing_subscriber::fmt::layer().pretty())
            .init();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// When OTEL_EXPORTER_OTLP_ENDPOINT is not set, build_otel_tracer returns None.
    #[test]
    fn build_otel_tracer_returns_none_when_env_unset() {
        let prev = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");

        let tracer = build_otel_tracer();
        assert!(tracer.is_none(), "expected None when OTEL_EXPORTER_OTLP_ENDPOINT is unset");

        if let Some(v) = prev {
            std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", v);
        }
    }

    /// When OTEL_EXPORTER_OTLP_ENDPOINT is set to an empty string, still return None.
    #[test]
    fn build_otel_tracer_returns_none_when_env_empty() {
        let prev = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
        std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "");

        let tracer = build_otel_tracer();
        assert!(tracer.is_none(), "expected None when OTEL_EXPORTER_OTLP_ENDPOINT is empty");

        match prev {
            Some(v) => std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", v),
            None => std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT"),
        }
    }

    /// When OTEL_EXPORTER_OTLP_ENDPOINT is set to a non-empty value, build_otel_tracer
    /// returns Some.  We use a fake endpoint — the tonic exporter creates a lazy
    /// channel and does not actually connect, but it does require a Tokio runtime.
    #[tokio::test]
    async fn build_otel_tracer_returns_some_when_env_set() {
        let prev = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
        std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://localhost:4317");

        let tracer = build_otel_tracer();
        assert!(tracer.is_some(), "expected Some when OTEL_EXPORTER_OTLP_ENDPOINT is set");

        match prev {
            Some(v) => std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", v),
            None => std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT"),
        }
    }
}
