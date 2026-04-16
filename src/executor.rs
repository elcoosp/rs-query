//! GPUI async execution helpers

use crate::{Mutation, MutationState, Query, QueryClient, QueryError, QueryState, RetryConfig, observer::QueryStateVariant};
use futures_timer::Delay;
use std::sync::Arc;
use gpui::Context;
use std::future::Future;

/// Execute a query using GPUI's executor.
pub fn spawn_query<T, V>(
    cx: &mut Context<V>,
    client: &QueryClient,
    query: &Query<T>,
    on_complete: impl FnOnce(&mut V, QueryState<T>, &mut Context<V>) + 'static,
) where
    T: Clone + Send + Sync + std::fmt::Debug + 'static,
    V: 'static,
{
    let key = query.key.clone();
    let fetch_fn = query.fetch_fn.clone();
    let options = query.options.clone();
    let client = client.clone();

    // Check deduplication
    if client.is_in_flight(&key) {
        tracing::trace!(
            target: "rs_query",
            query_key = %key.cache_key(),
            "Query already in flight, skipping duplicate request"
        );
        return;
    }

    tracing::debug!(
        target: "rs_query",
        query_key = %key.cache_key(),
        stale_time_ms = ?options.stale_time.as_millis(),
        max_retries = options.retry.max_retries,
        "Starting query"
    );

    client.set_in_flight(&key, true);
    client.notify_subscribers(key.cache_key(), QueryStateVariant::Loading);

    let retry_config = options.retry.clone();
    let key_for_task = key.cache_key().to_string();

    // Directly spawn the async future – no Tokio runtime needed.
    let task = cx.background_executor().spawn(execute_with_retry(
        fetch_fn,
        retry_config,
        key_for_task,
    ));

    let key_for_cleanup = key.clone();
    let client_for_cleanup = client.clone();

    cx.spawn(async move |this, cx| {
        let result = task.await;
        client_for_cleanup.set_in_flight(&key_for_cleanup, false);

        let state = match result {
            Ok(data) => {
                tracing::debug!(
                    target: "rs_query",
                    query_key = %key_for_cleanup.cache_key(),
                    "Query completed successfully"
                );
                client_for_cleanup.set_query_data(&key_for_cleanup, data.clone(), options);
                client_for_cleanup.notify_subscribers(key_for_cleanup.cache_key(), QueryStateVariant::Success);
                QueryState::Success(data)
            }
            Err(e) => {
                tracing::warn!(
                    target: "rs_query",
                    query_key = %key_for_cleanup.cache_key(),
                    error = %e,
                    "Query failed"
                );
                client_for_cleanup.notify_subscribers(key_for_cleanup.cache_key(), QueryStateVariant::Error);
                QueryState::Error {
                    error: e,
                    stale_data: client_for_cleanup.get_query_data(&key_for_cleanup),
                }
            }
        };

        let _ = this.update(cx, |this, cx| {
            on_complete(this, state, cx);
        });
    })
    .detach();
}

/// Execute a mutation with automatic cache invalidation and optional optimistic updates.
pub fn spawn_mutation<T, P, V>(
    cx: &mut Context<V>,
    client: &QueryClient,
    mutation: &Mutation<T, P>,
    params: P,
    on_complete: impl FnOnce(&mut V, MutationState<T>, &mut Context<V>) + 'static,
) where
    T: Clone + Send + Sync + std::fmt::Debug + 'static,
    P: Clone + Send + std::fmt::Debug + 'static,
    V: 'static,
{
    let mutate_fn = mutation.mutate_fn.clone();
    let invalidate_keys = mutation.invalidate_keys.clone();
    let on_mutate = mutation.on_mutate.clone();
    let client = client.clone();
    let params_for_rollback = params.clone();

    tracing::debug!(
        target: "rs_query",
        invalidate_keys = ?invalidate_keys.iter().map(|k| k.cache_key()).collect::<Vec<_>>(),
        "Starting mutation"
    );

    // Perform optimistic update if callback provided.
    let rollback_context = if let Some(ref on_mutate_cb) = on_mutate {
        match on_mutate_cb(&client, &params) {
            Ok(ctx) => Some(ctx),
            Err(e) => {
                tracing::warn!(
                    target: "rs_query",
                    error = %e,
                    "Optimistic update failed, mutation will proceed without optimistic data"
                );
                None
            }
        }
    } else {
        None
    };

    let task = cx
        .background_executor()
        .spawn(async move { (mutate_fn)(params).await });

    cx.spawn(async move |this, cx| {
        let result = task.await;

        let state = match result {
            Ok(data) => {
                tracing::debug!(
                    target: "rs_query",
                    invalidated_count = invalidate_keys.len(),
                    "Mutation completed successfully"
                );
                // Invalidate queries
                for key in &invalidate_keys {
                    client.invalidate_queries(key);
                }
                MutationState::Success(data)
            }
            Err(e) => {
                tracing::warn!(
                    target: "rs_query",
                    error = %e,
                    "Mutation failed"
                );
                // Rollback optimistic update if any
                if let Some(rollback) = rollback_context {
                    rollback(&client);
                }
                MutationState::Error(e)
            }
        };

        let _ = this.update(cx, |this, cx| {
            on_complete(this, state, cx);
        });
    })
    .detach();
}

/// Execute with retry logic, using futures_timer::Delay for backoff.
async fn execute_with_retry<T>(
    fetch_fn: Arc<dyn Fn() -> std::pin::Pin<Box<dyn Future<Output = Result<T, QueryError>> + Send>> + Send + Sync>,
    retry_config: RetryConfig,
    query_key: String,
) -> Result<T, QueryError>
where
    T: Clone + Send + Sync + 'static,
{
    let mut attempts = 0;
    let mut last_error = None;

    while attempts <= retry_config.max_retries {
        match (fetch_fn)().await {
            Ok(data) => {
                if attempts > 0 {
                    tracing::debug!(
                        target: "rs_query",
                        query_key = %query_key,
                        attempts = attempts + 1,
                        "Query succeeded after retry"
                    );
                }
                return Ok(data);
            }
            Err(e) => {
                if !e.is_retryable() || attempts >= retry_config.max_retries {
                    if attempts > 0 {
                        tracing::debug!(
                            target: "rs_query",
                            query_key = %query_key,
                            attempts = attempts + 1,
                            error = %e,
                            "Query failed after all retries"
                        );
                    }
                    return Err(e);
                }

                attempts += 1;
                let delay = std::cmp::min(
                    retry_config.base_delay * 2u32.pow(attempts - 1),
                    retry_config.max_delay,
                );

                tracing::debug!(
                    target: "rs_query",
                    query_key = %query_key,
                    attempt = attempts,
                    max_retries = retry_config.max_retries,
                    delay_ms = delay.as_millis(),
                    error = %e,
                    "Retrying query after error"
                );

                last_error = Some(e);
                Delay::new(delay).await;
            }
        }
    }

    Err(last_error.unwrap_or(QueryError::Custom("Max retries exceeded".into())))
}
