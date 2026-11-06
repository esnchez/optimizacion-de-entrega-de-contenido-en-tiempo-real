//! Runtime configuration, read once from environment variables at startup.

use std::{env, net::SocketAddr, str::FromStr, time::Duration};

#[derive(Debug, Clone)]
pub struct Config {
    /// Address the HTTP server binds to (`BIND_ADDR`, default `0.0.0.0:8080`).
    pub bind_addr: SocketAddr,
    /// Bucket that holds the content (`S3_BUCKET`, required).
    pub s3_bucket: String,
    /// Custom S3 endpoint, e.g. MinIO/LocalStack for local work (`S3_ENDPOINT`).
    pub s3_endpoint: Option<String>,
    /// AWS region override (`AWS_REGION`); otherwise the default provider chain decides.
    pub aws_region: Option<String>,

    /// Maximum requests in flight per pod before load shedding kicks in (`MAX_CONCURRENT_REQUESTS`).
    pub max_concurrent_requests: usize,
    /// Time limit to produce response headers (`REQUEST_TIMEOUT_MS`).
    pub request_timeout: Duration,

    /// TTL of cached object metadata, including "not found" results (`METADATA_CACHE_TTL_SECS`).
    pub metadata_cache_ttl: Duration,
    /// Maximum number of metadata entries (`METADATA_CACHE_CAPACITY`).
    pub metadata_cache_capacity: u64,
    /// Objects up to this size are kept in memory (`SMALL_OBJECT_MAX_BYTES`).
    pub small_object_max_bytes: u64,
    /// Total memory budget for cached object bodies (`BODY_CACHE_MAX_BYTES`).
    pub body_cache_max_bytes: u64,
    /// TTL of cached object bodies (`BODY_CACHE_TTL_SECS`).
    pub body_cache_ttl: Duration,

    /// Lifetime of presigned URLs from `/url/{key}` (`PRESIGN_TTL_SECS`).
    pub presign_ttl: Duration,
    /// Read buffer size when streaming S3 bodies to clients (`STREAM_CHUNK_BYTES`).
    pub stream_chunk_bytes: usize,
    /// `Cache-Control` sent with content so browsers/CDNs can cache it (`CACHE_CONTROL`).
    pub cache_control: String,
    /// Wait after draining before the process exits (`SHUTDOWN_FLUSH_SECS`). Response bytes
    /// already written to a socket are still in the kernel send buffer; if the process (and with
    /// it the container's network) goes away immediately, those bytes never reach the client.
    pub shutdown_flush: Duration,
    /// Connections older than this are closed after their next response so clients reconnect and
    /// spread over all pods, including new ones (`MAX_CONNECTION_AGE_SECS`, `0` disables).
    pub max_connection_age: Option<Duration>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required environment variable {0}")]
    Missing(&'static str),
    #[error("invalid value for {name}: {value:?}")]
    Invalid { name: &'static str, value: String },
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            bind_addr: parse_or("BIND_ADDR", "0.0.0.0:8080".parse().unwrap())?,
            s3_bucket: env::var("S3_BUCKET").map_err(|_| ConfigError::Missing("S3_BUCKET"))?,
            s3_endpoint: optional("S3_ENDPOINT"),
            aws_region: optional("AWS_REGION"),
            max_concurrent_requests: parse_or("MAX_CONCURRENT_REQUESTS", 10_000)?,
            request_timeout: Duration::from_millis(parse_or("REQUEST_TIMEOUT_MS", 10_000)?),
            metadata_cache_ttl: Duration::from_secs(parse_or("METADATA_CACHE_TTL_SECS", 60)?),
            metadata_cache_capacity: parse_or("METADATA_CACHE_CAPACITY", 100_000)?,
            small_object_max_bytes: parse_or("SMALL_OBJECT_MAX_BYTES", 1024 * 1024)?,
            body_cache_max_bytes: parse_or("BODY_CACHE_MAX_BYTES", 256 * 1024 * 1024)?,
            body_cache_ttl: Duration::from_secs(parse_or("BODY_CACHE_TTL_SECS", 60)?),
            presign_ttl: Duration::from_secs(parse_or("PRESIGN_TTL_SECS", 300)?),
            stream_chunk_bytes: parse_or("STREAM_CHUNK_BYTES", 64 * 1024)?,
            cache_control: optional("CACHE_CONTROL")
                .unwrap_or_else(|| "public, max-age=300".to_owned()),
            shutdown_flush: Duration::from_secs(parse_or("SHUTDOWN_FLUSH_SECS", 5)?),
            max_connection_age: match parse_or("MAX_CONNECTION_AGE_SECS", 300)? {
                0 => None,
                secs => Some(Duration::from_secs(secs)),
            },
        })
    }
}

fn optional(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.is_empty())
}

fn parse_or<T: FromStr>(name: &'static str, default: T) -> Result<T, ConfigError> {
    match optional(name) {
        None => Ok(default),
        Some(value) => value
            .parse()
            .map_err(|_| ConfigError::Invalid { name, value }),
    }
}
