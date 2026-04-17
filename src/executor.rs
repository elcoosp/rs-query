// src/executor.rs
//! Query and mutation execution primitives.
//!
//! Provides `spawn_query`, `spawn_mutation`, and `spawn_infinite_query` which
//! bridge GPUI's foreground executor with Tokio-based async fetch functions.

use crate::cancellation::cancellable_fetch;
use crate::client::QueryClient;
use crate::infinite::{InfiniteData, InfiniteQuery, InfiniteQueryObserver};
use crate::mutation::{Mutation, MutationState};
use crate::observer::QueryObserver;
use crate::query::Query;
use crate::state::{QueryState, QueryStateVariant};
use crate::QueryError;
use std::any::TypeId;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Guard types for activity counters
// ---------------------------------------------------------------------------

/// Guard that decrements the fetching counter when dropped.
struct FetchingGuard {
    client: QueryClient,
}

impl Drop for FetchingGuard {
    fn drop(&mut self) {
        self.client.dec_fetching();
    }
}

/// Guard that decrements the mutating counter when dropped.
struct MutatingGuard {
    client: QueryClient,
}

impl Drop for MutatingGuard {
    fn drop(&mut self) {
        self.client.dec_mutating();
    }
}

// ---------------------------------------------------------------------------
// In-flight task helper
// ---------------------------------------------------------------------------

use tokio::task::AbortHandle;

/// Stored per in-flight query so it can be cancelled.
pub(crate) struct InFlightTask {
    pub abort_handle: AbortHandle,
    pub cancel_token: CancellationToken,
}

// ---------------------------------------------------------------------------
// Public executor functions
// ---------------------------------------------------------------------------

/// Execute a query on a background thread and deliver state updates to the view.
///
/// Uses [`QueryObserver::from_query`] so that `initialData` and `placeholderData`
/// are applied automatically. Deduplicates concurrent identical queries.
pub fn spawn_query<V, T, F>(
    cx: &mut gpui::ViewContext<V>,
    client: QueryClient,
    query: Query<T>,
    callback: F,
) where
    V: 'static,
    T: Clone + Send + Sync + 'static,
    F: Fn(&mut V, &QueryState<T>, &mut gpui::ViewContext<V>) + Send + Sync + 'static,
{
    // Use from_query to apply initialData
    let mut observer = QueryObserver::from_query(&client, &query);

    if !query.options.enabled {
        let initial_state = observer.state().clone();
        cx.spawn(|this, mut cx| async move {
            let _ = this.update(&mut cx, |view, cx| {
                callback(view, &initial_state, cx);
            });
        })
        .detach();
        return;
    }

    // Deduplication
    if client.is_in_flight(&query.key) {
        return;
    }

    let key = query.key.clone();
    observer.set_loading();
    let loading_state = observer.state().clone();

    // Increment fetching counter
    client.inc_fetching();

    let (tx, rx) = oneshot::channel();
    let token = CancellationToken::new();
    let token_clone = token.clone();

    let abort_handle = tokio::spawn(async move {
        let _guard = FetchingGuard {
            client: client.clone(),
        };
        let state = execute_query(&client, &query, token_clone).await;
        client.clear_in_flight(&key);
        let _ = tx.send(state);
    })
    .abort_handle();

    client.set_in_flight(
        &key,
        InFlightTask {
            abort_handle,
            cancel_token: token,
        },
    );

    cx.spawn(|this, mut cx| async move {
        // Deliver loading state
        let _ = this.update(&mut cx, |view, cx| {
            callback(view, &loading_state, cx);
        });

        // Wait for result
        match rx.await {
            Ok(final_state) => {
                let _ = this.update(&mut cx, |view, cx| {
                    callback(view, &final_state, cx);
                });
            }
            Err(_) => {
                // Task was cancelled or sender dropped
                let _ = this.update(&mut cx, |view, cx| {
                    callback(
                        view,
                        &QueryState::Error {
                            error: QueryError::Cancelled,
                            stale_data: None,
                        },
                        cx,
                    );
                });
            }
        }
    })
    .detach();
}

