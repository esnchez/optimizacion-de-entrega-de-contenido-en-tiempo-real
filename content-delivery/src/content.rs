//! Content service: puts in-memory caches with request coalescing in front of S3.
//!
//! Many users usually ask for the same few "hot" objects at the same time. Without a cache every
//! request becomes an S3 call; here concurrent misses for one key share a single S3 request
//! (moka's `or_try_insert_with` coalesces them) and later hits never leave the process.

use std::sync::Arc;

use bytes::Bytes;
use moka::future::Cache;

use crate::{
    config::Config,
    error::AppError,
    storage::{ObjectMeta, ObjectStream, S3Store},
};

pub struct ContentService {
    store: S3Store,
    /// key -> metadata. `None` caches "not found" so 404 floods don't reach S3.
    metadata: Cache<String, Option<Arc<ObjectMeta>>>,
    /// "key + ETag" -> full body of a small object, bounded by total bytes.
    bodies: Cache<String, Bytes>,
    small_object_max_bytes: u64,
}

impl ContentService {
    pub fn new(store: S3Store, cfg: &Config) -> Self {
        let metadata = Cache::builder()
            .name("object-metadata")
            .max_capacity(cfg.metadata_cache_capacity)
            .time_to_live(cfg.metadata_cache_ttl)
            .build();

        let bodies = Cache::builder()
            .name("object-bodies")
            // Capacity is measured in bytes thanks to the weigher.
            .max_capacity(cfg.body_cache_max_bytes)
            .weigher(|_key: &String, body: &Bytes| u32::try_from(body.len()).unwrap_or(u32::MAX))
            .time_to_live(cfg.body_cache_ttl)
            .build();

        Self {
            store,
            metadata,
            bodies,
            small_object_max_bytes: cfg.small_object_max_bytes,
        }
    }

    pub fn store(&self) -> &S3Store {
        &self.store
    }

    /// Cached `HeadObject`; concurrent misses for the same key share one S3 call.
    pub async fn metadata(&self, key: &str) -> Result<Option<Arc<ObjectMeta>>, AppError> {
        let store = &self.store;
        let entry = self
            .metadata
            .entry_by_ref(key)
            .or_try_insert_with(async { Ok::<_, AppError>(store.head(key).await?.map(Arc::new)) })
            .await
            .map_err(|err| (*err).clone())?;
        record_cache("metadata", entry.is_fresh());
        Ok(entry.into_value())
    }

    /// Whether this object is served from memory instead of streamed from S3.
    pub fn is_small(&self, meta: &ObjectMeta) -> bool {
        meta.size <= self.small_object_max_bytes
    }

    /// Full body of a small object, from memory or loaded once from S3.
    pub async fn small_body(&self, key: &str, meta: &ObjectMeta) -> Result<Bytes, AppError> {
        // Including the ETag means a changed object never serves a stale body.
        let cache_key = format!("{key}\0{}", meta.etag.as_deref().unwrap_or_default());
        let store = &self.store;
        let entry = self
            .bodies
            .entry(cache_key)
            .or_try_insert_with(store.get_bytes(key))
            .await
            .map_err(|err| (*err).clone())?;
        record_cache("body", entry.is_fresh());
        Ok(entry.into_value())
    }

    /// Streams (part of) an object straight from S3 without buffering it.
    pub async fn stream(&self, key: &str, range: Option<&str>) -> Result<ObjectStream, AppError> {
        self.store.get(key, range).await
    }
}

fn record_cache(cache: &'static str, loaded: bool) {
    let result = if loaded { "miss" } else { "hit" };
    metrics::counter!("cache_requests_total", "cache" => cache, "result" => result).increment(1);
}
