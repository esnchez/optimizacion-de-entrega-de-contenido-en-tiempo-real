//! State shared by every request handler.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use metrics_exporter_prometheus::PrometheusHandle;

use crate::{config::Config, content::ContentService};

/// Cheap to clone: every field is an `Arc` (or wraps one), so cloning per request is just a
/// reference-count bump. Nothing in here needs a lock.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub content: Arc<ContentService>,
    pub metrics: PrometheusHandle,
    /// Flipped to `false` on shutdown so Kubernetes stops sending new traffic.
    ready: Arc<AtomicBool>,
}

impl AppState {
    pub fn new(config: Config, content: ContentService, metrics: PrometheusHandle) -> Self {
        Self {
            config: Arc::new(config),
            content: Arc::new(content),
            metrics,
            ready: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::Relaxed);
    }
}
