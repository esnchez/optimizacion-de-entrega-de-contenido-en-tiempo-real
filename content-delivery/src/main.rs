//! Content delivery service: a scalable HTTP server that connects users to S3 objects.
//!
//! See `docs/` at the repository root for the architecture and the crates used.

mod config;
mod connections;
mod content;
mod error;
mod routes;
mod state;
mod storage;
mod telemetry;

use axum::serve::ListenerExt;
use tokio::net::TcpListener;

use crate::{
    config::Config,
    connections::{ConnectionInfo, CountedListener},
    content::ContentService,
    state::AppState,
    storage::S3Store,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    telemetry::init_tracing();
    let metrics = telemetry::init_metrics()?;
    let config = Config::from_env()?;
    tracing::info!(?config, "starting content delivery service");

    let store = S3Store::from_config(&config).await;
    let content = ContentService::new(store, &config);
    let state = AppState::new(config, content, metrics);
    let app = routes::router(state.clone());

    let listener = TcpListener::bind(state.config.bind_addr).await?;
    // Disable Nagle's algorithm: small responses and first bytes go out without waiting.
    let listener = listener.tap_io(|tcp| {
        if let Err(err) = tcp.set_nodelay(true) {
            tracing::warn!(%err, "failed to set TCP_NODELAY");
        }
    });
    let listener = CountedListener::new(listener, state.config.max_connection_age);
    tracing::info!(addr = %state.config.bind_addr, "listening");

    let flush = state.config.shutdown_flush;
    // ConnectInfo carries each connection's recycle deadline to the requests made on it.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<ConnectionInfo>(),
    )
    .with_graceful_shutdown(shutdown_signal(state))
    .await?;
    // Every response has been fully written, but the tail of the larger ones may still sit in
    // kernel send buffers. Give slow clients time to receive it before the process exits.
    tracing::info!(?flush, "all connections drained, flushing socket buffers");
    tokio::time::sleep(flush).await;
    tracing::info!("server stopped");
    Ok(())
}

/// Waits for Ctrl+C or SIGTERM (what Kubernetes sends), then marks the pod not ready. axum then
/// stops accepting connections and waits for in-flight requests to finish.
async fn shutdown_signal(state: AppState) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    state.set_ready(false);
    tracing::info!("shutdown signal received, draining connections");
}
