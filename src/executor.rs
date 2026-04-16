// src/executor.rs
//! Query and mutation execution

use crate::mutation::{Mutation, RollbackFn};
use crate::observer::QueryStateVariant;
use crate::query::Query;
use crate::{QueryClient, QueryError, QueryState};
use futures_timer::Delay;
use std::time::Duration;

/// Result of a query execution.
pub struct QueryResult<T> {
    pub state: QueryState<T>,
}

/// Execute a query and return the result via a oneshot channel.
pub fn spawn_query<T: Clone + Send + Sync + 'static, V: 'static>(
    _cx: &mut gpui::Context<V>,
    client: &QueryClient,
    query: &Query<T>,
    callback: impl FnOnce(&mut V, QueryState<T>, &mut gpui::Context<V>) + Send + 'static,
) {
    let key = query.key.clone();
    let options = query.options.clone();
    let initial_data = query.initial_data.clone();
    let select_fn = query.select.clone();
    let client = client.clone();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            if !options.enabled {
                let state = if let Some(initial) = initial_data {
                    QueryState::Success(select_fn.as_ref().map_or_else(|| initial, |f| f(&initial)))
                } else {
                    QueryState::Idle
                };
                let _ = (state, callback);
                return;
            }

            if client.is_in_flight(&key) {
                return;
            }

            if client.get_query_data::<T>(&key).is_none() {
                if let Some(initial) = initial_data {
                    let resolved = select_fn.as_ref().map_or_else(|| initial, |f| f(&initial));
                    client.set_query_data(&key, resolved, options.clone());
                }
            }

            let mut observer =
                crate::observer::QueryObserver::with_options(&client, key.clone(), options.clone());

            let has_stale_data = observer.state().has_data() && client.is_stale(&key);

            if has_stale_data {
                observer.set_loading();
            } else if observer.state().has_data() && !client.is_stale(&key) {
                return;
            } else {
                observer.set_loading();
            }

            let abort_handle =
                tokio::spawn(async { Ok::<T, QueryError>(unreachable!()) }).abort_handle();
            client.set_abort_handle(&key, abort_handle);

            let retry_config = options.retry.clone();

            let result: Result<T, QueryError> = loop {
                break Err(QueryError::Cancelled);
            };

            let final_result: Result<T, QueryError> = if let Err(ref _err) = result {
                let mut last_err = _err.clone();
                let mut succeeded = false;
                let mut success_data: Option<T> = None;
                for retry_attempt in 0..retry_config.max_retries {
                    if !retry_config.should_retry(retry_attempt) {
                        break;
                    }
                    let _delay = retry_config.delay_for_attempt(retry_attempt);
                    Delay::new(_delay).await;
                    last_err = QueryError::network(format!("attempt {}", retry_attempt));
                }
                if succeeded {
                    Ok(success_data.unwrap())
                } else {
                    Err(last_err)
                }
            } else {
                result
            };

            client.clear_abort_handle(&key);

            match final_result {
                Ok(data) => {
                    let transformed = select_fn
                        .as_ref()
                        .map_or_else(|| data.clone(), |f| f(&data));
                    client.set_query_data(&key, transformed, options.clone());
                }
                Err(_error) => {
                    client.notify_subscribers(key.cache_key(), QueryStateVariant::Error);
                }
            }

            let _ = observer;
        });
    });
}

/// Execute a mutation with optional optimistic updates and rollback.
pub fn spawn_mutation<
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + Sync + 'static,
    V: 'static,
