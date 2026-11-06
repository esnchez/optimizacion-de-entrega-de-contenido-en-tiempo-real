//! Logging (tracing) and metrics (Prometheus) setup, plus the per-request metrics middleware.

use std::time::Instant;

use axum::{extract::MatchedPath, extract::Request, middleware::Next, response::Response};
use metrics_exporter_prometheus::{BuildError, Matcher, PrometheusBuilder, PrometheusHandle};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// JSON logs when `LOG_FORMAT=json` (for production log pipelines), human-readable otherwise.
pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let registry = tracing_subscriber::registry().with(filter);
    if std::env::var("LOG_FORMAT").is_ok_and(|f| f == "json") {
        registry
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    } else {
        registry.with(tracing_subscriber::fmt::layer()).init();
    }
}

/// Installs the global metrics recorder; the handle renders the `/metrics` text format.
pub fn init_metrics() -> Result<PrometheusHandle, BuildError> {
    // Buckets focused on the p99 < 50ms target. Cached responses take well under 1 ms, so the
    // sub-millisecond buckets are needed for quantiles to show real values instead of "1 ms".
    const LATENCY_BUCKETS: &[f64] = &[
        0.0001, 0.00025, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5,
        5.0,
    ];
    PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Full("http_request_duration_seconds".to_owned()),
            LATENCY_BUCKETS,
        )?
        .install_recorder()
}

/// Records request count, latency (time to response headers) and in-flight requests.
pub async fn track_metrics(req: Request, next: Next) -> Response {
    let start = Instant::now();
    // The route template (`/content/{*key}`) rather than the raw path keeps label cardinality low.
    let path = req
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "unmatched".to_owned(), |p| p.as_str().to_owned());
    let method = req.method().to_string();

    let in_flight = metrics::gauge!("http_requests_in_flight");
    in_flight.increment(1.0);
    let response = next.run(req).await;
    in_flight.decrement(1.0);

    let labels = [
        ("method", method),
        ("path", path),
        ("status", response.status().as_u16().to_string()),
    ];
    metrics::counter!("http_requests_total", &labels).increment(1);
    metrics::histogram!("http_request_duration_seconds", &labels)
        .record(start.elapsed().as_secs_f64());
    response
}
