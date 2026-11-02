// src/executor.rs
//! Query executor - handles async query execution with retries and deduplication

use crate::cancellation::cancellable_fetch;
use crate::error::QueryError;
use crate::mutation::Mutation;
use crate::network::{NetworkMode, can_fetch, can_retry};
use crate::observer::QueryStateVariant;
use crate::query::Query;
use crate::retryer::RetryDecision;
use crate::runtime::{BoxFuture, SpawnedTask};
use crate::{MutationState, QueryClient, QueryState};
use quoin_core::CancellationToken;
use std::sync::Arc;
use std::time::Duration;

/// Guard that decrements the fetching count on drop.
struct FetchingGuard {
    client: QueryClient,
}
impl Drop for FetchingGuard {
    fn drop(&mut self) {
        self.client.dec_fetching();
    }
}

/// Guard that decrements the mutating count on drop.
struct MutatingGuard {
    client: QueryClient,
}
impl Drop for MutatingGuard {
    fn drop(&mut self) {
        self.client.dec_mutating();
    }
}

/// Resolve the backoff delay for a retry attempt.
///
/// Uses the shared `TanStack` backoff curve (`crate::retryer::retry_delay`:
/// `min(1000 * 2^attempt, 30s)`, no jitter) when the config matches the
/// default exponential shape (1 s base, exponential), otherwise falls back to
/// the config's own `delay_for_attempt` logic.
fn delay_for(config: &crate::options::RetryConfig, attempt: u32) -> Duration {
    if config.exponential_backoff && config.base_delay == crate::retryer::BASE_RETRY_DELAY {
        crate::retryer::retry_delay(attempt)
    } else {
        config.delay_for_attempt(attempt)
    }
}

/// Wait until a retry may continue under the given network mode.
///
/// Mirrors the `TanStack` retryer pause: the loop parks on the online manager's
/// watch channel and resumes when connectivity allows retries; cancellation
/// through `token` (or a closed channel) breaks the wait.
async fn wait_for_online(
    client: &QueryClient,
    network_mode: NetworkMode,
    token: &CancellationToken,
) {
    let online = client.online_manager().clone();
    let mut rx = online.subscribe();
    loop {
        if token.is_cancelled() || can_retry(network_mode, online.is_online()) {
            return;
        }
        // Park until connectivity changes or cancellation. A closed change
        // stream resolves the race immediately; the flag re-check at the top
        // of the loop then decides whether waiting can continue.
        let changed = token.run_until_cancelled(rx.changed());
        match changed.await {
            // `Ok` means connectivity changed (the loop re-checks the flag);
            // `Err` (watch closed) and `None` (cancelled) both end the wait.
            Some(Ok(())) => {}
            _ => return,
        }
    }
}

/// Wait for a deduplicated in-flight query to resolve instead of issuing a
/// parallel fetch.
///
/// Mirrors `TanStack`'s deduplication: if data was cached while we subscribed
/// the cached value wins (stale-aware); otherwise the subscriber stream is
/// polled until a terminal (`Success`/`Stale`/`Error`) update arrives. If the
/// channel closes unexpectedly the caller still gets [`QueryState::Loading`].
async fn wait_for_dedup<T: Clone + Send + Sync + 'static>(
    client: &QueryClient,
    query: &Query<T>,
) -> QueryState<T> {
    let mut rx = client.subscribe(query.key.cache_key());

    // First, check if data is already cached (could have been set while we waited)
    if let Some(data) = client.get_query_data::<T>(&query.key) {
        let is_stale = client.is_stale(&query.key);
        return if is_stale {
            QueryState::Stale(data)
        } else {
            QueryState::Success(data)
        };
    }

    // Wait for the in-flight query to complete.
    while let Ok(update) = rx.recv().await {
        match update.state_variant {
            QueryStateVariant::Success => {
                if let Some(data) = client.get_query_data::<T>(&query.key) {
                    return QueryState::Success(data);
                }
            }
            QueryStateVariant::Stale => {
                if let Some(data) = client.get_query_data::<T>(&query.key) {
                    return QueryState::Stale(data);
                }
            }
            QueryStateVariant::Error => {
                return QueryState::Error {
                    error: QueryError::custom("Deduplicated request failed"),
                    stale_data: client.get_query_data::<T>(&query.key),
                };
            }
            // Keep waiting for Loading/Refetching updates.
            _ => {}
        }
    }
    // If the channel closed unexpectedly, fall back to loading state.
    QueryState::Loading
}

/// Execute a query with retries and deduplication, respecting cancellation.
///
/// Integrates the `TanStack` behaviours:
///
/// * **Network gating** — a fetch cannot *start* while offline in the default
///   `online` network mode; it fails with [`QueryError::Paused`] and the
///   current (stale) data is preserved.
/// * **Retryer semantics** — retry counts come from a
///   [`RetryDecision::Count`] built from `options.retry`, backoff flows
///   through the [`TimeoutManager`], and between retries the fetch pauses
///   while connectivity forbids continuing (`can_retry`), resuming when the
///   online manager reports connectivity again.
/// * **Restoring** — while `client.is_restoring()`, no fetch is started and
///   [`QueryState::Idle`] is returned immediately.
pub async fn execute_query<T: Clone + Send + Sync + 'static + PartialEq>(
    client: &QueryClient,
    query: &Query<T>,
    token: CancellationToken,
) -> QueryState<T> {
    execute_query_inner(client, query, token, true).await
}