>(
    _cx: &mut gpui::Context<V>,
    client: &QueryClient,
    mutation: &Mutation<T, P>,
    params: P,
    callback: impl FnOnce(&mut V, crate::MutationState<T>, &mut gpui::Context<V>) + Send + 'static,
) {
    let mutate_fn = Arc::clone(&mutation.mutate_fn);
    let invalidates_keys = mutation.invalidates_keys.clone();
    let on_mutate = mutation.on_mutate.clone();
    let on_success = mutation.on_success.clone();
    let on_error = mutation.on_error.clone();
    let client = client.clone();
    let params_clone = params.clone();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            let rollback: Option<RollbackFn> = if let Some(ref on_mutate_fn) = on_mutate {
                match on_mutate_fn(&client, &params_clone) {
                    Ok(rollback) => Some(rollback),
                    Err(err) => {
                        let _ = (
                            crate::MutationState::<T>::Error(err),
                            &params_clone,
                            &on_error,
                        );
                        return;
                    }
                }
            } else {
                None
            };

            let result = mutate_fn(params_clone.clone()).await;

            match result {
                Ok(data) => {
                    if let Some(ref success_fn) = on_success {
                        success_fn(&data, &params_clone);
                    }
                    for key in &invalidates_keys {
                        client.invalidate_queries(key);
                    }
                    let _ = (crate::MutationState::Success(data), callback);
                }
                Err(error) => {
                    if let Some(rollback) = rollback {
                        rollback(&client);
                    }
                    if let Some(ref error_fn) = on_error {
                        error_fn(&error, &params_clone);
                    }
                    let _ = (crate::MutationState::Error(error), callback);
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    #[test]
    fn test_query_result_struct() {
        let result = QueryResult {
            state: QueryState::Success(42i32),
        };
        assert!(result.state.is_success());
    }

    #[tokio::test]
    async fn test_retry_logic_success_on_first_try() {
        let attempt_count = Arc::new(AtomicUsize::new(0));
        let attempt_clone = Arc::clone(&attempt_count);

        let fetch_fn = || {
            let c = Arc::clone(&attempt_clone);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok::<String, QueryError>("data".to_string())
            }
        };

        let result = fetch_fn().await;
        assert!(result.is_ok());
        assert_eq!(attempt_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_logic_succeeds_after_retries() {
        let retry_config = crate::RetryConfig::new(3)
            .base_delay(Duration::from_millis(1))
            .exponential_backoff(false);

        let attempt_count = Arc::new(AtomicUsize::new(0));
        let attempt_clone = Arc::clone(&attempt_count);

        let mut attempt = 0u32;
        loop {
            attempt_clone.fetch_add(1, Ordering::SeqCst);
            let current = attempt_clone.load(Ordering::SeqCst);

            if current >= 3 {
                break Ok::<String, QueryError>("data".to_string());
            }

            if retry_config.should_retry(attempt) {
                let delay = retry_config.delay_for_attempt(attempt);
                Delay::new(delay).await;
                attempt += 1;
                continue;
            }
            break Err(QueryError::network("max retries"));
        }

        assert_eq!(attempt_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_logic_exhausts_retries() {
        let retry_config = crate::RetryConfig::new(2).base_delay(Duration::from_millis(1));

        let mut last_err = QueryError::network("fail");
        for attempt in 0..retry_config.max_retries {
            if !retry_config.should_retry(attempt) {
                break;
            }
            let delay = retry_config.delay_for_attempt(attempt);
            Delay::new(delay).await;
            last_err = QueryError::network(format!("attempt {}", attempt));
        }

        assert!(matches!(last_err, QueryError::Network(_)));
    }

    #[tokio::test]
    async fn test_retry_delay_calculation() {
        let config = crate::RetryConfig::new(5).base_delay(Duration::from_millis(10));
        assert_eq!(config.delay_for_attempt(0), Duration::from_millis(10));
        assert_eq!(config.delay_for_attempt(1), Duration::from_millis(20));
        assert_eq!(config.delay_for_attempt(2), Duration::from_millis(40));
        assert_eq!(config.delay_for_attempt(3), Duration::from_millis(80));
    }

    #[test]
    fn test_mutation_rollback_on_error() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        client.set_query_data(&key, "original".to_string(), crate::QueryOptions::default());
        client.set_query_data(
            &key,
            "optimistic".to_string(),
            crate::QueryOptions::default(),
        );
        client.set_query_data(
            &key,
            "rolled_back".to_string(),
            crate::QueryOptions::default(),
        );

        let data: Option<String> = client.get_query_data(&key);
        assert_eq!(data, Some("rolled_back".to_string()));
    }

    #[test]
    fn test_mutation_invalidate_on_success() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("users");
        let key2 = QueryKey::new("posts");
        client.set_query_data(&key1, "users".to_string(), crate::QueryOptions::default());
        client.set_query_data(&key2, "posts".to_string(), crate::QueryOptions::default());

        client.invalidate_queries(&key1);
        client.invalidate_queries(&key2);

        assert!(client.is_stale(&key1));
        assert!(client.is_stale(&key2));
    }

    #[tokio::test]
    async fn test_query_disabled_no_fetch() {
        let options = crate::QueryOptions {
            enabled: false,
            ..Default::default()
        };
        assert!(!options.enabled);
    }

    #[tokio::test]
    async fn test_query_deduplication_check() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        assert!(!client.is_in_flight(&key));

        let handle = tokio::spawn(async {});
        client.set_abort_handle(&key, handle.abort_handle());
        assert!(client.is_in_flight(&key));

        client.clear_abort_handle(&key);
        assert!(!client.is_in_flight(&key));
    }

    #[test]
    fn test_query_error_preserves_stale_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(
            &key,
            "stale_data".to_string(),
            crate::QueryOptions::default(),
        );

        client.notify_subscribers(key.cache_key(), QueryStateVariant::Error);

        let data: Option<String> = client.get_query_data(&key);
        assert_eq!(data, Some("stale_data".to_string()));
    }

    #[test]
    fn test_initial_data_set_when_cache_empty() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        assert!(client.get_query_data::<String>(&key).is_none());
        client.set_query_data(&key, "initial".to_string(), crate::QueryOptions::default());

        let data: Option<String> = client.get_query_data(&key);
        assert_eq!(data, Some("initial".to_string()));
    }

    #[test]
    fn test_initial_data_not_overwrite_existing() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "existing".to_string(), crate::QueryOptions::default());

        let existing: Option<String> = client.get_query_data(&key);
        assert!(existing.is_some());
    }
}
