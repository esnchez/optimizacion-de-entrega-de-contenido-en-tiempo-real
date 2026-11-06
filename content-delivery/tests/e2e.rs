//! End-to-end tests: the running service against a real S3 API (LocalStack).
//!
//! They are `#[ignore]`d so a plain `cargo test` only runs the unit tests. Run them with Task,
//! which also starts the environment (`docker compose`) and stops it afterwards:
//!
//! ```text
//! task e2e            # LocalStack + the service image, then these tests
//! task e2e:dev        # against `task run` (service on the host, fast rebuilds)
//! task e2e:cluster    # against the minikube deployment
//! ```
//!
//! Each test uploads its own objects under a unique key, so tests don't depend on each other or on
//! a seeded bucket. Tests that count S3 calls read the service's own metrics, so they run one at a
//! time (`--test-threads=1`, set by the Task tasks) and send their requests to a single instance
//! (`OPS_URL`).
//!
//! Configuration (environment variables):
//!
//! | Variable | Default | Meaning |
//! | --- | --- | --- |
//! | `BASE_URL` | `http://127.0.0.1:8080` | Content endpoints (a pod, a container or the Ingress) |
//! | `OPS_URL` | `BASE_URL` | One single instance: metrics, S3-call counts, connection tests |
//! | `RESOLVE` | — | `host=ip:port`, e.g. `cdn.local=127.0.0.1:8080` to reach the Ingress by host name |
//! | `S3_ENDPOINT` | `http://127.0.0.1:4566` | LocalStack, as reachable from where the tests run |
//! | `BUCKET` | `content` | The service's `S3_BUCKET` |
//! | `CACHE_TTL_SECS` | `2` | The service's cache TTL; the object-update test waits this long |
//! | `MAX_CONNECTION_AGE_SECS` | `0` | The service's value; `0` skips the connection-recycling test |
//! | `FOLLOW_PRESIGN` | `0` | `1` also downloads through the presigned URL (S3 host must be reachable) |

