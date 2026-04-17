// src/cancellation.rs
//! Utilities for cooperative cancellation.

use crate::QueryError;
use tokio_util::sync::CancellationToken;

/// Run a fetch function with cancellation support.
pub async fn cancellable_fetch<T, F, Fut>(
    token: CancellationToken,
    fetch_fn: F,
) -> Result<T, QueryError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, QueryError>>,
{
    tokio::select! {
        _ = token.cancelled() => Err(QueryError::Cancelled),
        result = fetch_fn() => result,
    }
}
