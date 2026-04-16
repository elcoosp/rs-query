// src/executor.rs
//! Query executor - handles async query execution with retries and deduplication

use crate::error::QueryError;
use crate::observer::QueryStateVariant;
use crate::query::Query;
use crate::{MutationState, QueryClient, QueryState};
use futures_timer::Delay;
use std::sync::Arc;

/// Execute a query with retries and deduplication
pub async fn execute_query<T: Clone + Send + Sync + 'static>(
    client: &QueryClient,
    query: &Query<T>,
) -> QueryState<T> {
    let cache_key = query.key.cache_key();

    // Check if already in flight (deduplication)
    if client.is_in_flight(&query.key) {
        let mut rx = client.subscribe(&cache_key);

        if let Some(data) = client.get_query_data::<T>(&query.key) {
            let is_stale = client.is_stale(&query.key);
            if is_stale {
                return QueryState::Stale(data);
            } else {
                return QueryState::Success(data);
            }
        }

        if let Ok(update) = rx.recv().await {
            match update.state_variant {
                QueryStateVariant::Success => {
                    if let Some(data) = client.get_query_data::<T>(&query.key) {
                        return QueryState::Success(data);
                    }
                }
                QueryStateVariant::Error => {
                    return QueryState::Error {
                        error: QueryError::custom("Deduplicated request failed"),
                        stale_data: client.get_query_data::<T>(&query.key),
                    };
                }
                QueryStateVariant::Loading => {
                    return QueryState::Loading;
                }
                QueryStateVariant::Stale => {
                    if let Some(data) = client.get_query_data::<T>(&query.key) {
                        return QueryState::Stale(data);
                    }
                }
                QueryStateVariant::Idle | QueryStateVariant::Refetching => {
                    return QueryState::Idle;
                }
            }
        }

        return QueryState::Loading;
    }

    let is_refetch = client.get_query_data::<T>(&query.key).is_some();

    if is_refetch {
        if let Some(_data) = client.get_query_data::<T>(&query.key) {
            let is_stale = client.is_stale(&query.key);
            if is_stale {
                client.notify_subscribers(&cache_key, QueryStateVariant::Loading);
            } else {
                client.notify_subscribers(&cache_key, QueryStateVariant::Stale);
            }
        }
    } else {
        client.notify_subscribers(&cache_key, QueryStateVariant::Loading);
    }

    let fetch_fn = Arc::clone(&query.fetch_fn);
    let select_fn = query.select.clone();
    let options = query.options.clone();
    let key = query.key.clone();
    let client_clone = client.clone();

    // Execute inline (no tokio::spawn - GPUI's executor isn't tokio-based)
    let mut attempts = 0;
    let max_attempts = options.retry.max_retries as usize + 1;

    loop {
        attempts += 1;

        match fetch_fn().await {
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
            Err(e) => {
                if attempts < max_attempts && should_retry(&e, &options.retry) {
                    let delay = options.retry.delay_for_attempt((attempts - 1) as u32);
                    Delay::new(delay).await;
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

/// Execute a mutation with optimistic update support
pub async fn execute_mutation<T: Clone + Send + Sync + 'static, P: Clone + Send + 'static>(
    client: &QueryClient,
    mutation: &crate::mutation::Mutation<T, P>,
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
            Ok(data)
        }
        Err(e) => {
            if let Some(rollback) = rollback {
                rollback(client);
            }
            Err(e)
        }
    }
}

fn should_retry(error: &QueryError, _retry: &crate::options::RetryConfig) -> bool {
    match error {
        QueryError::Network(_) => true,
        QueryError::Timeout(_) => true,
        _ => false,
    }
}

/// Spawn a query on GPUI's foreground executor
pub fn spawn_query<V: 'static, T: Clone + Send + Sync + 'static>(
    cx: &mut gpui::Context<V>,
    client: &QueryClient,
    query: &Query<T>,
    callback: impl FnOnce(&mut V, QueryState<T>, &mut gpui::Context<V>) + Send + 'static,
) {
    let client = client.clone();
    let query = query.clone();
    let entity = cx.entity().downgrade();
    let mut async_cx = cx.to_async();

    cx.foreground_executor()
        .spawn(async move {
            let state = execute_query(&client, &query).await;
            let _ = entity.update(&mut async_cx, |this, cx| {
                callback(this, state, cx);
            });
        })
        .detach();
}

/// Spawn a mutation on GPUI's foreground executor
pub fn spawn_mutation<
    V: 'static,
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + Sync + 'static,
>(
    cx: &mut gpui::Context<V>,
    client: &QueryClient,
    mutation: &crate::mutation::Mutation<T, P>,
    params: P,
    callback: impl FnOnce(&mut V, MutationState<T>, &mut gpui::Context<V>) + Send + 'static,
) {
    let client = client.clone();
    let mutation = mutation.clone();
    let entity = cx.entity().downgrade();
    let mut async_cx = cx.to_async();

    cx.foreground_executor()
        .spawn(async move {
            let result = execute_mutation(&client, &mutation, params).await;

            let state = match result {
                Ok(data) => MutationState::Success(data),
                Err(e) => MutationState::Error(e),
            };

            let _ = entity.update(&mut async_cx, |this, cx| {
                callback(this, state, cx);
            });
        })
        .detach();
}
// src/executor.rs - Add test module

#[cfg(test)]
// src/executor.rs - Replace the test module with this corrected version
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