use std::{
    env,
    net::SocketAddr,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use aws_sdk_s3::{
    Client as S3,
    config::{BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
};
use reqwest::{Client, Response, StatusCode, header, redirect};

// --------------------------------------------------------------------------------------------
// Test environment
// --------------------------------------------------------------------------------------------

struct Env {
    http: Client,
    base: String,
    ops: String,
    s3: S3,
    bucket: String,
    cache_ttl: Duration,
    max_age: Option<Duration>,
    follow_presign: bool,
}

fn var(name: &str, default: &str) -> String {
    env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

/// HTTP client without redirects (the presign test checks the 307 itself), honouring `RESOLVE`.
fn http_client(builder: reqwest::ClientBuilder) -> Client {
    let mut builder = builder.redirect(redirect::Policy::none()).no_proxy();
    if let Some(resolve) = env::var("RESOLVE").ok().filter(|v| !v.is_empty()) {
        let (host, addr) = resolve
            .split_once('=')
            .expect("RESOLVE must be host=ip:port");
        let addr: SocketAddr = addr.parse().expect("RESOLVE address must be ip:port");
        builder = builder.resolve(host, addr);
    }
    builder.build().unwrap()
}

impl Env {
    async fn new() -> Self {
        let base = var("BASE_URL", "http://127.0.0.1:8080");
        let ops = var("OPS_URL", &base);
        let s3_conf = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new("test", "test", None, None, "e2e"))
            .endpoint_url(var("S3_ENDPOINT", "http://127.0.0.1:4566"))
            .force_path_style(true)
            .build();
        let max_age: u64 = var("MAX_CONNECTION_AGE_SECS", "0").parse().unwrap();
        let env = Self {
            http: http_client(Client::builder()),
            base,
            ops,
            s3: S3::from_conf(s3_conf),
            bucket: var("BUCKET", "content"),
            cache_ttl: Duration::from_secs(var("CACHE_TTL_SECS", "2").parse().unwrap()),
            max_age: (max_age > 0).then(|| Duration::from_secs(max_age)),
            follow_presign: var("FOLLOW_PRESIGN", "0") == "1",
        };
        // Idempotent: fails harmlessly when the bucket already exists.
        let _ = env.s3.create_bucket().bucket(&env.bucket).send().await;
        env.wait_until_ready().await;
        env
    }

    async fn wait_until_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let ready = self.http.get(format!("{}/readyz", self.ops)).send().await;
            if ready.is_ok_and(|r| r.status() == StatusCode::OK) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "service not ready at {}",
                self.ops
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// A key no other test (or earlier run) uses.
    fn unique_key(name: &str) -> String {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("e2e/{nanos}-{}-{name}", SEQ.fetch_add(1, Ordering::Relaxed))
    }

    async fn put(&self, key: &str, body: &[u8], content_type: &str) {
        self.s3
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .body(ByteStream::from(body.to_vec()))
            .send()
            .await
            .expect("upload to S3");
    }

    async fn get(&self, path: &str) -> Response {
        self.get_with(path, &[]).await
    }

    async fn get_with(&self, path: &str, headers: &[(&str, &str)]) -> Response {
        let mut req = self.http.get(format!("{}{path}", self.base));
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        req.send().await.expect("GET")
    }

    async fn head(&self, path: &str) -> Response {
        let url = format!("{}{path}", self.base);
        self.http.head(url).send().await.expect("HEAD")
    }

    /// GET on the single instance behind `OPS_URL`.
    async fn get_ops(&self, path: &str) -> Response {
        let url = format!("{}{path}", self.ops);
        self.http.get(url).send().await.expect("GET")
    }

    /// Current value of one exact Prometheus series on the `OPS_URL` instance, 0 if absent.
    async fn metric(&self, series: &str) -> f64 {
        let text = self.get_ops("/metrics").await.text().await.unwrap();
        text.lines()
            .find_map(|line| line.strip_prefix(series)?.strip_prefix(' '))
            .map_or(0.0, |value| value.trim().parse().unwrap())
    }
}

fn header(res: &Response, name: header::HeaderName) -> String {
    res.headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default()
}

/// Deterministic pseudo-random bytes (xorshift), so large-object tests compare exact contents.
fn pseudo_random(len: usize) -> Vec<u8> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

const METADATA_MISSES: &str = r#"cache_requests_total{cache="metadata",result="miss"}"#;
const BODY_MISSES: &str = r#"cache_requests_total{cache="body",result="miss"}"#;
/// Larger than `SMALL_OBJECT_MAX_BYTES` (1 MiB): served by streaming from S3, not from memory.
const LARGE: usize = 3 * 1024 * 1024;

// --------------------------------------------------------------------------------------------
// Operational endpoints
// --------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn health_readiness_and_metrics_endpoints_respond() {
    let env = Env::new().await;
    for path in ["/healthz", "/readyz", "/metrics"] {
        assert_eq!(env.get_ops(path).await.status(), 200, "{path}");
    }
}

// --------------------------------------------------------------------------------------------
// Small objects (served from memory)
// --------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn small_object_is_served_with_the_s3_metadata() {
    let env = Env::new().await;
    let key = Env::unique_key("hello.txt");
    env.put(&key, b"hello world", "text/plain").await;

    let res = env.get(&format!("/content/{key}")).await;
    assert_eq!(res.status(), 200);
    assert_eq!(header(&res, header::CONTENT_TYPE), "text/plain");
    assert_eq!(header(&res, header::CONTENT_LENGTH), "11");
    assert_eq!(header(&res, header::ACCEPT_RANGES), "bytes");
    assert_eq!(header(&res, header::CACHE_CONTROL), "public, max-age=300");
    assert!(!header(&res, header::ETAG).is_empty(), "ETag present");
    assert!(
        !header(&res, header::LAST_MODIFIED).is_empty(),
        "Last-Modified present"
    );
    assert_eq!(res.text().await.unwrap(), "hello world");
}

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn if_none_match_returns_304_only_for_the_current_etag() {
    let env = Env::new().await;
    let key = Env::unique_key("etag.txt");
    env.put(&key, b"hello world", "text/plain").await;
    let path = format!("/content/{key}");
    let etag = header(&env.get(&path).await, header::ETAG);

    let exact = env.get_with(&path, &[("if-none-match", &etag)]).await;
    assert_eq!(exact.status(), 304, "exact ETag");
    let list = format!("\"other\", W/{etag}");
    let weak_in_list = env.get_with(&path, &[("if-none-match", &list)]).await;
    assert_eq!(weak_in_list.status(), 304, "weak ETag inside a list");
    let other = env.get_with(&path, &[("if-none-match", "\"nope\"")]).await;
    assert_eq!(other.status(), 200, "different ETag");
}

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn keys_with_spaces_and_unicode_are_served() {
    let env = Env::new().await;
    let id = Env::unique_key("x").replace('/', "-");
    env.put(
        &format!("e2e/dir with space/ñandú-{id}.txt"),
        b"hola",
        "text/plain",
    )
    .await;

    let res = env
        .get(&format!(
            "/content/e2e/dir%20with%20space/%C3%B1and%C3%BA-{id}.txt"
        ))
        .await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), "hola");
}

