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
