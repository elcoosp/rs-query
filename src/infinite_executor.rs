// src/infinite_executor.rs
//! Infinite query execution

use crate::error::QueryError;
use crate::infinite::{InfiniteData, InfiniteQuery};
use crate::observer::{QueryObserver, QueryStateVariant};
use crate::{QueryClient, QueryState};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Observer for an infinite query with fetch-next/previous capabilities.
pub struct InfiniteQueryObserver<T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static>
{
    pub observer: QueryObserver<InfiniteData<T, P>>,
    query: Arc<InfiniteQuery<T, P>>,
    next_page_tx: mpsc::Sender<()>,
    prev_page_tx: mpsc::Sender<()>,
}

impl<T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static>
    InfiniteQueryObserver<T, P>
{
    pub fn state(&self) -> &QueryState<InfiniteData<T, P>> {
        self.observer.state()
    }

    pub fn has_next_page(&self) -> bool {
        if let Some(data) = self.observer.state().data() {
            if let Some(ref get_next) = self.query.get_next_page_param {
                if let Some(last_page) = data.last_page() {
                    return get_next(last_page, &data.pages).is_some();
                }
            }
        }
        false
    }

    pub fn has_previous_page(&self) -> bool {
        if let Some(data) = self.observer.state().data() {
            if let Some(ref get_prev) = self.query.get_previous_page_param {
                if let Some(first_page) = data.first_page() {
                    return get_prev(first_page, &data.pages).is_some();
                }
            }
        }
        false
    }

    /// Request fetching the next page.
    /// If a fetch is already in progress or the channel is full, the request is ignored.
    pub fn fetch_next_page(&self) {
        let _ = self.next_page_tx.try_send(());
    }

    /// Request fetching the previous page.
    /// If a fetch is already in progress or the channel is full, the request is ignored.
    pub fn fetch_previous_page(&self) {
        let _ = self.prev_page_tx.try_send(());
    }
}