// --------------------------------------------------------------------------------------------
// Large objects (streamed from S3) and ranges
// --------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn large_object_is_streamed_byte_for_byte() {
    let env = Env::new().await;
    let key = Env::unique_key("big.bin");
    let data = pseudo_random(LARGE);
    env.put(&key, &data, "application/octet-stream").await;
    let path = format!("/content/{key}");

    let head = env.head(&path).await;
    assert_eq!(head.status(), 200);
    assert_eq!(header(&head, header::CONTENT_LENGTH), LARGE.to_string());

    let body = env.get(&path).await.bytes().await.unwrap();
    assert!(body == data, "streamed body differs from the S3 object");
}

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn range_requests_return_the_exact_slice() {
    let env = Env::new().await;
    let big = Env::unique_key("big.bin");
    let data = pseudo_random(LARGE);
    env.put(&big, &data, "application/octet-stream").await;

    let res = env
        .get_with(&format!("/content/{big}"), &[("range", "bytes=100-199")])
        .await;
    assert_eq!(res.status(), 206);
    assert_eq!(
        header(&res, header::CONTENT_RANGE),
        format!("bytes 100-199/{LARGE}")
    );
    assert!(
        res.bytes().await.unwrap() == data[100..200],
        "slice differs from S3"
    );

    // Ranges on small objects also work (they go to S3 instead of memory).
    let small = Env::unique_key("hello.txt");
    env.put(&small, b"hello world", "text/plain").await;
    let res = env
        .get_with(&format!("/content/{small}"), &[("range", "bytes=0-4")])
        .await;
    assert_eq!(res.status(), 206);
    assert_eq!(res.text().await.unwrap(), "hello");
}

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn range_beyond_the_end_returns_416_with_the_size() {
    let env = Env::new().await;
    let key = Env::unique_key("big.bin");
    env.put(&key, &pseudo_random(LARGE), "application/octet-stream")
        .await;

    let res = env
        .get_with(&format!("/content/{key}"), &[("range", "bytes=99999999-")])
        .await;
    assert_eq!(res.status(), 416);
    assert_eq!(
        header(&res, header::CONTENT_RANGE),
        format!("bytes */{LARGE}")
    );
}

// --------------------------------------------------------------------------------------------
// Errors
// --------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn missing_keys_return_404() {
    let env = Env::new().await;
    let path = format!("/content/{}", Env::unique_key("missing"));
    assert_eq!(env.get(&path).await.status(), 404, "GET");
    assert_eq!(env.head(&path).await.status(), 404, "HEAD");
}

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn not_found_is_cached_so_repeated_requests_reach_s3_once() {
    let env = Env::new().await;
    let path = format!("/content/{}", Env::unique_key("missing"));
    let before = env.metric(METADATA_MISSES).await;
    for _ in 0..5 {
        assert_eq!(env.get_ops(&path).await.status(), 404);
    }
    assert_eq!(
        env.metric(METADATA_MISSES).await - before,
        1.0,
        "S3 HeadObject calls"
    );
}

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn invalid_key_returns_400_and_unknown_route_404() {
    let env = Env::new().await;
    let too_long = "a".repeat(1025);
    assert_eq!(env.get(&format!("/content/{too_long}")).await.status(), 400);
    assert_eq!(env.get("/nothing-here").await.status(), 404);
}