/// Core query execution logic: calls the fetch function with retries and cancellation.
pub async fn execute_query<T: Clone + Send + Sync + 'static>(
    client: &QueryClient,
    query: &Query<T>,
    token: CancellationToken,
) -> QueryState<T> {
    let max_retries = query.options.retry.max_retries;
    let mut delay = query.options.retry.initial_delay;

    for attempt in 0..=max_retries {
        // Check cancellation before each attempt
        if token.is_cancelled() {
            return QueryState::Error {
                error: QueryError::Cancelled,
                stale_data: client.get_query_data(&query.key),
            };
        }

        match cancellable_fetch(token.clone(), || (query.fetch_fn)()).await {
            Ok(data) => {
                let final_data = match &query.select {
                    Some(select_fn) => select_fn(&data),
                    None => data,
                };

                // Store in cache, applying structural sharing if configured
                if let Some(share_fn) = &query.share_fn {
                    client.set_query_data_shared(
                        &query.key,
                        final_data,
                        query.options.clone(),
                        share_fn,
                    );
                } else {
                    client.set_query_data(&query.key, final_data, query.options.clone());
                }

                return QueryState::Success(client.get_query_data(&query.key).unwrap());
            }
            Err(QueryError::Cancelled) => {
                return QueryState::Error {
                    error: QueryError::Cancelled,
                    stale_data: client.get_query_data(&query.key),
                };
            }
            Err(e) => {
                if attempt == max_retries {
                    return QueryState::Error {
                        error: e,
                        stale_data: client.get_query_data(&query.key),
                    };
                }
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = token.cancelled() => {
                        return QueryState::Error {
                            error: QueryError::Cancelled,
                            stale_data: client.get_query_data(&query.key),
                        };
                    }
                }
                delay = std::cmp::min(delay * 2, query.options.retry.max_delay);
            }
        }
    }

    unreachable!()
}

/// Execute a mutation on a background thread with optional optimistic updates.
pub fn spawn_mutation<V, T, P, F>(
    cx: &mut gpui::ViewContext<V>,
    client: QueryClient,
    mutation: Mutation<T, P>,
    params: P,
    callback: F,
) where
    V: 'static,
    T: Clone + Send + Sync + 'static,
    P: Send + Sync + 'static,
    F: Fn(&mut V, &MutationState<T>, &mut gpui::ViewContext<V>) + Send + Sync + 'static,
{
    client.inc_mutating();

    let (tx, rx) = oneshot::channel();

    tokio::spawn(async move {
        let _guard = MutatingGuard {
            client: client.clone(),
        };

        // Optimistic update phase
        let rollback: Option<Box<dyn FnOnce(&QueryClient) + Send>> = match &mutation.on_mutate {
            Some(on_mutate_fn) => match on_mutate_fn(&client, params) {
                Ok(rb) => Some(rb),
                Err(e) => {
                    let _ = tx.send(MutationState::Error(e));
                    return;
                }
            },
            None => None,
        };

        // Execute mutation
        let result = (mutation.mutate_fn)(params).await;

        match result {
            Ok(data) => {
                // Invalidate specified keys on success
                for key in &mutation.invalidates {
                    client.invalidate_queries(key);
                }
                let _ = tx.send(MutationState::Success(data));
            }
            Err(e) => {
                // Rollback on error
                if let Some(rb) = rollback {
                    rb(&client);
                }
                let _ = tx.send(MutationState::Error(e));
            }
        }
    });

    cx.spawn(|this, mut cx| async move {
        if let Ok(state) = rx.await {
            let _ = this.update(&mut cx, |view, cx| {
                callback(view, &state, cx);
            });
        }
    })
    .detach();
}

