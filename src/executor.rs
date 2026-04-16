//! GPUI async execution helpers

use crate::observer::QueryStateVariant;
use crate::{
    Mutation, MutationState, Query, QueryClient, QueryError, QueryState, RetryConfig,
};
use futures_timer::Delay;
use gpui::Context;
use std::future::Future;
use std::sync::Arc;
use tokio::task::AbortHandle;

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

    // Populate initial data if cache empty
    if client.get_query_data::<T>(&key).is_none() {
        if let Some(initial) = &options.initial_data {
            if let Some(data) = initial.downcast_ref::<T>() {
                client.set_query_data(&key, data.clone(), options.clone());
            }
        } else if let Some(initial_fn) = &options.initial_data_fn {
            let arc_data = (initial_fn)();
            if let Some(data) = arc_data.downcast_ref::<T>() {
                client.set_query_data(&key, data.clone(), options.clone());
            }
        }
    }

    // Deduplication: if already in flight, skip
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

    client.notify_subscribers(key.cache_key(), QueryStateVariant::Loading);

    let retry_config = options.retry.clone();
    let key_for_task = key.cache_key().to_string();

    // Spawn a tokio task to get an AbortHandle
    let task = tokio::spawn(execute_with_retry(
        fetch_fn,
        retry_config,
        key_for_task.clone(),
    ));
    let abort_handle = task.abort_handle();
    client.set_abort_handle(&key, abort_handle);

    // Bridge to GPUI executor for awaiting
    let gpui_task = cx.background_executor().spawn(task);

    let key_for_cleanup = key.clone();
    let client_for_cleanup = client.clone();
    let select = options.select.clone();

    cx.spawn(async move |this, cx| {
        let result = gpui_task.await;
        client_for_cleanup.clear_abort_handle(&key_for_cleanup);

        let state = match result {
            Ok(Ok(data)) => {
                tracing::debug!(
                    target: "rs_query",
                    query_key = %key_for_cleanup.cache_key(),
                    "Query completed successfully"
                );
                let final_data = if let Some(select_fn) = &select {
                    let arc_data: Arc<dyn std::any::Any + Send + Sync> = Arc::new(data);
                    let transformed = select_fn(&*arc_data);
                    transformed
                        .downcast_ref::<T>()
                        .expect("select returned wrong type")
                        .clone()
                } else {
                    data
                };
                client_for_cleanup.set_query_data(&key_for_cleanup, final_data.clone(), options);
                client_for_cleanup.notify_subscribers(key_for_cleanup.cache_key(), QueryStateVariant::Success);
                QueryState::Success(final_data)
            }
            Ok(Err(e)) | Err(tokio::task::JoinError::from(e)) => {
                // Task was either cancelled or returned an error
                let error = match e {
                    tokio::task::JoinError::from(e) => QueryError::Custom(format!("Task cancelled: {}", e)),
                    _ => e,
                };
                tracing::warn!(
                    target: "rs_query",
                    query_key = %key_for_cleanup.cache_key(),
                    error = %error,
                    "Query failed"
                );
                client_for_cleanup.notify_subscribers(key_for_cleanup.cache_key(), QueryStateVariant::Error);
                QueryState::Error {
                    error,
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

/// Execute a mutation (unchanged cancellation-wise for now).
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
    let _params_for_rollback = params.clone();

    tracing::debug!(
        target: "rs_query",
        invalidate_keys = ?invalidate_keys.iter().map(|k| k.cache_key()).collect::<Vec<_>>(),
        "Starting mutation"
    );

    let rollback_context = if let Some(ref on_mutate_cb) = on_mutate {
        match on_mutate_cb(&client, &params) {
            Ok(ctx) => Some(ctx),
            Err(e) => {
                tracing::warn!(target: "rs_query", error = %e, "Optimistic update failed");
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
                for key in &invalidate_keys {
                    client.invalidate_queries(key);
                }
                MutationState::Success(data)
            }
            Err(e) => {
                tracing::warn!(target: "rs_query", error = %e, "Mutation failed");
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

/// Execute with retry logic, checking for cancellation.
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
        // Check for cancellation before each attempt
        if tokio::task::yield_now().await; // Not directly, but we can check a global cancellation token if needed. For simplicity, rely on abort handle.

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