// --------------------------------------------------------------------------------------------
// Presigned redirect
// --------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn url_route_redirects_to_a_presigned_s3_url() {
    let env = Env::new().await;
    let key = Env::unique_key("hello.txt");
    env.put(&key, b"hello world", "text/plain").await;

    let res = env.get(&format!("/url/{key}")).await;
    assert_eq!(res.status(), 307);
    let location = header(&res, header::LOCATION);
    assert!(
        location.contains("X-Amz-Signature="),
        "signed URL: {location}"
    );
    assert_eq!(
        env.get(&format!("/url/{}", Env::unique_key("missing")))
            .await
            .status(),
        404
    );

    if env.follow_presign {
        let body = env
            .http
            .get(&location)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "hello world");
    }
}

// --------------------------------------------------------------------------------------------
// Middleware
// --------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn request_id_is_generated_and_propagated() {
    let env = Env::new().await;
    let key = Env::unique_key("hello.txt");
    env.put(&key, b"hello world", "text/plain").await;
    let path = format!("/content/{key}");

    // At least 32 characters: the service generates a UUID (36), ingress-nginx a 32-hex id.
    let generated = header(&env.get(&path).await, "x-request-id".parse().unwrap());
    assert!(
        generated.len() >= 32,
        "generated x-request-id: {generated:?}"
    );
    let given = env.get_with(&path, &[("x-request-id", "e2e-123")]).await;
    assert_eq!(header(&given, "x-request-id".parse().unwrap()), "e2e-123");
}

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn cors_allows_any_origin() {
    let env = Env::new().await;
    let key = Env::unique_key("hello.txt");
    env.put(&key, b"hello world", "text/plain").await;
    let res = env
        .get_with(
            &format!("/content/{key}"),
            &[("origin", "https://player.example")],
        )
        .await;
    assert_eq!(header(&res, header::ACCESS_CONTROL_ALLOW_ORIGIN), "*");
}

// --------------------------------------------------------------------------------------------
// Caching
// --------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn burst_of_200_requests_for_a_cold_key_makes_one_s3_call_of_each_kind() {
    let env = Env::new().await;
    let key = Env::unique_key("hot.json");
    env.put(&key, b"{}", "application/json").await;
    let url = format!("{}/content/{key}", env.ops);
    let (heads_before, gets_before) = (
        env.metric(METADATA_MISSES).await,
        env.metric(BODY_MISSES).await,
    );

    // Open the 200 connections first: through Docker Desktop's port mapping, opening them can take
    // longer than the 2 s cache TTL, and an entry that expires mid-burst is rightly loaded again.
    let send_burst = |url: String| {
        let mut burst = tokio::task::JoinSet::new();
        for _ in 0..200 {
            let (http, url) = (env.http.clone(), url.clone());
            burst.spawn(async move { http.get(url).send().await.unwrap().status() });
        }
        burst
    };
    let mut warm_up = send_burst(format!("{}/healthz", env.ops));
    while let Some(status) = warm_up.join_next().await {
        assert_eq!(status.unwrap(), 200);
    }

    let started = Instant::now();
    let mut burst = send_burst(url);
    while let Some(status) = burst.join_next().await {
        assert_eq!(status.unwrap(), 200);
    }
    let elapsed = started.elapsed();

    // One load per cache lifetime the burst spanned (exactly 1 when it fits inside the TTL).
    let allowed = 1.0 + (elapsed.as_secs_f64() / env.cache_ttl.as_secs_f64()).floor();
    let heads = env.metric(METADATA_MISSES).await - heads_before;
    let gets = env.metric(BODY_MISSES).await - gets_before;
    assert!(
        (1.0..=allowed).contains(&heads),
        "S3 HeadObject calls: {heads} for a burst of {elapsed:?}"
    );
    assert!(
        (1.0..=allowed).contains(&gets),
        "S3 GetObject calls: {gets} for a burst of {elapsed:?}"
    );
}

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn updated_object_is_served_after_the_cache_ttl() {
    let env = Env::new().await;
    let key = Env::unique_key("mutable.txt");
    let path = format!("/content/{key}");
    env.put(&key, b"v1", "text/plain").await;
    assert_eq!(env.get(&path).await.text().await.unwrap(), "v1");

    env.put(&key, b"version-2", "text/plain").await;
    tokio::time::sleep(env.cache_ttl + Duration::from_secs(1)).await;
    let res = env.get(&path).await;
    assert_eq!(header(&res, header::CONTENT_LENGTH), "9");
    assert_eq!(res.text().await.unwrap(), "version-2");
}

