//! GPUI async execution helpers for infinite queries

use crate::infinite::{InfiniteData, InfiniteQuery};
use crate::observer::QueryStateVariant;
use crate::{QueryClient, QueryError, QueryState, RetryConfig};
use futures_timer::Delay;
use gpui::Context;
use std::future::Future;
use std::sync::Arc;

pub struct InfiniteQueryObserver<T, P>
where
    T: Clone + Send + Sync + std::fmt::Debug + 'static,
    P: Clone + Send + Sync + std::fmt::Debug + 'static,
{
    pub client: QueryClient,
    pub query: InfiniteQuery<T, P>,
    pub current_state: QueryState<InfiniteData<T, P>>,
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub is_fetching_next_page: bool,
    pub is_fetching_previous_page: bool,
}

impl<T, P> InfiniteQueryObserver<T, P>
where
    T: Clone + Send + Sync + std::fmt::Debug + 'static,
    P: Clone + Send + Sync + std::fmt::Debug + 'static,
{
    pub fn new(client: &QueryClient, query: InfiniteQuery<T, P>) -> Self {
        let current_state = Self::compute_state(client, &query.key);
        let (has_next, has_prev) = Self::compute_pagination_flags(client, &query);
        Self {
            client: client.clone(),
            query,
            current_state,
            has_next_page: has_next,
            has_previous_page: has_prev,
            is_fetching_next_page: false,
            is_fetching_previous_page: false,
        }
    }

    fn compute_state(
        client: &QueryClient,
        key: &crate::QueryKey,
    ) -> QueryState<InfiniteData<T, P>> {
        if let Some(data) = client.get_infinite_data::<T, P>(key) {
            let stale = client.is_stale(key);
            if stale {
                QueryState::Stale(data)
            } else {
                QueryState::Success(data)
            }
        } else {
            QueryState::Idle
        }
    }

    fn compute_pagination_flags(client: &QueryClient, query: &InfiniteQuery<T, P>) -> (bool, bool) {
        if let Some(data) = client.get_infinite_data::<T, P>(&query.key) {
            let has_next = if let Some(ref get_next) = query.get_next_page_param {
                data.pages
                    .last()
                    .and_then(|last| get_next(last, &data.pages))
                    .is_some()
            } else {
                false
            };
            let has_prev = if let Some(ref get_prev) = query.get_previous_page_param {
                data.pages
                    .first()
                    .and_then(|first| get_prev(first, &data.pages))
                    .is_some()
            } else {
                false
            };
            (has_next, has_prev)
        } else {
            (false, false)
        }
    }

    pub fn state(&self) -> &QueryState<InfiniteData<T, P>> {
        &self.current_state
    }
}