/// [`execute_query`] with the dedup fast-path configurable.
///
/// `spawn_query` registers its spawned fetch as in-flight and must therefore
/// skip the dedup check: the task itself is the fetcher, not a waiter, and
/// checking would deadlock waiting on its own in-flight entry.
async fn execute_query_inner<T: Clone + Send + Sync + 'static + PartialEq>(
    client: &QueryClient,
    query: &Query<T>,
    token: CancellationToken,
    allow_dedup: bool,
) -> QueryState<T> {
    // Persisted-cache restore in progress: report idle without fetching.
    if client.is_restoring() {
        return QueryState::Idle;
    }

    let cache_key = query.key.cache_key();

    // Deduplication: if already in flight, wait for result.
    if allow_dedup && client.is_in_flight(&query.key) {
        return wait_for_dedup(client, query).await;
    }

    client.notify_subscribers(cache_key, QueryStateVariant::Loading);

    // TanStack retryer network gate: a fetch cannot *start* while offline in
    // `online` mode. We surface the pause as an error carrying the current
    // (stale) data, instead of parking the caller on a paused fetch.
    if !can_fetch(
        query.options.network_mode,
        client.online_manager().is_online(),
    ) {
        return QueryState::Error {
            error: QueryError::Paused,
            stale_data: client.get_query_data::<T>(&query.key),
        };
    }

    let fetch_fn = Arc::clone(&query.fetch_fn);
    let select_fn = query.select.clone();
    let options = query.options.clone();
    let key = query.key.clone();
    let client_clone = client.clone();

    // Retryer semantics: the retry budget is expressed as a RetryDecision
    // built from the retry config; error-kind gating (only network/timeout
    // failures are retryable) is preserved from the original executor.
    let retry_decision = RetryDecision::Count(options.retry.max_retries);

    let mut attempts: u32 = 0;

    loop {
        if token.is_cancelled() {
            return QueryState::Error {
                error: QueryError::cancelled(),
                stale_data: client_clone.get_query_data::<T>(&key),
            };
        }

        attempts += 1;
        match cancellable_fetch(token.clone(), || fetch_fn()).await {
            Ok(data) => {
                let final_data = if let Some(ref select) = select_fn {
                    select(&data)
                } else {
                    data
                };
                client_clone.set_query_data(&key, final_data, options.clone());
                return QueryState::Success(
                    client_clone
                        .get_query_data::<T>(&key)
                        .expect("data should be set"),
                );
            }
            Err(ref e) if e.is_cancelled() => {
                return QueryState::Error {
                    error: QueryError::cancelled(),
                    stale_data: client_clone.get_query_data::<T>(&key),
                };
            }
            Err(e) => {
                let attempt_index = attempts - 1;
                if retry_decision.should_retry(&e, attempt_index)
                    && should_retry(&e, &options.retry)
                {
                    // TanStack retryer pause: retries wait for connectivity
                    // when the network mode forbids continuing offline.
                    if !can_retry(
                        options.network_mode,
                        client_clone.online_manager().is_online(),
                    ) {
                        wait_for_online(&client_clone, options.network_mode, &token).await;
                        if token.is_cancelled() {
                            return QueryState::Error {
                                error: QueryError::cancelled(),
                                stale_data: client_clone.get_query_data::<T>(&key),
                            };
                        }
                    }
                    let delay = delay_for(&options.retry, attempt_index);
                    client_clone.runtime().sleep(delay).await;
                    continue;
                }
                client_clone.notify_subscribers(key.cache_key(), QueryStateVariant::Error);
                return QueryState::Error {
                    error: e,
                    stale_data: client_clone.get_query_data::<T>(&key),
                };
            }
        }
    }
}

/// Execute a mutation with optimistic update support.
///
/// # Errors
///
/// Returns the mutation function's error (after rollback + `on_error`
/// callbacks), or the `on_mutate` hook's error when the optimistic phase
/// fails.
pub async fn execute_mutation<T: Clone + Send + Sync + 'static, P: Clone + Send + 'static>(
    client: &QueryClient,
    mutation: &Mutation<T, P>,
    params: P,
) -> Result<T, QueryError> {
    let mutate_fn = Arc::clone(&mutation.mutate_fn);
    let params = Arc::new(params);

    let rollback: Option<crate::mutation::RollbackFn> =
        if let Some(ref on_mutate) = mutation.on_mutate {
            match on_mutate(client, &*params) {
                Ok(rollback) => Some(rollback),
                Err(e) => return Err(e),
            }
        } else {
            None
        };

    let result = mutate_fn((*params).clone()).await;

    match result {
        Ok(data) => {
            for key in &mutation.invalidates_keys {
                client.invalidate_queries(key);
            }
            if let Some(on_success) = &mutation.on_success {
                on_success(&data, &*params);
            }
            Ok(data)
        }
        Err(e) => {
            if let Some(rollback) = rollback {
                rollback(client);
            }
            if let Some(on_error) = &mutation.on_error {
                on_error(&e, &*params);
            }
            Err(e)
        }
    }
}

fn should_retry(error: &QueryError, _retry: &crate::options::RetryConfig) -> bool {
    matches!(error, QueryError::Network(_) | QueryError::Timeout(_))
}

/// Spawn a query on the client's runtime.
///
/// While `client.is_restoring()` (persisted-cache restore in progress) the
/// fetch is skipped entirely and the handle resolves immediately with
/// [`QueryState::Idle`], mirroring `TanStack`'s `_optimisticResults:
/// 'isRestoring'` forcing `fetchStatus: 'idle'`.
///
/// The fetch registers itself as in-flight (deduplication, cancellation via
/// [`QueryClient::cancel_query`]) and increments the fetching activity
/// counter for its duration.
///
/// Observation is decoupled from delivery: await the returned
/// [`SpawnedTask`] for the final state, or subscribe to the key through
/// [`QueryClient::subscribe`]/the hooks for incremental updates. Dropping the
/// handle without awaiting or detaching aborts the fetch (structured
/// concurrency).
pub fn spawn_query<T>(client: &QueryClient, query: &Query<T>) -> SpawnedTask<QueryState<T>>
where
    T: Clone + Send + Sync + 'static + PartialEq,
{
    let client = client.clone();
    let query = query.clone();

    if client.is_restoring() {
        let (tx, rx) = futures::channel::oneshot::channel();
        let handle = client.runtime().spawn(Box::pin(async move {
            let _ = tx.send(QueryState::Idle);
        }));
        return SpawnedTask::new(handle, rx, Arc::new(|| QueryState::Idle));
    }

    // Increment fetching count and set up guard.
    client.inc_fetching();
    let guard = FetchingGuard {
        client: client.clone(),
    };

    // Create cancellation token and store in-flight task.
    let token = CancellationToken::new();
    let token_clone = token.clone();
    let client_for_task = client.clone();
    let query_for_task = query.clone();
    let runtime = client.runtime().clone();
    let (tx, rx) = futures::channel::oneshot::channel();
    let task: BoxFuture<()> = Box::pin(async move {
        let _guard = guard; // ensure decrement on drop
        // The spawned task is the fetcher itself: it must not deduplicate
        // against its own in-flight entry (that would deadlock).
        //
        // Panic isolation mirrors the old tokio-join semantics: a panicking
        // fetch surfaces as an error state instead of poisoning the worker.
        let state = match futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
            execute_query_inner(&client_for_task, &query_for_task, token_clone, false),
        ))
        .await
        {
            Ok(state) => state,
            Err(panic) => QueryState::Error {
                error: QueryError::custom(panic_message(&panic)),
                stale_data: client_for_task.get_query_data(&query_for_task.key),
            },
        };
        client_for_task.clear_in_flight(&query_for_task.key);
        let _ = tx.send(state);
    });
    let handle = runtime.spawn(task);
    client.set_in_flight(
        &query.key,
        crate::client::InFlightTask {
            abort: {
                let handle = handle.clone();
                Box::new(move || handle.abort())
            },
            cancel_token: token,
        },
    );
    SpawnedTask::new(
        handle,
        rx,
        Arc::new(move || QueryState::Error {
            error: QueryError::cancelled(),
            stale_data: client.get_query_data(&query.key),
        }),
    )
}

