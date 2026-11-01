// src/cancellation.rs
//! Utilities for cooperative cancellation.

use crate::QueryError;
use tokio_util::sync::CancellationToken;

/// Run a fetch function with cancellation support.
///
/// Resolves with the fetch result, or with [`QueryError::cancelled`] when
/// `token` is cancelled before the fetch completes. The in-flight future is
/// dropped at the cancellation point, so the underlying work must be
/// cancellation-safe.
///
/// # Errors
///
/// Returns [`QueryError::cancelled`] if the token is cancelled; otherwise the
/// fetch function's own error is propagated unchanged.
pub async fn cancellable_fetch<T, F, Fut>(
    token: CancellationToken,
    fetch_fn: F,
) -> Result<T, QueryError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, QueryError>>,
{
    tokio::select! {
        () = token.cancelled() => Err(QueryError::cancelled()),
        result = fetch_fn() => result,
    }
}
