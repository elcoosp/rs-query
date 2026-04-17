// src/cancellation.rs
//! Cancellation helpers for cooperative query abort.

use crate::QueryError;
use tokio_util::sync::CancellationToken;

/// Wrap a fetch function so it can be cancelled via a [`CancellationToken`].
///
/// Uses `tokio::select!` to race the fetch against the cancellation signal.
/// If the token is cancelled before the fetch completes, returns
/// [`QueryError::Cancelled`].
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_cancellable_fetch_completes_normally() {
        let token = CancellationToken::new();
        let result = cancellable_fetch(token, || async { Ok("data".to_string()) }).await;
        assert_eq!(result.unwrap(), "data");
    }

    #[tokio::test]
    async fn test_cancellable_fetch_returns_cancelled() {
        let token = CancellationToken::new();
        let token_clone = token.clone();

        let handle = tokio::spawn(async move {
            cancellable_fetch(token, || async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                Ok("late".to_string())
            })
            .await
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        token_clone.cancel();

        let result = handle.await.unwrap();
        assert!(matches!(result, Err(QueryError::Cancelled)));
    }

    #[tokio::test]
    async fn test_cancellable_fetch_propagates_error() {
        let token = CancellationToken::new();
        let result = cancellable_fetch(token, || async {
            Err(QueryError::Message("fail".to_string()))
        })
        .await;
        assert!(matches!(result, Err(QueryError::Message(_))));
    }
}