/// Spawn an infinite query and return an observer.
pub fn spawn_infinite_query<T, P, V>(
    cx: &mut Context<V>,
    client: &QueryClient,
    query: &InfiniteQuery<T, P>,
    on_complete: impl FnOnce(&mut V, QueryState<InfiniteData<T, P>>, &mut Context<V>) + 'static,
) -> InfiniteQueryObserver<T, P>
where
    T: Clone + Send + Sync + std::fmt::Debug + 'static,
    P: Clone + Send + Sync + std::fmt::Debug + 'static,
    V: 'static,
{
    let key = query.key.clone();
    let fetch_fn = query.fetch_fn.clone();
    let initial_page_param = query.initial_page_param.clone();
    let options = query.options.clone();
    let client = client.clone();
    let _max_pages = query.max_pages;
    let _get_next = query.get_next_page_param.clone();
    let _get_prev = query.get_previous_page_param.clone();

    let observer = InfiniteQueryObserver::new(&client, query.clone());

    if client.is_in_flight(&key) {
        tracing::trace!(
            target: "rs_query",
            query_key = %key.cache_key(),
            "Infinite query already in flight, skipping duplicate request"
        );
        return observer;
    }

    tracing::debug!(
        target: "rs_query",
        query_key = %key.cache_key(),
        stale_time_ms = ?options.stale_time.as_millis(),
        max_retries = options.retry.max_retries,
        "Starting infinite query"
    );

    client.notify_subscribers(key.cache_key(), QueryStateVariant::Loading);

    let retry_config = options.retry.clone();
    let key_for_task = key.cache_key().to_string();

    let task = tokio::spawn(execute_with_retry(
        fetch_fn,
        initial_page_param,
        retry_config,
        key_for_task.clone(),
    ));
    let abort_handle = task.abort_handle();
    client.set_abort_handle(&key, abort_handle);

    let gpui_task = cx.background_executor().spawn(task);

    let key_for_cleanup = key.clone();
    let client_for_cleanup = client.clone();

    cx.spawn(async move |this, cx| {
        let result = gpui_task.await;
        client_for_cleanup.clear_abort_handle(&key_for_cleanup);

        let state = match result {
            Ok(Ok((page, page_param))) => {
                tracing::debug!(
                    target: "rs_query",
                    query_key = %key_for_cleanup.cache_key(),
                    "Infinite query initial page completed"
                );
                let data = InfiniteData {
                    pages: vec![page],
                    page_params: vec![page_param],
                };
                client_for_cleanup.set_infinite_data(&key_for_cleanup, data, options);
                client_for_cleanup
                    .notify_subscribers(key_for_cleanup.cache_key(), QueryStateVariant::Success);
                QueryState::Success(client_for_cleanup.get_infinite_data(&key_for_cleanup).unwrap())
            }
            Ok(Err(e)) => {
                tracing::warn!(
                    target: "rs_query",
                    query_key = %key_for_cleanup.cache_key(),
                    error = %e,
                    "Infinite query failed"
                );
                client_for_cleanup
                    .notify_subscribers(key_for_cleanup.cache_key(), QueryStateVariant::Error);
                QueryState::Error {
                    error: e,
                    stale_data: client_for_cleanup.get_infinite_data(&key_for_cleanup),
                }
            }
            Err(join_error) => {
                let error = QueryError::Custom(format!("Task cancelled: {}", join_error));
                tracing::warn!(
                    target: "rs_query",
                    query_key = %key_for_cleanup.cache_key(),
                    error = %error,
                    "Infinite query cancelled"
                );
                client_for_cleanup
                    .notify_subscribers(key_for_cleanup.cache_key(), QueryStateVariant::Error);
                QueryState::Error {
                    error,
                    stale_data: client_for_cleanup.get_infinite_data(&key_for_cleanup),
                }
            }
        };

        let _ = this.update(cx, |this, cx| {
            on_complete(this, state, cx);
        });
    })
    .detach();

    observer
}

async fn execute_with_retry<T, P>(
    fetch_fn: Arc<
        dyn Fn(P) -> std::pin::Pin<Box<dyn Future<Output = Result<T, QueryError>> + Send>>
            + Send
            + Sync,
    >,
    page_param: P,
    retry_config: RetryConfig,
    query_key: String,
) -> Result<(T, P), QueryError>
where
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + Sync + 'static,
{
    let mut attempts = 0;
    let mut last_error = None;
    let current_param = page_param;

    while attempts <= retry_config.max_retries {
        match (fetch_fn)(current_param.clone()).await {
            Ok(data) => {
                if attempts > 0 {
                    tracing::debug!(
                        target: "rs_query",
                        query_key = %query_key,
                        attempts = attempts + 1,
                        "Infinite query page succeeded after retry"
                    );
                }
                return Ok((data, current_param));
            }
            Err(e) => {
                if !e.is_retryable() || attempts >= retry_config.max_retries {
                    if attempts > 0 {
                        tracing::debug!(
                            target: "rs_query",
                            query_key = %query_key,
                            attempts = attempts + 1,
                            error = %e,
                            "Infinite query failed after all retries"
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
                    "Retrying infinite query page after error"
                );

                last_error = Some(e);
                Delay::new(delay).await;
            }
        }
    }

    Err(last_error.unwrap_or(QueryError::Custom(
        "Max retries exceeded".into(),
    )))
}
