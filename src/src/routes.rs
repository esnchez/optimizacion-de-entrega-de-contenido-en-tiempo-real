//! HTTP routes, the middleware stack and the handlers.

use std::time::Duration;

use axum::{
    Router,
    body::Body,
    error_handling::HandleErrorLayer,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use tokio_util::io::ReaderStream;
use tower::{BoxError, ServiceBuilder};
use tower_http::{
    cors::{Any, CorsLayer},
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

use crate::{connections, error::AppError, state::AppState, storage::ObjectMeta, telemetry};

/// S3 keys are at most 1024 bytes of UTF-8.
const MAX_KEY_BYTES: usize = 1024;

pub fn router(state: AppState) -> Router {
    let cfg = &state.config;

    // Content routes are protected by backpressure: once `max_concurrent_requests` are in flight,
    // extra requests get an immediate 503 instead of queueing and pushing everybody's latency up.
    let content = Router::new()
        .route("/content/{*key}", get(get_content).head(head_content))
        .route("/url/{*key}", get(presigned_url))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(|_: BoxError| async {
                    metrics::counter!("http_requests_shed_total").increment(1);
                    StatusCode::SERVICE_UNAVAILABLE
                }))
                .load_shed()
                .concurrency_limit(cfg.max_concurrent_requests),
        )
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            cfg.request_timeout,
        ))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::HEAD])
                .max_age(Duration::from_secs(3600)),
        );

    // Probes and metrics stay outside the limiter: an overloaded pod must not fail its liveness
    // probe and get restarted, which would push its load onto the others.
    let ops = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics_handler));

    content
        .merge(ops)
        .route_layer(middleware::from_fn(telemetry::track_metrics))
        // Applies to every route: any response can be the one that closes an old connection.
        .layer(middleware::from_fn(connections::close_expired_connections))
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
                .layer(TraceLayer::new_for_http())
                .layer(PropagateRequestIdLayer::x_request_id()),
        )
        .with_state(state)
}

/// `GET /content/{key}`: serves the object, from memory when small, streamed from S3 otherwise.
/// Supports `Range` (seeking in video/audio players) and `If-None-Match` (304 revalidation).
async fn get_content(
    State(state): State<AppState>,
    Path(key): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    validate_key(&key)?;
    let meta = state
        .content
        .metadata(&key)
        .await?
        .ok_or(AppError::NotFound)?;

    if etag_matches(&headers, meta.etag.as_deref()) {
        return Ok(not_modified(&meta));
    }

    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());

    // Hot path: small, full-object requests come from memory.
    if range.is_none() && state.content.is_small(&meta) {
        let body = state.content.small_body(&key, &meta).await?;
        let mut response = Response::new(Body::from(body));
        apply_meta_headers(response.headers_mut(), &meta, &state.config.cache_control);
        return Ok(response);
    }

    // Large objects and ranges are streamed chunk by chunk: memory per request stays at about
    // one chunk no matter how large the object is.
    let object = match state.content.stream(&key, range).await {
        Err(AppError::RangeNotSatisfiable) => return Ok(range_not_satisfiable(meta.size)),
        other => other?,
    };

    let status = if object.content_range.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let reader = object.body.into_async_read();
    let stream = ReaderStream::with_capacity(reader, state.config.stream_chunk_bytes);

    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    let h = response.headers_mut();
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    insert_opt(h, header::CACHE_CONTROL, Some(&state.config.cache_control));
    insert_opt(h, header::CONTENT_TYPE, object.content_type.as_deref());
    insert_opt(h, header::ETAG, object.etag.as_deref());
    insert_opt(h, header::LAST_MODIFIED, object.last_modified.as_deref());
    insert_opt(h, header::CONTENT_RANGE, object.content_range.as_deref());
    if let Some(len) = object.content_length {
        h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    }
    Ok(response)
}