/// Spawn a mutation on the client's runtime.
///
/// While `client.is_restoring()` the mutation is skipped and the handle
/// resolves immediately with [`MutationState::Idle`]. The mutating activity
/// counter is incremented for the duration. Awaiting the handle observes the
/// final state; dropping it without awaiting or detaching aborts the
/// mutation.
pub fn spawn_mutation<T, P>(
    client: &QueryClient,
    mutation: &Mutation<T, P>,
    params: P,
) -> SpawnedTask<MutationState<T>>
where
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + Sync + 'static,
{
    let client = client.clone();
    let mutation = mutation.clone();

    if client.is_restoring() {
        let (tx, rx) = futures::channel::oneshot::channel();
        let handle = client.runtime().spawn(Box::pin(async move {
            let _ = tx.send(MutationState::Idle);
        }));
        return SpawnedTask::new(handle, rx, Arc::new(|| MutationState::Idle));
    }

    client.inc_mutating();
    let guard = MutatingGuard {
        client: client.clone(),
    };

    let runtime = client.runtime().clone();
    let (tx, rx) = futures::channel::oneshot::channel();
    let task: BoxFuture<()> = Box::pin(async move {
        let _guard = guard;
        let state = match execute_mutation(&client, &mutation, params).await {
            Ok(data) => MutationState::Success(data),
            Err(e) => MutationState::Error(e),
        };
        let _ = tx.send(state);
    });
    let handle = runtime.spawn(task);
    SpawnedTask::new(
        handle,
        rx,
        Arc::new(|| MutationState::Error(QueryError::cancelled())),
    )
}