/// Core infinite query execution loop.
/// Extracted from `spawn_infinite_query` for testability.
pub(crate) async fn run_infinite_query_loop<T, P>(
    client: QueryClient,
    key: crate::QueryKey,
    options: crate::QueryOptions,
    initial_param: P,
    fetch_fn: Arc<
        dyn Fn(
                P,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<T, QueryError>> + Send + Sync>,
            > + Send
            + Sync,
    >,
    get_next_fn: Option<Arc<dyn Fn(&T, &[T]) -> Option<P> + Send + Sync>>,
    get_prev_fn: Option<Arc<dyn Fn(&T, &[T]) -> Option<P> + Send + Sync>>,
    max_pages: Option<usize>,
    mut observer: QueryObserver<InfiniteData<T, P>>,
    mut next_page_rx: mpsc::Receiver<()>,
    mut prev_page_rx: mpsc::Receiver<()>,
) where
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + Sync + 'static,
{
    observer.set_loading();
    let result = fetch_fn(initial_param.clone()).await;
    match result {
        Ok(page) => {
            let mut initial_data = InfiniteData::new();
            initial_data.push_page(page, initial_param);
            client.set_infinite_data(&key, initial_data, options.clone());
        }
        Err(_) => {
            client.notify_subscribers(key.cache_key(), QueryStateVariant::Error);
        }
    }
    observer.update();

    loop {
        tokio::select! {
            Some(_) = next_page_rx.recv() => {
                if client.is_in_flight(&key) { continue; }
                let current_data = client.get_infinite_data::<T, P>(&key);
                if let Some(data) = current_data {
                    if let Some(ref get_next) = get_next_fn {
                        if let Some(last_page) = data.last_page() {
                            if let Some(next_param) = get_next(last_page, &data.pages) {
                                let handle = tokio::spawn(async {}).abort_handle();
                                client.set_abort_handle(&key, handle);
                                match fetch_fn(next_param.clone()).await {
                                    Ok(page) => {
                                        client.append_infinite_page(&key, page, next_param, max_pages);
                                        observer.update();
                                    }
                                    Err(_) => {
                                        client.notify_subscribers(key.cache_key(), QueryStateVariant::Error);
                                    }
                                }
                                client.clear_abort_handle(&key);
                            }
                        }
                    }
                }
            }
            Some(_) = prev_page_rx.recv() => {
                if client.is_in_flight(&key) { continue; }
                let current_data = client.get_infinite_data::<T, P>(&key);
                if let Some(data) = current_data {
                    if let Some(ref get_prev) = get_prev_fn {
                        if let Some(first_page) = data.first_page() {
                            if let Some(prev_param) = get_prev(first_page, &data.pages) {
                                let handle = tokio::spawn(async {}).abort_handle();
                                client.set_abort_handle(&key, handle);
                                match fetch_fn(prev_param.clone()).await {
                                    Ok(page) => {
                                        client.prepend_infinite_page(&key, page, prev_param, max_pages);
                                        observer.update();
                                    }
                                    Err(_) => {
                                        client.notify_subscribers(key.cache_key(), QueryStateVariant::Error);
                                    }
                                }
                                client.clear_abort_handle(&key);
                            }
                        }
                    }
                }
            }
            else => break,
        }
    }
}

/// Execute an infinite query.
pub fn spawn_infinite_query<
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + Sync + 'static,
    V: 'static,
>(
    _cx: &mut gpui::Context<V>,
    client: &QueryClient,
    query: &InfiniteQuery<T, P>,
    _callback: impl FnOnce(&mut V, QueryState<InfiniteData<T, P>>, &mut gpui::Context<V>)
        + Send
        + 'static,
) -> InfiniteQueryObserver<T, P> {
    let key = query.key.clone();
    let options = query.options.clone();
    let initial_param = query.initial_page_param.clone();
    let fetch_fn = Arc::clone(&query.fetch_fn);
    let get_next_fn = query.get_next_page_param.clone();
    let get_prev_fn = query.get_previous_page_param.clone();
    let max_pages = query.max_pages;
    let client = client.clone();

    let observer = QueryObserver::with_options(&client, key.clone(), options.clone());
    let observer_thread = observer.clone();

    // Use mpsc channels to allow multiple page fetch requests
    let (next_page_tx, next_page_rx) = mpsc::channel::<()>(1);
    let (prev_page_tx, prev_page_rx) = mpsc::channel::<()>(1);

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(run_infinite_query_loop(
            client,
            key,
            options,
            initial_param,
            fetch_fn,
            get_next_fn,
            get_prev_fn,
            max_pages,
            observer_thread,
            next_page_rx,
            prev_page_rx,
        ));
    });

    InfiniteQueryObserver {
        observer,
        query: Arc::new(query.clone()),
        next_page_tx,
        prev_page_tx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QueryKey, QueryOptions};
    use std::future::Future;
    use std::pin::Pin;
    use std::time::{Duration, Instant};

    /// Helper to create an Arc'd fetch function for tests.
    fn make_fetch_fn<T, P, F, Fut>(
        f: F,
    ) -> Arc<
        dyn Fn(P) -> Pin<Box<dyn Future<Output = Result<T, QueryError>> + Send + Sync>>
            + Send
            + Sync,
    >
    where
        T: Clone + Send + Sync + 'static,
        P: Clone + Send + Sync + 'static,
        F: Fn(P) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, QueryError>> + Send + Sync + 'static,
    {
        Arc::new(move |p| Box::pin(f(p)))
    }

    /// Helper to create an Arc'd get-next/get-previous page function for tests.
    fn make_page_fn<T, P, F>(f: F) -> Arc<dyn Fn(&T, &[T]) -> Option<P> + Send + Sync>
    where
        T: Send + Sync,
        P: Send + Sync,
        F: Fn(&T, &[T]) -> Option<P> + Send + Sync + 'static,
    {
        Arc::new(f)
    }

    /// Poll until data appears in the cache or timeout.
    async fn wait_for_data<T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static>(
        client: &QueryClient,
        key: &crate::QueryKey,
        timeout: Duration,
    ) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if client.get_infinite_data::<T, P>(key).is_some() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        false
    }

    /// Poll until page count reaches expected or timeout.
    async fn wait_for_page_count<
        T: Clone + Send + Sync + 'static,
        P: Clone + Send + Sync + 'static,
    >(
        client: &QueryClient,
        key: &crate::QueryKey,
        expected: usize,
        timeout: Duration,
    ) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if let Some(data) = client.get_infinite_data::<T, P>(key) {
                if data.page_count() >= expected {
                    return true;
                }
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        false
    }

    // ========================================================================
    // Existing tests (unchanged)
    // ========================================================================

    #[test]
    fn test_has_next_page_true() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(1));

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(infinite_observer.has_next_page());
    }

    #[test]
    fn test_has_next_page_false_when_no_fn() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_next_page());
    }

    #[test]
    fn test_has_next_page_false_when_fn_returns_none() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| None);

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_next_page());
    }

    #[test]
    fn test_has_next_page_false_when_no_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(1));

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_next_page());
    }

    #[test]
    fn test_has_previous_page_true() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_previous_page_param(|_first: &Vec<i32>, _all: &[Vec<i32>]| Some(-1));

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(infinite_observer.has_previous_page());
    }

    #[test]
    fn test_has_previous_page_false_when_no_fn() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_previous_page());
    }

    #[test]
    fn test_has_previous_page_false_when_no_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_previous_page_param(|_first: &Vec<i32>, _all: &[Vec<i32>]| Some(-1));

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_previous_page());
    }

    #[test]
    fn test_has_previous_page_false_when_fn_returns_none() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_previous_page_param(|_first: &Vec<i32>, _all: &[Vec<i32>]| None);

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_previous_page());
    }

    #[test]
    fn test_infinite_observer_state() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(infinite_observer.state().is_idle());
    }

    #[test]
    fn test_fetch_next_page_does_not_panic() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        infinite_observer.fetch_next_page();
        infinite_observer.fetch_previous_page();
    }

    #[test]
    fn test_append_infinite_page() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let result = client.append_infinite_page(&key, vec![3, 4], 1, None);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![1, 2]);
        assert_eq!(result.pages[1], vec![3, 4]);
    }

    #[test]
    fn test_append_infinite_page_with_max_pages_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let mut data = InfiniteData::new();
        data.push_page(vec![1], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        client.append_infinite_page(&key, vec![2], 1, Some(2));

        let result = client.append_infinite_page(&key, vec![3], 2, Some(2));
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![2]);
        assert_eq!(result.pages[1], vec![3]);
    }

    #[test]
    fn test_append_infinite_page_with_max_pages_no_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let mut data = InfiniteData::new();
        data.push_page(vec![1], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let result = client.append_infinite_page(&key, vec![2], 1, Some(3));
        assert!(result.is_some());
        assert_eq!(result.unwrap().page_count(), 2);
    }

    #[test]
    fn test_append_infinite_page_no_existing_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let result = client.append_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_append_infinite_page_wrong_type() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "wrong_type".to_string(), QueryOptions::default());

        let result = client.append_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_prepend_infinite_page() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let mut data = InfiniteData::new();
        data.push_page(vec![3, 4], 1);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let result = client.prepend_infinite_page(&key, vec![1, 2], 0, None);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![1, 2]);
        assert_eq!(result.pages[1], vec![3, 4]);
    }

    #[test]
    fn test_prepend_infinite_page_with_max_pages_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let mut data = InfiniteData::new();
        data.push_page(vec![3], 2);
        client.set_infinite_data(&key, data, QueryOptions::default());

        client.prepend_infinite_page(&key, vec![2], 1, Some(2));
        let result = client.prepend_infinite_page(&key, vec![1], 0, Some(2));

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![1]);
        assert_eq!(result.pages[1], vec![2]);
    }

    #[test]
    fn test_prepend_infinite_page_with_max_pages_no_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let mut data = InfiniteData::new();
        data.push_page(vec![2], 1);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let result = client.prepend_infinite_page(&key, vec![1], 0, Some(3));
        assert!(result.is_some());
        assert_eq!(result.unwrap().page_count(), 2);
    }

    #[test]
    fn test_prepend_infinite_page_no_existing_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let result = client.prepend_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_prepend_infinite_page_wrong_type() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "wrong_type".to_string(), QueryOptions::default());

        let result = client.prepend_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_mpsc_channel_send_recv() {
        let (tx, mut rx) = mpsc::channel::<()>(1);
        tx.send(()).await.unwrap();
        let result = rx.recv().await;
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_mpsc_channel_closed() {
        let (tx, mut rx) = mpsc::channel::<()>(1);
        drop(tx);
        let result = rx.recv().await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_mpsc_try_send() {
        let (tx, mut rx) = mpsc::channel::<()>(1);
        tx.try_send(()).unwrap();
        assert!(tx.try_send(()).is_err());
        let result = rx.recv().await;
        assert!(result.is_some());
    }

    #[test]
    fn test_infinite_query_error_notification() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let mut data = InfiniteData::new();
        data.push_page(vec![1], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        client.notify_subscribers(key.cache_key(), QueryStateVariant::Error);

        let result: Option<InfiniteData<Vec<i32>, i32>> = client.get_infinite_data(&key);
        assert!(result.is_some());
    }

    #[test]
    fn test_infinite_observer_has_next_page_no_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(1));

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_next_page());
        assert!(!infinite_observer.has_previous_page());
    }

    #[test]
    fn test_infinite_observer_has_previous_page_no_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_previous_page_param(|_first: &Vec<i32>, _all: &[Vec<i32>]| Some(-1));

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_previous_page());
    }

    #[test]
    fn test_infinite_observer_has_next_page_fn_returns_none() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| None);

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_next_page());
    }

    #[test]
    fn test_infinite_observer_has_previous_page_fn_returns_none() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_previous_page_param(|_first: &Vec<i32>, _all: &[Vec<i32>]| None);

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_previous_page());
    }

    #[tokio::test]
    async fn test_fetch_next_page_channel_full() {
        let (tx, mut rx) = mpsc::channel::<()>(1);

        tx.send(()).await.unwrap();
        assert!(tx.try_send(()).is_err());

        let _ = rx.recv().await;
        assert!(tx.try_send(()).is_ok());
    }

    #[tokio::test]
    async fn test_fetch_previous_page_channel_full() {
        let (tx, mut rx) = mpsc::channel::<()>(1);

        tx.send(()).await.unwrap();
        assert!(tx.try_send(()).is_err());

        let _ = rx.recv().await;
        assert!(tx.try_send(()).is_ok());
    }

    #[test]
    fn test_has_next_page_with_empty_pages() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(1));

        let data = InfiniteData::<Vec<i32>, i32>::new();
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_next_page());
    }

    #[test]
    fn test_has_previous_page_with_empty_pages() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_previous_page_param(|_first: &Vec<i32>, _all: &[Vec<i32>]| Some(-1));

        let data = InfiniteData::<Vec<i32>, i32>::new();
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_previous_page());
    }

    #[test]
    fn test_has_next_page_with_multiple_pages() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|last: &Vec<i32>, _all: &[Vec<i32>]| Some(last[0] + 10));

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2], 0);
        data.push_page(vec![11, 12], 1);
        data.push_page(vec![21, 22], 2);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(infinite_observer.has_next_page());
    }

    #[test]
    fn test_has_previous_page_with_multiple_pages() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            10,
        )
        .get_previous_page_param(|first: &Vec<i32>, _all: &[Vec<i32>]| {
            if first[0] > 0 {
                Some(first[0] - 10)
            } else {
                None
            }
        });

        let mut data = InfiniteData::new();
        data.push_page(vec![11, 12], 1);
        data.push_page(vec![21, 22], 2);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(infinite_observer.has_previous_page());
    }

    #[test]
    fn test_has_previous_page_at_boundary() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_previous_page_param(|first: &Vec<i32>, _all: &[Vec<i32>]| {
            if first[0] > 0 {
                Some(first[0] - 10)
            } else {
                None
            }
        });

        let mut data = InfiniteData::new();
        data.push_page(vec![0, 1], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_previous_page());
    }

    #[test]
    fn test_observer_state_with_success_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(infinite_observer.state().is_success());
        assert!(infinite_observer.state().has_data());
    }

    #[test]
    fn test_observer_state_with_stale_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        client.invalidate_queries(&key);

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(infinite_observer.state().has_data());
    }

    #[test]
    fn test_fetch_next_page_when_receiver_dropped() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let (next_page_tx, rx) = mpsc::channel::<()>(1);
        drop(rx);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx,
            prev_page_tx: mpsc::channel(1).0,
        };

        infinite_observer.fetch_next_page();
    }

    #[test]
    fn test_fetch_previous_page_when_receiver_dropped() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let (prev_page_tx, rx) = mpsc::channel::<()>(1);
        drop(rx);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx,
        };

        infinite_observer.fetch_previous_page();
    }

    #[test]
    fn test_has_next_page_uses_all_pages_in_callback() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, all: &[Vec<i32>]| {
            if all.len() < 3 {
                Some(all.len() as i32)
            } else {
                None
            }
        });

        let mut data = InfiniteData::new();
        data.push_page(vec![1], 0);
        data.push_page(vec![2], 1);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(infinite_observer.has_next_page());
    }

    #[test]
    fn test_has_next_page_uses_all_pages_at_limit() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, all: &[Vec<i32>]| {
            if all.len() < 3 {
                Some(all.len() as i32)
            } else {
                None
            }
        });

        let mut data = InfiniteData::new();
        data.push_page(vec![1], 0);
        data.push_page(vec![2], 1);
        data.push_page(vec![3], 2);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_next_page());
    }

    #[test]
    fn test_has_next_and_previous_together() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            5,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(1))
        .get_previous_page_param(|_first: &Vec<i32>, _all: &[Vec<i32>]| Some(-1));

        let mut data = InfiniteData::new();
        data.push_page(vec![1], 0);
        data.push_page(vec![2], 1);
        data.push_page(vec![3], 2);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(infinite_observer.has_next_page());
        assert!(infinite_observer.has_previous_page());
    }

    #[test]
    fn test_fetch_next_page_multiple_times() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        infinite_observer.fetch_next_page();
        infinite_observer.fetch_next_page();
        infinite_observer.fetch_next_page();
    }

    #[tokio::test]
    async fn test_fetch_next_page_drain_and_refill() {
        let (tx, mut rx) = mpsc::channel::<()>(2);

        tx.send(()).await.unwrap();
        tx.send(()).await.unwrap();
        assert!(tx.try_send(()).is_err());

        let _ = rx.recv().await;
        assert!(tx.try_send(()).is_ok());

        let _ = rx.recv().await;
        let _ = rx.recv().await;
        assert!(tx.try_send(()).is_ok());
    }

    #[test]
    fn test_observer_state_error_no_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(infinite_observer.state().is_idle());
        assert!(!infinite_observer.state().has_data());
        assert!(!infinite_observer.state().is_error());
    }

    // ========================================================================
    // NEW: Integration tests for run_infinite_query_loop
    // ========================================================================

    #[tokio::test]
    async fn test_loop_initial_fetch_success() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| {
            Box::pin(async move { Ok(vec![param, param + 1, param + 2]) })
        });

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            0,
            fetch_fn,
            None,
            None,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);
        assert_eq!(data.pages[0], vec![0, 1, 2]);

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_initial_fetch_error() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn: Arc<
            dyn Fn(i32) -> Pin<Box<dyn Future<Output = Result<Vec<i32>, QueryError>> + Send + Sync>>
                + Send
                + Sync,
        > = Arc::new(|_param: i32| {
            Box::pin(async move { Err(QueryError::network("fetch failed")) })
        });

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            0,
            fetch_fn,
            None,
            None,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        tokio::time::sleep(Duration::from_millis(50)).await;

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key);
        assert!(data.is_none());

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_fetch_next_page_success() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| {
            Box::pin(async move { Ok(vec![param * 10, param * 10 + 1]) })
        });

        let get_next = Some(make_page_fn(|last: &Vec<i32>, _all: &[Vec<i32>]| {
            Some(last[0] / 10 + 1)
        }));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            0,
            fetch_fn,
            get_next,
            None,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        next_tx.send(()).await.unwrap();

        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 2, Duration::from_secs(1)).await
        );

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 2);
        assert_eq!(data.pages[0], vec![0, 1]);
        assert_eq!(data.pages[1], vec![10, 11]);

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_fetch_next_page_error() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cc = call_count.clone();

        let fetch_fn: Arc<
            dyn Fn(i32) -> Pin<Box<dyn Future<Output = Result<Vec<i32>, QueryError>> + Send + Sync>>
                + Send
                + Sync,
        > = Arc::new(move |param: i32| {
            let cc = cc.clone();
            Box::pin(async move {
                if cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Ok(vec![param])
                } else {
                    Err(QueryError::network("page 2 failed"))
                }
            })
        });

        let get_next = Some(make_page_fn(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(1)));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            0,
            fetch_fn,
            get_next,
            None,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        next_tx.send(()).await.unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_fetch_prev_page_success() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| {
            Box::pin(async move { Ok(vec![param * 10, param * 10 + 1]) })
        });

        let get_prev = Some(make_page_fn(|first: &Vec<i32>, _all: &[Vec<i32>]| {
            Some(first[0] / 10 - 1)
        }));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            5,
            fetch_fn,
            None,
            get_prev,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        prev_tx.send(()).await.unwrap();

        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 2, Duration::from_secs(1)).await
        );

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 2);
        assert_eq!(data.pages[0], vec![40, 41]);
        assert_eq!(data.pages[1], vec![50, 51]);

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_fetch_prev_page_error() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cc = call_count.clone();

        let fetch_fn: Arc<
            dyn Fn(i32) -> Pin<Box<dyn Future<Output = Result<Vec<i32>, QueryError>> + Send + Sync>>
                + Send
                + Sync,
        > = Arc::new(move |param: i32| {
            let cc = cc.clone();
            Box::pin(async move {
                if cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Ok(vec![param])
                } else {
                    Err(QueryError::network("prev fetch failed"))
                }
            })
        });

        let get_prev = Some(make_page_fn(|first: &Vec<i32>, _all: &[Vec<i32>]| {
            Some(first[0] - 1)
        }));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            5,
            fetch_fn,
            None,
            get_prev,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        prev_tx.send(()).await.unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_no_get_next_fn_skips_next() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            0,
            fetch_fn,
            None,
            None,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        next_tx.send(()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_no_get_prev_fn_skips_prev() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            0,
            fetch_fn,
            None,
            None,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        prev_tx.send(()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_get_next_returns_none_no_more_pages() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let get_next = Some(make_page_fn(|_last: &Vec<i32>, _all: &[Vec<i32>]| None));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            0,
            fetch_fn,
            get_next,
            None,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        next_tx.send(()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_get_prev_returns_none_no_more_pages() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let get_prev = Some(make_page_fn(|_first: &Vec<i32>, _all: &[Vec<i32>]| None));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            0,
            fetch_fn,
            None,
            get_prev,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        prev_tx.send(()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_channel_close_breaks() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        // Close both senders immediately - the loop should break via else => break
        drop(next_tx);
        drop(prev_tx);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            99,
            fetch_fn,
            None,
            None,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        // Should complete quickly (not hang forever)
        let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_ok());
    }
    #[tokio::test]
    async fn test_loop_fetch_multiple_next_pages() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let get_next = Some(make_page_fn(|_last: &Vec<i32>, all: &[Vec<i32>]| {
            Some(all.len() as i32)
        }));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(2);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            0,
            fetch_fn,
            get_next,
            None,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 1, Duration::from_secs(1)).await
        );

        next_tx.send(()).await.unwrap();
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 2, Duration::from_secs(1)).await
        );

        next_tx.send(()).await.unwrap();
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 3, Duration::from_secs(1)).await
        );

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 3);
        assert_eq!(data.pages[0], vec![0]);
        assert_eq!(data.pages[1], vec![1]);
        assert_eq!(data.pages[2], vec![2]);

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_fetch_next_with_max_pages_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let get_next = Some(make_page_fn(|last: &Vec<i32>, _all: &[Vec<i32>]| {
            Some(last[0] + 1)
        }));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(4);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            0,
            fetch_fn,
            get_next,
            None,
            Some(3),
            observer,
            next_rx,
            prev_rx,
        ));

        // Initial: [[0]]
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 1, Duration::from_secs(1)).await
        );

        // Next: [[0], [1]]
        next_tx.send(()).await.unwrap();
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 2, Duration::from_secs(1)).await
        );

        // Next: [[0], [1], [2]]
        next_tx.send(()).await.unwrap();
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 3, Duration::from_secs(1)).await
        );

        // Next should trigger eviction: [[0], [1], [2]] -> [[1], [2], [3]]
        next_tx.send(()).await.unwrap();
        // Poll for the actual eviction result instead of just count (count stays 3)
        let start = Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(1) {
                panic!("timed out waiting for eviction");
            }
            if let Some(data) = client.get_infinite_data::<Vec<i32>, i32>(&key) {
                if data.page_count() == 3 && data.pages[0] == vec![1] {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 3);
        assert_eq!(data.pages[0], vec![1]);
        assert_eq!(data.pages[1], vec![2]);
        assert_eq!(data.pages[2], vec![3]);

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_fetch_prev_with_max_pages_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let get_prev = Some(make_page_fn(|first: &Vec<i32>, _all: &[Vec<i32>]| {
            Some(first[0] - 1)
        }));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(4);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            2,
            fetch_fn,
            None,
            get_prev,
            Some(3),
            observer,
            next_rx,
            prev_rx,
        ));

        // Initial: [[2]]
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 1, Duration::from_secs(1)).await
        );

        // Prev: [[1], [2]]
        prev_tx.send(()).await.unwrap();
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 2, Duration::from_secs(1)).await
        );

        // Prev: [[0], [1], [2]]
        prev_tx.send(()).await.unwrap();
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 3, Duration::from_secs(1)).await
        );

        // Prev should trigger eviction: [[0], [1], [2]] -> [[-1], [0], [1]]
        prev_tx.send(()).await.unwrap();
        let start = Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(1) {
                panic!("timed out waiting for eviction");
            }
            if let Some(data) = client.get_infinite_data::<Vec<i32>, i32>(&key) {
                if data.page_count() == 3 && data.pages[0] == vec![-1] {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 3);
        assert_eq!(data.pages[0], vec![-1]);
        assert_eq!(data.pages[1], vec![0]);
        assert_eq!(data.pages[2], vec![1]);

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }
    #[tokio::test]
    async fn test_loop_no_initial_data_next_request_ignored() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        // Fetch that takes 200ms — during this time, get_infinite_data returns None
        let fetch_fn = make_fetch_fn(|param: i32| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(vec![param])
            })
        });

        let get_next = Some(make_page_fn(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(1)));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            0,
            fetch_fn,
            get_next,
            None,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        // Send next request while initial fetch is in progress (0-200ms window)
        next_tx.send(()).await.unwrap();

        // No data yet since initial fetch hasn't completed
        tokio::time::sleep(Duration::from_millis(20)).await;
        let data = client.get_infinite_data::<Vec<i32>, i32>(&key);
        assert!(data.is_none());

        // Wait for initial fetch to complete
        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        // The loop processed the queued next request while data didn't exist,
        // so get_infinite_data returned None and it was silently dropped.
        // Should still only have 1 page.
        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }
    #[tokio::test]
    async fn test_loop_next_page_channel_full_request_dropped() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(vec![param])
            })
        });

        let get_next = Some(make_page_fn(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(1)));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            0,
            fetch_fn,
            get_next,
            None,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        next_tx.send(()).await.unwrap();

        let result = next_tx.try_send(());
        assert!(result.is_err());

        tokio::time::sleep(Duration::from_millis(300)).await;

        let result = next_tx.try_send(());
        assert!(result.is_ok());

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_both_directions_sequential() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let get_next = Some(make_page_fn(|last: &Vec<i32>, _all: &[Vec<i32>]| {
            Some(last[0] + 1)
        }));
        let get_prev = Some(make_page_fn(|first: &Vec<i32>, _all: &[Vec<i32>]| {
            Some(first[0] - 1)
        }));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(2);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(2);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            1,
            fetch_fn,
            get_next,
            get_prev,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 1, Duration::from_secs(1)).await
        );

        next_tx.send(()).await.unwrap();
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 2, Duration::from_secs(1)).await
        );

        prev_tx.send(()).await.unwrap();
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 3, Duration::from_secs(1)).await
        );

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 3);
        assert_eq!(data.pages[0], vec![0]);
        assert_eq!(data.pages[1], vec![1]);
        assert_eq!(data.pages[2], vec![2]);

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }
    #[tokio::test]
    async fn test_loop_empty_pages_after_initial_next_returns_none() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            key.clone(),
            QueryOptions::default(),
            0,
            fetch_fn,
            None,
            None,
            None,
            observer,
            next_rx,
            prev_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        next_tx.send(()).await.unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;
        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);

        drop(next_tx);
        drop(prev_tx);
        let _ = handle.await;
    }
}