// --------------------------------------------------------------------------------------------
// Metrics and connections
// --------------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn request_cache_and_connection_metrics_are_exported() {
    let env = Env::new().await;
    let key = Env::unique_key("hello.txt");
    env.put(&key, b"hello world", "text/plain").await;
    env.get_ops(&format!("/content/{key}")).await;

    let text = env.get_ops("/metrics").await.text().await.unwrap();
    for series in [
        "http_request_duration_seconds_bucket",
        r#"http_requests_total{method="GET",path="/content/{*key}",status="200"}"#,
        "cache_requests_total",
        "http_connections_open",
        "http_connections_total",
    ] {
        assert!(text.contains(series), "missing metric {series}");
    }
}

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn open_connections_are_counted_until_they_close() {
    // In a cluster the pod also gets Kubernetes probes and Prometheus scrapes, which open and
    // close their own connections. Opening 10 and allowing a margin of 2 keeps the test stable.
    const EXTRA: f64 = 10.0;
    const NOISE: f64 = 2.0;
    let env = Env::new().await;
    // Let connections left by earlier tests close first.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let before = env.metric("http_connections_open").await;
    let accepted_before = env.metric("http_connections_total").await;

    // Ten clients, each keeping one keep-alive connection open.
    let mut clients = Vec::new();
    for _ in 0..EXTRA as usize {
        let client = http_client(Client::builder());
        let res = client
            .get(format!("{}/healthz", env.ops))
            .send()
            .await
            .unwrap();
        res.bytes().await.unwrap();
        clients.push(client);
    }
    let while_open = env.metric("http_connections_open").await;
    assert!(
        while_open >= before + EXTRA - NOISE,
        "counted while open: {before} -> {while_open}"
    );
    assert!(env.metric("http_connections_total").await >= accepted_before + EXTRA);

    drop(clients); // closes their idle connections
    let deadline = Instant::now() + Duration::from_secs(5);
    while env.metric("http_connections_open").await > while_open - EXTRA + NOISE {
        assert!(
            Instant::now() < deadline,
            "connections still counted after closing"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
#[ignore = "needs the e2e environment: run with `task e2e`"]
async fn connections_older_than_the_max_age_are_closed_after_a_full_response() {
    let env = Env::new().await;
    let Some(max_age) = env.max_age else {
        eprintln!("skipped: MAX_CONNECTION_AGE_SECS not set");
        return;
    };
    let recycled_before = env.metric("http_connections_recycled_total").await;
    // One keep-alive connection, reused for every request until the server closes it.
    let client = http_client(Client::builder().pool_max_idle_per_host(1));
    let started = Instant::now();
    let closed_after = loop {
        let res = client
            .get(format!("{}/healthz", env.ops))
            .send()
            .await
            .unwrap();
        let close = header(&res, header::CONNECTION) == "close";
        assert_eq!(
            res.text().await.unwrap(),
            "ok",
            "the closing response is complete"
        );
        if close {
            break started.elapsed();
        }
        assert!(
            started.elapsed() < max_age + Duration::from_secs(3),
            "never closed"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    };

    // The deadline is max_age minus 0-20% jitter.
    assert!(
        closed_after >= max_age.mul_f64(0.8) - Duration::from_millis(300),
        "{closed_after:?}"
    );
    assert!(env.metric("http_connections_recycled_total").await > recycled_before);
    let after = client
        .get(format!("{}/healthz", env.ops))
        .send()
        .await
        .unwrap();
    assert_eq!(after.status(), 200, "the client reconnects transparently");
}