/// `HEAD /content/{key}`: answered entirely from the metadata cache, no body transfer.
async fn head_content(
    State(state): State<AppState>,
    Path(key): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    validate_key(&key)?;
    let meta = state
        .content
        .metadata(&key)
        .await?
        .ok_or(AppError::NotFound)?;
    if etag_matches(&headers, meta.etag.as_deref()) {
        return Ok(not_modified(&meta));
    }
    let mut response = Response::new(Body::empty());
    apply_meta_headers(response.headers_mut(), &meta, &state.config.cache_control);
    Ok(response)
}

/// `GET /url/{key}`: redirects to a short-lived presigned S3 URL. The bytes then flow straight
/// from S3 to the client, which takes the bandwidth off this service for very large files.
async fn presigned_url(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Response, AppError> {
    validate_key(&key)?;
    // Checking existence first (usually a cache hit) avoids handing out URLs that will 404.
    state
        .content
        .metadata(&key)
        .await?
        .ok_or(AppError::NotFound)?;
    let url = state
        .content
        .store()
        .presign_get(&key, state.config.presign_ttl)
        .await?;
    Ok(Redirect::temporary(&url).into_response())
}

async fn readyz(State(state): State<AppState>) -> StatusCode {
    if state.is_ready() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

async fn metrics_handler(State(state): State<AppState>) -> String {
    state.metrics.render()
}

fn validate_key(key: &str) -> Result<(), AppError> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES {
        return Err(AppError::InvalidKey);
    }
    Ok(())
}

/// True when the client's cached copy (`If-None-Match`) is still current.
fn etag_matches(headers: &HeaderMap, etag: Option<&str>) -> bool {
    let (Some(etag), Some(value)) = (etag, headers.get(header::IF_NONE_MATCH)) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    value.split(',').map(str::trim).any(|candidate| {
        candidate == "*" || candidate.trim_start_matches("W/") == etag.trim_start_matches("W/")
    })
}

fn not_modified(meta: &ObjectMeta) -> Response {
    let mut response = StatusCode::NOT_MODIFIED.into_response();
    insert_opt(response.headers_mut(), header::ETAG, meta.etag.as_deref());
    response
}

fn range_not_satisfiable(size: u64) -> Response {
    let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
    insert_opt(
        response.headers_mut(),
        header::CONTENT_RANGE,
        Some(&format!("bytes */{size}")),
    );
    response
}

fn apply_meta_headers(h: &mut HeaderMap, meta: &ObjectMeta, cache_control: &str) {
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(meta.size));
    insert_opt(h, header::CACHE_CONTROL, Some(cache_control));
    insert_opt(h, header::CONTENT_TYPE, meta.content_type.as_deref());
    insert_opt(h, header::ETAG, meta.etag.as_deref());
    insert_opt(h, header::LAST_MODIFIED, meta.last_modified.as_deref());
}

/// Inserts a header when present and valid; values that aren't valid header text are skipped.
fn insert_opt(h: &mut HeaderMap, name: header::HeaderName, value: Option<&str>) {
    if let Some(value) = value.and_then(|v| HeaderValue::from_str(v).ok()) {
        h.insert(name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_inm(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::IF_NONE_MATCH, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn etag_match_handles_lists_weak_tags_and_wildcard() {
        assert!(etag_matches(&with_inm("\"abc\""), Some("\"abc\"")));
        assert!(etag_matches(&with_inm("\"x\", W/\"abc\""), Some("\"abc\"")));
        assert!(etag_matches(&with_inm("*"), Some("\"abc\"")));
        assert!(!etag_matches(&with_inm("\"other\""), Some("\"abc\"")));
        assert!(!etag_matches(&HeaderMap::new(), Some("\"abc\"")));
        assert!(!etag_matches(&with_inm("\"abc\""), None));
    }

    #[test]
    fn key_validation() {
        assert!(validate_key("videos/intro.mp4").is_ok());
        assert!(validate_key("").is_err());
        assert!(validate_key(&"a".repeat(MAX_KEY_BYTES + 1)).is_err());
    }
}
