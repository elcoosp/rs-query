// src/cancellation.rs
//! Utilities for cooperative cancellation.

use crate::QueryError;
pub use quoin_core::CancellationToken;

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
    // A token cancelled before the call must not race a ready fetch future.
    if token.is_cancelled() {
        return Err(QueryError::cancelled());
    }
    match token.run_until_cancelled(fetch_fn()).await {
        Some(result) => result,
        None => Err(QueryError::cancelled()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cancellable_fetch_resolves_value() {
        let token = CancellationToken::new();
        let result = cancellable_fetch(token, || async { Ok::<_, QueryError>("data") }).await;
        assert_eq!(result, Ok("data"));
    }

    #[tokio::test]
    async fn test_cancellable_fetch_cancelled_before() {
        let token = CancellationToken::new();
        token.cancel();
        let result = cancellable_fetch(token, || async { Ok::<_, QueryError>("data") }).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().is_cancelled());
    }

    #[tokio::test]
    async fn test_cancellable_fetch_cancelled_midway() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let token = CancellationToken::new();
        let token2 = token.clone();
        let started = Arc::new(AtomicBool::new(false));
        let started2 = started.clone();
        let handle = tokio::spawn(async move {
            cancellable_fetch(token2, || async {
                started2.store(true, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                Ok::<_, QueryError>("late")
            })
            .await
        });
        // Wait until the fetch actually started, then cancel it.
        while !started.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        token.cancel();
        let result = handle.await.expect("task should finish");
        assert!(result.is_err());
        assert!(result.unwrap_err().is_cancelled());
    }
}