/// Execute an infinite query and return an observer for paginated access.
pub fn spawn_infinite_query<V, T, P, F>(
    cx: &mut gpui::ViewContext<V>,
    client: QueryClient,
    query: InfiniteQuery<T, P>,
    callback: F,
) -> InfiniteQueryObserver<T, P>
where
    V: 'static,
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + Sync + 'static,
    F: Fn(&mut V, &QueryState<InfiniteData<T, P>>, &mut gpui::ViewContext<V>)
        + Send
        + Sync
        + 'static,
{
    let key = query.key.clone();
    let initial_data = InfiniteData {
        pages: Vec::new(),
        page_params: Vec::new(),
    };

    client.inc_fetching();

    let (tx, rx) = oneshot::channel();
    let token = CancellationToken::new();
    let token_clone = token.clone();

    let abort_handle = tokio::spawn({
        let client = client.clone();
        let query = query.clone();
        async move {
            let _guard = FetchingGuard {
                client: client.clone(),
            };
            let state = execute_infinite_query(&client, &query, token_clone).await;
            client.clear_in_flight(&key);
            let _ = tx.send(state);
        }
    })
    .abort_handle();

    client.set_in_flight(
        &key,
        InFlightTask {
            abort_handle,
            cancel_token: token,
        },
    );

    // Notify loading
    let loading_state = QueryState::Loading;
    let client_clone = client.clone();
    cx.spawn(|this, mut cx| async move {
        let _ = this.update(&mut cx, |view, cx| {
            callback(view, &loading_state, cx);
        });
        if let Ok(final_state) = rx.await {
            let _ = this.update(&mut cx, |view, cx| {
                callback(view, &final_state, cx);
            });
        }
    })
    .detach();

    InfiniteQueryObserver::new(client_clone, query, initial_data)
}

