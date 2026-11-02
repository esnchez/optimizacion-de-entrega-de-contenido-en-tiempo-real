//! Error type shared by the handlers and its mapping to HTTP responses.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

/// `Clone` because cache loaders share one error between all coalesced waiters.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AppError {
    #[error("object not found")]
    NotFound,
    #[error("requested range not satisfiable")]
    RangeNotSatisfiable,
    #[error("invalid object key")]
    InvalidKey,
    #[error("storage error: {0}")]
    Storage(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::RangeNotSatisfiable => StatusCode::RANGE_NOT_SATISFIABLE,
            AppError::InvalidKey => StatusCode::BAD_REQUEST,
            AppError::Storage(detail) => {
                // Upstream details stay in the logs, never in the response body.
                tracing::error!(error = %detail, "storage backend failure");
                StatusCode::BAD_GATEWAY
            }
        };
        (status, status.canonical_reason().unwrap_or("error")).into_response()
    }
}