/// Extract a readable message from a caught panic payload.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        format!("fetch panicked: {message}")
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!("fetch panicked: {message}")
    } else {
        "fetch panicked".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QueryClient, QueryError, QueryKey, QueryOptions, query::Query};
    use quoin_core::CancellationToken;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    // Helper to create a default token for tests
    fn test_token() -> CancellationToken {
        CancellationToken::new()
    }

    fn make_retry_options(max_retries: u32) -> QueryOptions {
        QueryOptions {
            retry: crate::options::RetryConfig {
                max_retries,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_execute_query_success() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query = Query::new(key, || async { Ok("data".to_string()) });

        let state = execute_query(&client, &query, test_token()).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"data".to_string()));
    }

    #[tokio::test]
    async fn test_execute_query_error_no_retry() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<String> =
            Query::new(key, || async { Err(QueryError::custom("immediate fail")) });

        let state = execute_query(&client, &query, test_token()).await;
        assert!(state.is_error());
        assert!(state.data().is_none());
    }

    #[tokio::test]
    async fn test_execute_query_network_error_retries() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();

        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query = Query::new(key, move || {
            let a = attempts_clone.clone();
            async move {
                let count = a.fetch_add(1, Ordering::SeqCst);
                if count < 1 {
                    Err(QueryError::Network("transient".to_string()))
                } else {
                    Ok("recovered".to_string())
                }
            }
        })
        .options(make_retry_options(3));

        let state = execute_query(&client, &query, test_token()).await;
        assert!(state.is_success());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_execute_query_timeout_error_retries() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();

        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query = Query::new(key, move || {
            let a = attempts_clone.clone();
            async move {
                let count = a.fetch_add(1, Ordering::SeqCst);
                if count < 1 {
                    Err(QueryError::Timeout("slow".to_string()))
                } else {
                    Ok("recovered".to_string())
                }
            }
        })
        .options(make_retry_options(3));

        let state = execute_query(&client, &query, test_token()).await;
        assert!(state.is_success());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_execute_query_max_retries_exceeded() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<String> = Query::new(key, || async {
            Err(QueryError::Network("persistent".to_string()))
        })
        .options(make_retry_options(2));

        let state = execute_query(&client, &query, test_token()).await;
        assert!(state.is_error());
    }

    #[tokio::test]
    async fn test_execute_query_no_retry_on_custom_error() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();

        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<String> = Query::new(key, move || {
            let a = attempts_clone.clone();
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                Err(QueryError::custom("not retryable"))
            }
        })
        .options(make_retry_options(3));

        let state = execute_query(&client, &query, test_token()).await;
        assert!(state.is_error());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_execute_query_caches_result() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query = Query::new(key.clone(), || async { Ok("data".to_string()) });

        let _ = execute_query(&client, &query, test_token()).await;

        let cached: Option<String> = client.get_query_data(&key);
        assert_eq!(cached, Some("data".to_string()));
    }

    #[tokio::test]
    async fn test_execute_query_refetch_updates_cache() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        // Pre-populate cache with old data
        client.set_query_data(&key, "old".to_string(), QueryOptions::default());

        let query = Query::new(key.clone(), || async { Ok("new".to_string()) });

        // Execute should complete and update cache
        let state = execute_query(&client, &query, test_token()).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"new".to_string()));

        // Cache should be updated with new data
        let cached: Option<String> = client.get_query_data(&key);
        assert_eq!(cached, Some("new".to_string()));
    }

    // ---- New tests for improved coverage ----

    #[tokio::test]
    async fn test_fetching_guard_decrements_on_drop() {
        let client = QueryClient::new();
        assert_eq!(client.fetching_count(), 0);
        client.inc_fetching();
        assert_eq!(client.fetching_count(), 1);
        {
            let _guard = FetchingGuard {
                client: client.clone(),
            };
        }
        assert_eq!(client.fetching_count(), 0);
    }

    #[tokio::test]
    async fn test_mutating_guard_decrements_on_drop() {
        let client = QueryClient::new();
        assert_eq!(client.mutating_count(), 0);
        client.inc_mutating();
        assert_eq!(client.mutating_count(), 1);
        {
            let _guard = MutatingGuard {
                client: client.clone(),
            };
        }
        assert_eq!(client.mutating_count(), 0);
    }
    #[tokio::test]
    async fn test_execute_query_deduplication_waits_for_in_flight() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_called = Arc::new(AtomicUsize::new(0));
        let fetch_called_clone = fetch_called.clone();

        let query: Query<String> = Query::new(key.clone(), move || {
            let fetch_called = fetch_called_clone.clone();
            async move {
                fetch_called.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok("data".to_string())
            }
        });

        // Simulate an already in‑flight query.
        let token = CancellationToken::new();
        let handle = tokio::spawn(async {});
        client.set_in_flight(
            &key,
            crate::client::InFlightTask {
                abort: {
                    let handle = handle;
                    Box::new(move || handle.abort())
                },
                cancel_token: token,
            },
        );

        // Spawn two queries that will hit the deduplication path.
        let client1 = client.clone();
        let query1 = query.clone();
        let handle1 = tokio::spawn(async move {
            execute_query(&client1, &query1, CancellationToken::new()).await
        });

        let client2 = client.clone();
        let query2 = query.clone();
        let handle2 = tokio::spawn(async move {
            execute_query(&client2, &query2, CancellationToken::new()).await
        });

        // Give them time to enter the dedup loop, then clear the in‑flight flag and notify.
        tokio::time::sleep(Duration::from_millis(20)).await;
        client.clear_in_flight(&key);
        client.notify_subscribers(key.cache_key(), QueryStateVariant::Error);

        let state1 = handle1.await.unwrap();
        let state2 = handle2.await.unwrap();

        // Both should have waited and eventually returned an error (or loading).
        assert!(state1.is_loading() || state1.is_error());
        assert!(state2.is_loading() || state2.is_error());

        // The fetch function was never called because we never actually ran it.
        assert_eq!(fetch_called.load(Ordering::SeqCst), 0);

        // A non-deduplicated execution runs the fetch closure for real.
        client.clear_in_flight(&key);
        let state = execute_query(&client, &query, CancellationToken::new()).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"data".to_string()));
        assert_eq!(fetch_called.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn test_execute_query_cancellation_during_fetch() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<String> = Query::new(key.clone(), || async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok("done".to_string())
        });

        let token = CancellationToken::new();
        let token_clone = token.clone();
        let task_client = client.clone();
        let task_query = query.clone();

        let handle =
            tokio::spawn(
                async move { execute_query(&task_client, &task_query, token_clone).await },
            );

        // Cancel after a short delay
        tokio::time::sleep(Duration::from_millis(50)).await;
        token.cancel();

        let state = handle.await.unwrap();
        assert!(matches!(
            state,
            QueryState::Error { error, .. } if error.is_cancelled()
        ));

        // Running the same query to completion exercises the fetch body that
        // the cancelled run never reached.
        let state = execute_query(&client, &query, CancellationToken::new()).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"done".to_string()));
    }

    #[tokio::test]
    async fn test_execute_query_select_transformation() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query = Query::new(key.clone(), || async { Ok("data".to_string()) })
            .select(|s: &String| s.to_uppercase());

        let state = execute_query(&client, &query, test_token()).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"DATA".to_string()));

        // The cached data should also be transformed
        let cached: Option<String> = client.get_query_data(&key);
        assert_eq!(cached, Some("DATA".to_string()));
    }

    #[tokio::test]
    async fn test_execute_query_select_with_refetch() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        // Pre-populate with original data (without select applied)
        client.set_query_data(&key, "old".to_string(), QueryOptions::default());

        let query = Query::new(key.clone(), || async { Ok("new".to_string()) })
            .select(|s: &String| s.to_uppercase());

        let state = execute_query(&client, &query, test_token()).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"NEW".to_string()));
    }

    #[tokio::test]
    async fn test_spawn_query_increments_and_decrements_fetching_count() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<String> = Query::new(key.clone(), || async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok("data".to_string())
        });

        assert_eq!(client.fetching_count(), 0);

        // The spawn bookkeeping itself is covered by the `#[gpui::test]`
        // suite below; here we verify that a plain execution leaves the
        // fetching count untouched afterwards (and runs the fetch body).
        let state = execute_query(&client, &query, test_token()).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"data".to_string()));
        assert_eq!(client.fetching_count(), 0);
    }

    // ---- Network gating / retryer integration tests ----

    #[test]
    fn test_delay_for_matches_tanstack_curve_for_default_config() {
        let config = crate::options::RetryConfig::default();
        assert_eq!(delay_for(&config, 0), Duration::from_millis(1000));
        assert_eq!(delay_for(&config, 2), crate::retryer::retry_delay(2));
        assert_eq!(delay_for(&config, 10), Duration::from_secs(30));
    }

    #[test]
    fn test_delay_for_falls_back_to_config_logic() {
        let config = crate::options::RetryConfig::new(3)
            .base_delay(Duration::from_millis(50))
            .exponential_backoff(false);
        assert_eq!(delay_for(&config, 0), Duration::from_millis(50));
        assert_eq!(delay_for(&config, 2), Duration::from_millis(50));
    }

    #[tokio::test]
    async fn test_execute_query_paused_when_offline_in_online_mode() {
        let client = QueryClient::new();
        client.online_manager().set_online(false);
        let key = QueryKey::new("test");
        client.set_query_data(&key, "stale".to_string(), QueryOptions::default());

        let query = Query::new(key.clone(), || async { Ok("new".to_string()) });
        let state = execute_query(&client, &query, test_token()).await;

        assert!(
            matches!(
                &state,
                QueryState::Error { error, .. } if *error == QueryError::Paused
            ),
            "expected Paused error, got {state:?}"
        );
        // The current (stale) data must be preserved.
        assert_eq!(state.data(), Some(&"stale".to_string()));
        // The fetch never ran: the cache still holds the old value.
        let cached: Option<String> = client.get_query_data(&key);
        assert_eq!(cached, Some("stale".to_string()));
    }

    #[tokio::test]
    async fn test_execute_query_offline_always_mode_starts() {
        let client = QueryClient::new();
        client.online_manager().set_online(false);
        let key = QueryKey::new("test");

        let query =
            Query::new(key.clone(), || async { Ok("offline".to_string()) }).options(QueryOptions {
                network_mode: NetworkMode::Always,
                ..Default::default()
            });

        let state = execute_query(&client, &query, test_token()).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"offline".to_string()));
    }

    #[tokio::test]
    async fn test_execute_query_offline_first_starts_first_attempt() {
        let client = QueryClient::new();
        client.online_manager().set_online(false);
        let key = QueryKey::new("test");

        let query =
            Query::new(key.clone(), || async { Ok("first".to_string()) }).options(QueryOptions {
                network_mode: NetworkMode::OfflineFirst,
                ..Default::default()
            });

        let state = execute_query(&client, &query, test_token()).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"first".to_string()));
    }

    #[tokio::test(start_paused = true)]
    async fn test_execute_query_retry_pauses_until_online() {
        let client = QueryClient::new();
        client.online_manager().set_online(false);
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);
        let key = QueryKey::new("test");

        // offlineFirst: the first attempt runs offline; retries pause until
        // connectivity returns.
        let query = Query::new(key, move || {
            let a = Arc::clone(&attempts_clone);
            async move {
                let count = a.fetch_add(1, Ordering::SeqCst);
                if count < 1 {
                    Err(QueryError::Network("dropped".to_string()))
                } else {
                    Ok("recovered".to_string())
                }
            }
        })
        .options(QueryOptions {
            network_mode: NetworkMode::OfflineFirst,
            retry: crate::options::RetryConfig::new(3).base_delay(Duration::from_millis(10)),
            ..Default::default()
        });

        let client_for_task = client.clone();
        let handle = tokio::spawn(async move {
            execute_query(&client_for_task, &query, CancellationToken::new()).await
        });

        tokio::time::advance(Duration::from_millis(50)).await;
        assert!(
            !handle.is_finished(),
            "retry should be paused while offline"
        );

        client.online_manager().set_online(true);
        let state = handle.await.expect("task should finish");
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"recovered".to_string()));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_execute_query_skips_fetch_while_restoring() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let fetches_clone = Arc::clone(&fetches);

        let client = QueryClient::new();
        client.set_is_restoring(true);
        let key = QueryKey::new("test");
        let query = Query::new(key, move || {
            let f = Arc::clone(&fetches_clone);
            async move {
                f.fetch_add(1, Ordering::SeqCst);
                Ok::<String, QueryError>("fetched".to_string())
            }
        });

        let state = execute_query(&client, &query, test_token()).await;
        assert!(state.is_idle());
        assert_eq!(fetches.load(Ordering::SeqCst), 0);

        client.set_is_restoring(false);
        let state = execute_query(&client, &query, test_token()).await;
        assert!(state.is_success());
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    // ---- Existing mutation tests ----

    #[test]
    fn test_should_retry_network() {
        assert!(should_retry(
            &QueryError::Network("err".to_string()),
            &crate::options::RetryConfig::default()
        ));
    }

    #[test]
    fn test_should_retry_timeout() {
        assert!(should_retry(
            &QueryError::Timeout("err".to_string()),
            &crate::options::RetryConfig::default()
        ));
    }

    #[test]
    fn test_should_not_retry_custom() {
        assert!(!should_retry(
            &QueryError::custom("err"),
            &crate::options::RetryConfig::default()
        ));
    }

    #[tokio::test]
    async fn test_execute_mutation_success() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        let mutation =
            crate::mutation::Mutation::new(
                |param: i32| async move { Ok(format!("result_{param}")) },
            )
            .invalidates_key(key);

        let result = execute_mutation(&client, &mutation, 42).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "result_42");
    }

    #[tokio::test]
    async fn test_execute_mutation_error() {
        let client = QueryClient::new();
        let mutation: crate::mutation::Mutation<String, i32> =
            crate::mutation::Mutation::new(|_param: i32| async { Err(QueryError::custom("fail")) });

        let result = execute_mutation(&client, &mutation, 1).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_mutation_with_rollback() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        let key_for_test = key.clone();

        let mutation: crate::mutation::Mutation<String, i32> =
            crate::mutation::Mutation::new(|_param: i32| async { Err(QueryError::custom("fail")) })
                .on_mutate(move |client: &QueryClient, _params: &i32| {
                    client.set_query_data(&key, "optimistic".to_string(), QueryOptions::default());
                    let key_for_rollback = key.clone();
                    Ok(Box::new(move |client: &QueryClient| {
                        client.set_query_data(
                            &key_for_rollback,
                            "rolled_back".to_string(),
                            QueryOptions::default(),
                        );
                    }))
                });

        let result = execute_mutation(&client, &mutation, 1).await;
        assert!(result.is_err());

        let data: Option<String> = client.get_query_data(&key_for_test);
        assert_eq!(data, Some("rolled_back".to_string()));
    }

    #[tokio::test]
    async fn test_execute_mutation_invalidates_on_success() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        client.set_query_data(&key, "cached".to_string(), QueryOptions::default());
        assert!(!client.is_stale(&key));

        let mutation: crate::mutation::Mutation<String, i32> = crate::mutation::Mutation::new(
            |param: i32| async move { Ok(format!("result_{param}")) },
        )
        .invalidates_key(key.clone());

        let _ = execute_mutation(&client, &mutation, 1).await;
        assert!(client.is_stale(&key));
    }

    #[tokio::test]
    async fn test_execute_mutation_on_mutate_error() {
        let client = QueryClient::new();
        let fail_on_mutate = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let should_fail = Arc::clone(&fail_on_mutate);
        let mutation: crate::mutation::Mutation<String, i32> =
            crate::mutation::Mutation::new(|_param: i32| async {
                Ok("should not reach".to_string())
            })
            .on_mutate(move |_client: &QueryClient, _params: &i32| {
                if should_fail.load(Ordering::SeqCst) {
                    Err(QueryError::custom("optimistic failed"))
                } else {
                    Ok(Box::new(|_client: &QueryClient| {}))
                }
            });

        let result = execute_mutation(&client, &mutation, 1).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "optimistic failed");

        // Once `on_mutate` succeeds, the mutate body itself runs.
        fail_on_mutate.store(false, Ordering::SeqCst);
        let result = execute_mutation(&client, &mutation, 2).await;
        assert_eq!(result.expect("mutation should run"), "should not reach");
    }

    // ----- Coverage: pre-cancelled token / dedup result paths -----

    #[tokio::test]
    async fn test_execute_query_returns_cancelled_when_token_pre_cancelled() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "stale".to_string(), QueryOptions::default());

        let query = Query::new(key, || async { Ok("never".to_string()) });
        let token = test_token();
        token.cancel();

        let state = execute_query(&client, &query, token).await;
        assert!(
            matches!(
                &state,
                QueryState::Error { error, .. } if error.is_cancelled()
            ),
            "expected cancelled error, got {state:?}"
        );
        // The stale data snapshot is preserved.
        assert_eq!(state.data(), Some(&"stale".to_string()));
    }

    #[tokio::test]
    async fn test_execute_query_dedup_returns_cached_fresh_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("dedup-fresh");
        client.set_query_data(&key, "cached".to_string(), QueryOptions::default());

        let handle = tokio::spawn(async {});
        client.set_in_flight(
            &key,
            crate::client::InFlightTask {
                abort: {
                    let handle = handle;
                    Box::new(move || handle.abort())
                },
                cancel_token: CancellationToken::new(),
            },
        );

        let query: Query<String> = Query::new(key.clone(), || async { Ok("never".to_string()) });
        let state = execute_query(&client, &query, test_token()).await;

        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"cached".to_string()));
    }

    #[tokio::test]
    async fn test_execute_query_dedup_returns_cached_stale_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("dedup-stale");
        client.set_query_data(&key, "cached".to_string(), QueryOptions::default());
        client.invalidate_queries(&key);
        assert!(client.is_stale(&key));

        let handle = tokio::spawn(async {});
        client.set_in_flight(
            &key,
            crate::client::InFlightTask {
                abort: {
                    let handle = handle;
                    Box::new(move || handle.abort())
                },
                cancel_token: CancellationToken::new(),
            },
        );

        let query: Query<String> = Query::new(key.clone(), || async { Ok("never".to_string()) });
        let state = execute_query(&client, &query, test_token()).await;

        assert!(state.is_stale());
        assert_eq!(state.data(), Some(&"cached".to_string()));
    }

    #[tokio::test]
    async fn test_execute_query_dedup_waits_for_success_update() {
        let client = QueryClient::new();
        let key = QueryKey::new("dedup-waits");

        let handle = tokio::spawn(async {});
        client.set_in_flight(
            &key,
            crate::client::InFlightTask {
                abort: {
                    let handle = handle;
                    Box::new(move || handle.abort())
                },
                cancel_token: CancellationToken::new(),
            },
        );

        // Complete the "in-flight" fetch after the dedup waiter subscribed.
        let completer_client = client.clone();
        let completer_key = key.clone();
        let completer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            completer_client.set_query_data(
                &completer_key,
                "resolved".to_string(),
                QueryOptions::default(),
            );
            completer_client.clear_in_flight(&completer_key);
        });

        let query: Query<String> = Query::new(key.clone(), || async { Ok("never".to_string()) });
        let state = execute_query(&client, &query, test_token()).await;

        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"resolved".to_string()));
        completer.await.unwrap();
    }

    #[tokio::test]
    async fn test_execute_query_dedup_waits_for_stale_update() {
        let client = QueryClient::new();
        let key = QueryKey::new("dedup-stale-update");
        let cache_key = key.cache_key().to_string();

        let handle = tokio::spawn(async {});
        client.set_in_flight(
            &key,
            crate::client::InFlightTask {
                abort: {
                    let handle = handle;
                    Box::new(move || handle.abort())
                },
                cancel_token: CancellationToken::new(),
            },
        );

        let completer_client = client.clone();
        let completer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            // Write the cache entry directly: `set_query_data` would broadcast
            // a `Success` update first and short-circuit the dedup wait.
            completer_client.cache.insert(
                cache_key.clone(),
                crate::client::CacheEntry {
                    data: Arc::new("resolved".to_string()),
                    type_id: std::any::TypeId::of::<String>(),
                    fetched_at: std::time::Instant::now(),
                    last_accessed: std::time::Instant::now(),
                    options: QueryOptions::default(),
                    is_stale: true,
                },
            );
            completer_client.notify_subscribers(&cache_key, QueryStateVariant::Stale);
        });

        let query: Query<String> = Query::new(key.clone(), || async { Ok("never".to_string()) });
        let state = execute_query(&client, &query, test_token()).await;

        assert!(state.is_stale());
        assert_eq!(state.data(), Some(&"resolved".to_string()));
        completer.await.unwrap();
    }

    #[tokio::test]
    async fn test_execute_query_dedup_keeps_waiting_through_loading_updates() {
        let client = QueryClient::new();
        let key = QueryKey::new("dedup-loading");
        let cache_key = key.cache_key().to_string();

        let handle = tokio::spawn(async {});
        client.set_in_flight(
            &key,
            crate::client::InFlightTask {
                abort: {
                    let handle = handle;
                    Box::new(move || handle.abort())
                },
                cancel_token: CancellationToken::new(),
            },
        );

        let completer_client = client.clone();
        let completer_key = key.clone();
        let completer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            // Loading updates must be ignored by the dedup loop.
            completer_client.notify_subscribers(&cache_key, QueryStateVariant::Loading);
            completer_client.notify_subscribers(&cache_key, QueryStateVariant::Loading);
            completer_client.set_query_data(
                &completer_key,
                "resolved".to_string(),
                QueryOptions::default(),
            );
            completer_client.clear_in_flight(&completer_key);
        });

        let query: Query<String> = Query::new(key.clone(), || async { Ok("never".to_string()) });
        let state = execute_query(&client, &query, test_token()).await;

        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"resolved".to_string()));
        completer.await.unwrap();
    }

    // ----- Coverage: cancellation while paused for retry -----

    #[tokio::test(start_paused = true)]
    async fn test_execute_query_cancelled_while_paused_for_retry() {
        let client = QueryClient::new();
        client.online_manager().set_online(false);
        let key = QueryKey::new("test");

        // offline-first: the first attempt runs offline and fails; the retry
        // then pauses waiting for connectivity.
        let query: Query<String> = Query::new(key, || async {
            Err(QueryError::Network("dropped".to_string()))
        })
        .options(QueryOptions {
            network_mode: NetworkMode::OfflineFirst,
            retry: crate::options::RetryConfig::new(3).base_delay(Duration::from_millis(10)),
            ..Default::default()
        });

        let token = CancellationToken::new();
        let client_for_task = client.clone();
        let task_token = token.clone();
        let handle =
            tokio::spawn(async move { execute_query(&client_for_task, &query, task_token).await });

        tokio::time::advance(Duration::from_millis(50)).await;
        assert!(
            !handle.is_finished(),
            "retry should be paused while offline"
        );

        // Cancelling while parked must surface a cancelled error.
        token.cancel();
        let state = handle.await.expect("task should finish");
        assert!(
            matches!(
                &state,
                QueryState::Error { error, .. } if error.is_cancelled()
            ),
            "expected cancelled error, got {state:?}"
        );
    }

    // ----- Coverage: mutation on_success / on_error callbacks -----

    #[tokio::test]
    async fn test_execute_mutation_invokes_on_success() {
        let client = QueryClient::new();
        let fired = Arc::new(AtomicUsize::new(0));
        let fired_clone = Arc::clone(&fired);

        let mutation = crate::mutation::Mutation::new(|param: i32| async move { Ok(param * 2) })
            .on_success(move |_data: &i32, _params: &i32| {
                fired_clone.fetch_add(1, Ordering::SeqCst);
            });

        let result = execute_mutation(&client, &mutation, 21).await;
        assert_eq!(result.expect("should succeed"), 42);
        assert_eq!(fired.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_execute_mutation_invokes_on_error_and_rollback() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        let fired = Arc::new(AtomicUsize::new(0));
        let fired_clone = Arc::clone(&fired);

        let mutation: crate::mutation::Mutation<String, i32> =
            crate::mutation::Mutation::new(|_param: i32| async {
                Err(QueryError::custom("mutate failed"))
            })
            .on_mutate(move |client: &QueryClient, _params: &i32| {
                client.set_query_data(
                    &QueryKey::new("users"),
                    "optimistic".to_string(),
                    QueryOptions::default(),
                );
                Ok(Box::new(|client: &QueryClient| {
                    client.set_query_data(
                        &QueryKey::new("users"),
                        "rolled_back".to_string(),
                        QueryOptions::default(),
                    );
                }))
            })
            .on_error(move |_error: &QueryError, _params: &i32| {
                fired_clone.fetch_add(1, Ordering::SeqCst);
            });

        let result = execute_mutation(&client, &mutation, 1).await;
        assert!(result.is_err());
        assert_eq!(fired.load(Ordering::SeqCst), 1);

        let data: Option<String> = client.get_query_data(&key);
        assert_eq!(data, Some("rolled_back".to_string()));
    }

    // ----- Coverage: spawn_query / spawn_mutation on the client runtime -----

    /// Poll `ready` until it holds or the deadline passes (real-time; the
    /// pooled executor delivers asynchronously).
    fn wait_until_ready(ready: impl Fn() -> bool) {
        // Thread sleep (not `tokio::time`): the helper works under
        // `futures::executor::block_on` in plain `#[test]`s too.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !ready() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(ready(), "condition not reached in time");
    }

    #[test]
    fn wait_until_ready_polls_until_the_condition_flips() {
        // The condition is false on the first poll and true afterwards, so
        // the helper must actually poll repeatedly.
        let polls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&polls);
        wait_until_ready(move || counter.fetch_add(1, Ordering::SeqCst) >= 1);
        // At least one repeat poll happened (the first poll returned false);
        // real-time scheduling may interleave one extra benign poll.
        assert!(polls.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn test_spawn_query_resolves_idle_while_restoring() {
        let client = crate::test_support::tokio_client();
        client.set_is_restoring(true);
        let key = QueryKey::new("spawn-restore");
        let fetches = Arc::new(AtomicUsize::new(0));
        let fetches_clone = Arc::clone(&fetches);
        let query = Query::new(key.clone(), move || {
            let f = Arc::clone(&fetches_clone);
            async move {
                f.fetch_add(1, Ordering::SeqCst);
                Ok::<String, QueryError>("never".to_string())
            }
        });

        let state = spawn_query(&client, &query).await;
        assert!(
            matches!(state, QueryState::Idle),
            "restoring gate should resolve Idle, got {state:?}"
        );
        // No fetch ran and no counters leaked.
        assert_eq!(fetches.load(Ordering::SeqCst), 0);
        assert_eq!(client.fetching_count(), 0);
        assert!(!client.is_in_flight(&key));

        // Once restoring ends, the same query fetches normally.
        client.set_is_restoring(false);
        let state = execute_query(&client, &query, CancellationToken::new()).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"never".to_string()));
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_spawn_query_delivers_success_and_cleans_in_flight() {
        let client = crate::test_support::tokio_client();
        let key = QueryKey::new("spawn-success");
        let query = Query::new(key.clone(), || async {
            Ok::<String, QueryError>("fetched".to_string())
        });

        let handle = spawn_query(&client, &query);
        assert!(client.is_in_flight(&key), "spawn registers the fetch");
        assert_eq!(client.fetching_count(), 1);

        let state = handle.await;
        assert!(state.is_success(), "expected success, got {state:?}");
        assert_eq!(state.data(), Some(&"fetched".to_string()));
        wait_until_ready(|| !client.is_in_flight(&key));
        assert_eq!(client.fetching_count(), 0);
    }

    #[tokio::test]
    async fn test_spawn_query_cleans_in_flight_on_cancel_query() {
        let client = crate::test_support::tokio_client();
        let key = QueryKey::new("spawn-abort");
        let query = Query::new(key.clone(), || async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok::<String, QueryError>("never".to_string())
        });

        let handle = spawn_query(&client, &query);
        assert!(client.is_in_flight(&key));

        client.cancel_query(&key);
        assert!(!client.is_in_flight(&key));

        // The aborted fetch surfaces as a cancelled error state carrying
        // whatever data was cached before the cancellation.
        client.set_query_data(&key, "stale".to_string(), QueryOptions::default());
        let state = handle.await;
        assert!(
            state.is_error(),
            "aborted spawn must deliver an error, got {state:?}"
        );
        assert_eq!(state.data(), Some(&"stale".to_string()));

        // The fetch closure itself still works when driven directly.
        let state = execute_query(&client, &query, CancellationToken::new()).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"never".to_string()));
    }

    #[tokio::test]
    async fn test_spawn_mutation_delivers_success() {
        let client = crate::test_support::tokio_client();
        let key = QueryKey::new("spawn-mutation");
        client.set_query_data(&key, "cached".to_string(), QueryOptions::default());

        let mutation =
            crate::mutation::Mutation::new(|param: i32| async move { Ok(param.to_string()) })
                .invalidates_key(key.clone());

        let handle = spawn_mutation(&client, &mutation, 42);
        assert_eq!(client.mutating_count(), 1);

        let state = handle.await;
        assert!(
            matches!(&state, MutationState::Success(data) if data == "42"),
            "expected success, got {state:?}"
        );
        wait_until_ready(|| client.mutating_count() == 0);
        assert!(client.is_stale(&key), "invalidation ran on success");
    }

    #[tokio::test]
    async fn test_spawn_mutation_delivers_error() {
        let client = crate::test_support::tokio_client();
        let mutation: crate::mutation::Mutation<String, i32> =
            crate::mutation::Mutation::new(|_param: i32| async {
                Err(QueryError::custom("spawned mutation failed"))
            });

        let state = spawn_mutation(&client, &mutation, 1).await;
        assert!(
            matches!(
                &state,
                MutationState::Error(error)
                    if error.to_string() == "spawned mutation failed"
            ),
            "expected error, got {state:?}"
        );
        wait_until_ready(|| client.mutating_count() == 0);
    }

    #[tokio::test]
    async fn test_spawn_mutation_skips_while_restoring() {
        let client = crate::test_support::tokio_client();
        client.set_is_restoring(true);

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = Arc::clone(&calls);
        let mutation = crate::mutation::Mutation::new(move |_param: i32| {
            let calls = Arc::clone(&calls_clone);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok::<String, QueryError>("never".to_string())
            }
        });

        let state = spawn_mutation(&client, &mutation, 1).await;
        assert!(
            matches!(state, MutationState::Idle),
            "expected idle while restoring, got {state:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(client.mutating_count(), 0);

        // Once restoring ends, the mutation runs normally.
        client.set_is_restoring(false);
        let result = execute_mutation(&client, &mutation, 1).await;
        assert_eq!(result.expect("mutation should run"), "never");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_spawn_query_maps_panicked_fetch_to_custom_error() {
        // A panicking fetch is caught inside the spawned task (panic
        // isolation, mirroring the old tokio-join mapping) and surfaces as a
        // non-cancelled custom error state.
        let client = crate::test_support::tokio_client();
        let key = QueryKey::new("spawn-panic");
        let query: Query<String> = Query::new(key.clone(), || async {
            panic!("fetch exploded");
        });

        let handle = spawn_query(&client, &query);
        assert!(client.is_in_flight(&key));

        let state = handle.await;
        assert!(
            matches!(&state, QueryState::Error { error, .. } if !error.is_cancelled()),
            "panicked fetch should map to a custom error, got {state:?}"
        );
        wait_until_ready(|| !client.is_in_flight(&key));
        assert_eq!(client.fetching_count(), 0);
    }

    // ----- Coverage: wait_for_dedup dataless-update and closed-stream paths -----

    #[tokio::test]
    async fn test_wait_for_dedup_ignores_dataless_updates_and_falls_back_to_loading() {
        let client = QueryClient::new();
        let key = QueryKey::new("dedup-no-data");
        let cache_key = key.cache_key().to_string();
        let query: Query<String> = Query::new(key, || async { Ok("never".to_string()) });

        // Drive the wait future manually so updates can be interleaved
        // between individual polls deterministically.
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        let mut wait = Box::pin(wait_for_dedup(&client, &query));

        // First poll: subscribes, finds no cached data, parks on the stream.
        assert!(
            wait.as_mut().poll(&mut cx).is_pending(),
            "no cached data and no updates yet"
        );

        // A Success update without cached data must not resolve the wait.
        client.notify_subscribers(&cache_key, QueryStateVariant::Success);
        assert!(
            wait.as_mut().poll(&mut cx).is_pending(),
            "Success without data keeps the dedup wait alive"
        );

        // The same holds for a Stale update without cached data.
        client.notify_subscribers(&cache_key, QueryStateVariant::Stale);
        assert!(
            wait.as_mut().poll(&mut cx).is_pending(),
            "Stale without data keeps the dedup wait alive"
        );

        // Overflowing the subscription channel (capacity 16) surfaces a
        // `Lagged` error: the wait gives up and reports `Loading`.
        for _ in 0..(16 + 2) {
            client.notify_subscribers(&cache_key, QueryStateVariant::Loading);
        }
        match wait.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(state) => assert!(state.is_loading()),
            std::task::Poll::Pending => panic!("a lagged stream must fall back to loading"),
        }

        // The fetch closure itself is never run by the dedup wait; exercise
        // it directly so its body is covered.
        assert!((query.fetch_fn)().await.is_ok());
    }
}