async fn execute_infinite_query<T, P>(
    client: &QueryClient,
    query: &InfiniteQuery<T, P>,
    token: CancellationToken,
) -> QueryState<InfiniteData<T, P>>
where
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + Sync + 'static,
{
    let mut page_param = query.initial_page_param.clone();

    for _ in 0..=query.options.retry.max_retries {
        if token.is_cancelled() {
            return QueryState::Error {
                error: QueryError::Cancelled,
                stale_data: client.get_query_data(&query.key),
            };
        }

        match cancellable_fetch(token.clone(), || (query.fetch_fn)(page_param.clone())).await {
            Ok(page_data) => {
                let mut infinite_data = client
                    .get_infinite_data::<T, P>(&query.key)
                    .unwrap_or_else(|| InfiniteData {
                        pages: Vec::new(),
                        page_params: Vec::new(),
                    });

                infinite_data.push_page(page_data, page_param.clone());

                // Apply max_pages limit
                if let Some(max) = query.max_pages {
                    while infinite_data.pages.len() > max {
                        infinite_data.pages.remove(0);
                        infinite_data.page_params.remove(0);
                    }
                }

                client.set_infinite_data(&query.key, infinite_data.clone(), query.options.clone());

                return QueryState::Success(infinite_data);
            }
            Err(QueryError::Cancelled) => {
                return QueryState::Error {
                    error: QueryError::Cancelled,
                    stale_data: client.get_query_data(&query.key),
                };
            }
            Err(e) => {
                return QueryState::Error {
                    error: e,
                    stale_data: client.get_query_data(&query.key),
                };
            }
        }
    }

    unreachable!()
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{query::Query, QueryClient, QueryError, QueryKey, QueryOptions};
    use std::sync::atomic::{AtomicUsize, Ordering};

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
        let query: Query<String> = Query::new(key.clone(), || async { Ok("hello".to_string()) });

        let token = CancellationToken::new();
        let state = execute_query(&client, &query, token).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"hello".to_string()));
    }

    #[tokio::test]
    async fn test_execute_query_with_select() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<Vec<i32>> =
            Query::new(key.clone(), || async { Ok(vec![1, 2, 3]) }).select(|v| v.len());

        let token = CancellationToken::new();
        let state = execute_query(&client, &query, token).await;
        assert_eq!(state.data(), Some(&3));
    }

    #[tokio::test]
    async fn test_execute_query_retry_on_failure() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();

        let query: Query<String> = Query::new(key.clone(), move || {
            let a = attempts_clone.clone();
            async move {
                let count = a.fetch_add(1, Ordering::SeqCst) + 1;
                if count < 3 {
                    Err(QueryError::Message("fail".to_string()))
                } else {
                    Ok("retried".to_string())
                }
            }
        })
        .retry(crate::options::RetryConfig {
            max_retries: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
        });

        let token = CancellationToken::new();
        let state = execute_query(&client, &query, token).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"retried".to_string()));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_execute_query_cancellation() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<String> = Query::new(key.clone(), || async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            Ok("done".to_string())
        });

        let token = CancellationToken::new();
        let token_cancel = token.clone();

        let handle = tokio::spawn(async move { execute_query(&client, &query, token).await });

        tokio::time::sleep(Duration::from_millis(50)).await;
        token_cancel.cancel();

        let state = handle.await.unwrap();
        assert!(matches!(
            state,
            QueryState::Error {
                error: QueryError::Cancelled,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn test_execute_query_structural_sharing() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        // First fetch
        let query1: Query<Vec<i32>> =
            Query::new(key.clone(), || async { Ok(vec![1, 2, 3]) }).structural_sharing(true);
        let token1 = CancellationToken::new();
        let state1 = execute_query(&client, &query1, token1).await;

        // Get the arc from cache
        let cache_key = key.cache_key().to_string();
        let arc1 = {
            let entry = client.cache.get(&cache_key).unwrap();
            entry.data.clone()
        };

        // Second fetch with same data
        let query2: Query<Vec<i32>> =
            Query::new(key.clone(), || async { Ok(vec![1, 2, 3]) }).structural_sharing(true);
        let token2 = CancellationToken::new();
        let state2 = execute_query(&client, &query2, token2).await;

        let arc2 = {
            let entry = client.cache.get(&cache_key).unwrap();
            entry.data.clone()
        };

        // Should be the same Arc due to structural sharing
        assert!(Arc::ptr_eq(&arc1, &arc2));
        drop(state1);
        drop(state2);
    }
    #[tokio::test]
    async fn test_execute_query_success() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query = Query::new(key, || async { Ok("data".to_string()) });

        let state = execute_query(&client, &query).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"data".to_string()));
    }

    #[tokio::test]
    async fn test_execute_query_error_no_retry() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<String> =
            Query::new(key, || async { Err(QueryError::custom("immediate fail")) });

        let state = execute_query(&client, &query).await;
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

        let state = execute_query(&client, &query).await;
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

        let state = execute_query(&client, &query).await;
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

        let state = execute_query(&client, &query).await;
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

        let state = execute_query(&client, &query).await;
        assert!(state.is_error());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_execute_query_caches_result() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query = Query::new(key.clone(), || async { Ok("data".to_string()) });

        let _ = execute_query(&client, &query).await;

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
        let state = execute_query(&client, &query).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"new".to_string()));

        // Cache should be updated with new data
        let cached: Option<String> = client.get_query_data(&key);
        assert_eq!(cached, Some("new".to_string()));
    }
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
                |param: i32| async move { Ok(format!("result_{}", param)) },
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
            |param: i32| async move { Ok(format!("result_{}", param)) },
        )
        .invalidates_key(key.clone());

        let _ = execute_mutation(&client, &mutation, 1).await;
        assert!(client.is_stale(&key));
    }

    #[tokio::test]
    async fn test_execute_mutation_on_mutate_error() {
        let client = QueryClient::new();
        let mutation: crate::mutation::Mutation<String, i32> =
            crate::mutation::Mutation::new(|_param: i32| async {
                Ok("should not reach".to_string())
            })
            .on_mutate(|_client: &QueryClient, _params: &i32| {
                Err(QueryError::custom("optimistic failed"))
            });

        let result = execute_mutation(&client, &mutation, 1).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "optimistic failed");
    }
}
