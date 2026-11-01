// src/infinite_executor.rs
//! Infinite query execution

use crate::client::InFlightTask;
use crate::infinite::{InfiniteData, InfiniteQuery, PageFetchFn, PageParamFn};
use crate::observer::{QueryObserver, QueryStateVariant};
use crate::{QueryClient, QueryState};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Direction marker for forward (next-page) fetch requests.
const DIRECTION_FORWARD: u8 = 0;
/// Direction marker for backward (previous-page) fetch requests.
const DIRECTION_BACKWARD: u8 = 1;

/// Observer for an infinite query with fetch-next/previous capabilities.
pub struct InfiniteQueryObserver<T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static>
{
    pub observer: QueryObserver<InfiniteData<T, P>>,
    query: Arc<InfiniteQuery<T, P>>,
    next_page_tx: mpsc::Sender<()>,
    prev_page_tx: mpsc::Sender<()>,
    /// Direction of the most recent page-fetch request; stands in for
    /// `TanStack`'s `fetchMeta.fetchMore.direction` (only one page fetch can be
    /// in flight at a time in this port).
    fetch_direction: Arc<AtomicU8>,
}

impl<T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static>
    InfiniteQueryObserver<T, P>
{
    pub fn state(&self) -> &QueryState<InfiniteData<T, P>> {
        self.observer.state()
    }

    pub fn has_next_page(&self) -> bool {
        if let Some(data) = self.observer.state().data()
            && let Some(ref get_next) = self.query.get_next_page_param
            && let Some(last_page) = data.last_page()
        {
            return get_next(last_page, &data.pages).is_some();
        }
        false
    }

    pub fn has_previous_page(&self) -> bool {
        if let Some(data) = self.observer.state().data()
            && let Some(ref get_prev) = self.query.get_previous_page_param
            && let Some(first_page) = data.first_page()
        {
            return get_prev(first_page, &data.pages).is_some();
        }
        false
    }

    /// Request fetching the next page.
    pub fn fetch_next_page(&self) {
        self.fetch_direction
            .store(DIRECTION_FORWARD, Ordering::Relaxed);
        let _ = self.next_page_tx.try_send(());
    }

    /// Request fetching the previous page.
    pub fn fetch_previous_page(&self) {
        self.fetch_direction
            .store(DIRECTION_BACKWARD, Ordering::Relaxed);
        let _ = self.prev_page_tx.try_send(());
    }

    /// Whether a page fetch (either direction) is currently in flight.
    pub fn is_fetching(&self) -> bool {
        self.observer.client().is_in_flight(&self.query.key)
    }

    /// Whether the in-flight fetch is a forward page fetch (`TanStack`
    /// `isFetchingNextPage`); the direction is the one of the most recent
    /// page-fetch request.
    pub fn is_fetching_next_page(&self) -> bool {
        self.is_fetching() && self.fetch_direction.load(Ordering::Relaxed) == DIRECTION_FORWARD
    }

    /// Whether the in-flight fetch is a backward page fetch (`TanStack`
    /// `isFetchingPreviousPage`).
    pub fn is_fetching_previous_page(&self) -> bool {
        self.is_fetching() && self.fetch_direction.load(Ordering::Relaxed) == DIRECTION_BACKWARD
    }

    /// TanStack-style `fetchNextPage({ cancelRefetch, throwOnError })`.
    ///
    /// With `cancel_refetch` (the `TanStack` default) a running page fetch is
    /// cancelled through the client first so the loop accepts the new
    /// request; with `cancel_refetch == false` an in-flight fetch is reused
    /// and the request is dropped (this port has no shared promise to
    /// return). `throw_on_error` is accepted for API parity: page-fetch
    /// errors surface through the query key's error notification inside
    /// `run_infinite_query_loop`, not through a thrown value.
    pub fn fetch_next_page_with(&self, cancel_refetch: bool, throw_on_error: bool) {
        self.fetch_page_with(cancel_refetch, throw_on_error, true);
    }

    /// TanStack-style `fetchPreviousPage({ cancelRefetch, throwOnError })`.
    pub fn fetch_previous_page_with(&self, cancel_refetch: bool, throw_on_error: bool) {
        self.fetch_page_with(cancel_refetch, throw_on_error, false);
    }

    fn fetch_page_with(&self, cancel_refetch: bool, throw_on_error: bool, forward: bool) {
        let _ = throw_on_error; // errors surface via the Error notification (see run_infinite_query_loop)
        if self.is_fetching() {
            if !cancel_refetch {
                return;
            }
            self.observer.client().cancel_query(&self.query.key);
        }
        if forward {
            self.fetch_next_page();
        } else {
            self.fetch_previous_page();
        }
    }
}

/// Bundled inputs for [`run_infinite_query_loop`], grouping the query
/// definition pieces (key, options, initial page param and the fetch /
/// page-param functions) so the loop signature stays lean.
pub(crate) struct InfiniteLoopParams<T, P> {
    key: crate::QueryKey,
    options: crate::QueryOptions,
    initial_param: P,
    fetch_fn: PageFetchFn<T, P>,
    get_next_fn: Option<PageParamFn<T, P>>,
    get_prev_fn: Option<PageParamFn<T, P>>,
    max_pages: Option<usize>,
}

/// Core infinite query execution loop.
pub(crate) async fn run_infinite_query_loop<T, P>(
    client: QueryClient,
    params: InfiniteLoopParams<T, P>,
    mut observer: QueryObserver<InfiniteData<T, P>>,
    mut next_page_rx: mpsc::Receiver<()>,
    mut prev_page_rx: mpsc::Receiver<()>,
) where
    T: Clone + Send + Sync + 'static + PartialEq,
    P: Clone + Send + Sync + 'static + PartialEq,
{
    let InfiniteLoopParams {
        key,
        options,
        initial_param,
        fetch_fn,
        get_next_fn,
        get_prev_fn,
        max_pages,
    } = params;

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
            Some(()) = next_page_rx.recv() => {
                if client.is_in_flight(&key) { continue; }
                let current_data = client.get_infinite_data::<T, P>(&key);
                if let Some(data) = current_data
                    && let Some(ref get_next) = get_next_fn
                        && let Some(last_page) = data.last_page()
                            && let Some(next_param) = get_next(last_page, &data.pages) {
                                let handle = tokio::spawn(async {}).abort_handle();
                                let token = CancellationToken::new();
                                client.set_in_flight(&key, InFlightTask {
                                    abort_handle: handle,
                                    cancel_token: token,
                                });
                                match fetch_fn(next_param.clone()).await {
                                    Ok(page) => {
                                        client.append_infinite_page(&key, page, next_param, max_pages);
                                        observer.update();
                                    }
                                    Err(_) => {
                                        client.notify_subscribers(key.cache_key(), QueryStateVariant::Error);
                                    }
                                }
                                client.clear_in_flight(&key);
                            }
            }
            Some(()) = prev_page_rx.recv() => {
                if client.is_in_flight(&key) { continue; }
                let current_data = client.get_infinite_data::<T, P>(&key);
                if let Some(data) = current_data
                    && let Some(ref get_prev) = get_prev_fn
                        && let Some(first_page) = data.first_page()
                            && let Some(prev_param) = get_prev(first_page, &data.pages) {
                                let handle = tokio::spawn(async {}).abort_handle();
                                let token = CancellationToken::new();
                                client.set_in_flight(&key, InFlightTask {
                                    abort_handle: handle,
                                    cancel_token: token,
                                });
                                match fetch_fn(prev_param.clone()).await {
                                    Ok(page) => {
                                        client.prepend_infinite_page(&key, page, prev_param, max_pages);
                                        observer.update();
                                    }
                                    Err(_) => {
                                        client.notify_subscribers(key.cache_key(), QueryStateVariant::Error);
                                    }
                                }
                                client.clear_in_flight(&key);
                            }
            }
            else => break,
        }
    }
}

/// Execute an infinite query.
///
/// # Panics
///
/// Panics if the background current-thread Tokio runtime cannot be built
/// (`Builder::build().unwrap()`), which is practically unreachable in a
/// healthy host process.
pub fn spawn_infinite_query<
    T: Clone + Send + Sync + 'static + PartialEq,
    P: Clone + Send + Sync + 'static + PartialEq,
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
    let params = InfiniteLoopParams {
        key: key.clone(),
        options: options.clone(),
        initial_param: query.initial_page_param.clone(),
        fetch_fn: Arc::clone(&query.fetch_fn),
        get_next_fn: query.get_next_page_param.clone(),
        get_prev_fn: query.get_previous_page_param.clone(),
        max_pages: query.max_pages,
    };
    let client = client.clone();

    let observer = QueryObserver::with_options(&client, key, options);
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
            params,
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
        fetch_direction: Arc::new(AtomicU8::new(DIRECTION_FORWARD)),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::QueryError;
    use crate::{QueryKey, QueryOptions};
    use std::future::Future;
    use std::time::{Duration, Instant};

    /// Helper to create an Arc'd fetch function for tests.
    fn make_fetch_fn<T, P, F, Fut>(f: F) -> PageFetchFn<T, P>
    where
        T: Clone + Send + Sync + 'static,
        P: Clone + Send + Sync + 'static,
        F: Fn(P) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, QueryError>> + Send + Sync + 'static,
    {
        Arc::new(move |p| Box::pin(f(p)))
    }

    /// Helper to create an Arc'd get-next/get-previous page function for tests.
    fn make_page_fn<T, P, F>(f: F) -> PageParamFn<T, P>
    where
        T: Send + Sync,
        P: Send + Sync,
        F: Fn(&T, &[T]) -> Option<P> + Send + Sync + 'static,
    {
        Arc::new(f)
    }

    /// Run the query's fetch closure once on a throwaway runtime.
    ///
    /// Several observer-level unit tests build an [`InfiniteQuery`] whose fetch
    /// future is never awaited; calling it once here exercises the test-defined
    /// closure without changing what the test asserts.
    fn fetch_initial<T, P>(query: &InfiniteQuery<T, P>)
    where
        T: Clone + Send + Sync + 'static,
        P: Clone + Send + Sync + 'static,
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        assert!(
            rt.block_on((query.fetch_fn)(query.initial_page_param.clone()))
                .is_ok()
        );
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
            if let Some(data) = client.get_infinite_data::<T, P>(key)
                && data.page_count() >= expected
            {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        false
    }

    #[tokio::test]
    async fn test_wait_helpers_return_false_on_timeout() {
        let client = QueryClient::new();
        let key = QueryKey::new("never-fetched");
        assert!(
            !wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_millis(10)).await,
            "nothing ever lands in the cache"
        );
        assert!(
            !wait_for_page_count::<Vec<i32>, i32>(&client, &key, 1, Duration::from_millis(10))
                .await,
            "no pages ever arrive"
        );
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        // Exercise the None branch of the param closure directly (first page
        // already at the boundary).
        let get_prev = query.get_previous_page_param.as_ref().unwrap();
        assert_eq!(get_prev(&vec![0], &[vec![0]]), None);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        // Exercise the Some branch of the param closure directly (first page
        // strictly above the boundary).
        let get_prev = query.get_previous_page_param.as_ref().unwrap();
        assert_eq!(get_prev(&vec![10], &[vec![10]]), Some(0));
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let (next_page_tx, rx) = mpsc::channel::<()>(1);
        drop(rx);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let (prev_page_tx, rx) = mpsc::channel::<()>(1);
        drop(rx);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx,
            fetch_direction: std::sync::Arc::default(),
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
                Some(i32::try_from(all.len()).unwrap_or(i32::MAX))
            } else {
                None
            }
        });

        let mut data = InfiniteData::new();
        data.push_page(vec![1], 0);
        data.push_page(vec![2], 1);
        client.set_infinite_data(&key, data, QueryOptions::default());

        fetch_initial(&query);
        // Exercise the None branch of the param closure directly (the page
        // limit is reached).
        let get_next = query.get_next_page_param.as_ref().unwrap();
        let three = [vec![1], vec![2], vec![3]];
        assert_eq!(get_next(&three[0], &three), None);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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
                Some(i32::try_from(all.len()).unwrap_or(i32::MAX))
            } else {
                None
            }
        });

        let mut data = InfiniteData::new();
        data.push_page(vec![1], 0);
        data.push_page(vec![2], 1);
        data.push_page(vec![3], 2);
        client.set_infinite_data(&key, data, QueryOptions::default());

        fetch_initial(&query);
        // Exercise the Some branch of the param closure directly (below the
        // page limit).
        let get_next = query.get_next_page_param.as_ref().unwrap();
        let one = [vec![1]];
        assert_eq!(get_next(&one[0], &one), Some(1));
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
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

        fetch_initial(&query);
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
        };

        assert!(infinite_observer.state().is_idle());
        assert!(!infinite_observer.state().has_data());
        assert!(!infinite_observer.state().is_error());
    }

    // ========================================================================
    // NEW: Integration tests for run_infinite_query_loop
    // ========================================================================

    #[test]
    fn test_infinite_query_has_next_page_on_query_definition() {
        // InfiniteQuery::has_next_page/has_previous_page wired through the
        // observer's cached data.
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(1));

        fetch_initial(&query);

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        assert!(query.has_next_page(Some(&data)));
        assert!(!query.has_next_page(None));
    }

    #[tokio::test]
    async fn test_is_fetching_flags_track_in_flight_direction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let _ = (query.fetch_fn)(0).await.unwrap();
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
        };

        assert!(!infinite_observer.is_fetching());
        assert!(!infinite_observer.is_fetching_next_page());
        assert!(!infinite_observer.is_fetching_previous_page());

        // Simulate an in-flight page fetch; the direction comes from the
        // most recent request.
        let handle = tokio::spawn(async {});
        client.set_in_flight(
            &key,
            InFlightTask {
                abort_handle: handle.abort_handle(),
                cancel_token: CancellationToken::new(),
            },
        );
        // Default direction is forward.
        assert!(infinite_observer.is_fetching());
        assert!(infinite_observer.is_fetching_next_page());
        assert!(!infinite_observer.is_fetching_previous_page());

        // A previous-page request flips the direction.
        infinite_observer.fetch_previous_page();
        assert!(infinite_observer.is_fetching());
        assert!(infinite_observer.is_fetching_previous_page());
        assert!(!infinite_observer.is_fetching_next_page());

        client.clear_in_flight(&key);
        assert!(!infinite_observer.is_fetching());
        assert!(!infinite_observer.is_fetching_next_page());
        assert!(!infinite_observer.is_fetching_previous_page());
    }

    #[tokio::test]
    async fn test_fetch_next_page_with_cancels_in_flight() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let _ = (query.fetch_fn)(0).await.unwrap();
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
        };

        let handle = tokio::spawn(async {});
        client.set_in_flight(
            &key,
            InFlightTask {
                abort_handle: handle.abort_handle(),
                cancel_token: CancellationToken::new(),
            },
        );

        // cancel_refetch = true: cancel the running fetch, then queue the
        // next-page request (the receiver is dropped, so the send fails
        // silently -- that's fine).
        infinite_observer.fetch_next_page_with(true, false);
        assert!(!client.is_in_flight(&key));
    }

    #[tokio::test]
    async fn test_fetch_next_page_with_reuses_in_flight_without_cancel() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let _ = (query.fetch_fn)(0).await.unwrap();
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
            fetch_direction: std::sync::Arc::default(),
        };

        let handle = tokio::spawn(async {});
        client.set_in_flight(
            &key,
            InFlightTask {
                abort_handle: handle.abort_handle(),
                cancel_token: CancellationToken::new(),
            },
        );

        // cancel_refetch = false: keep the in-flight fetch (dedup).
        infinite_observer.fetch_next_page_with(false, false);
        assert!(client.is_in_flight(&key));

        // And the previous-page variant behaves the same.
        infinite_observer.fetch_previous_page_with(false, false);
        assert!(client.is_in_flight(&key));
    }

    #[tokio::test]
    async fn test_fetch_next_page_with_sends_request_when_idle() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let _ = (query.fetch_fn)(0).await.unwrap();
        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let (next_tx, mut next_rx) = mpsc::channel(1);
        let (prev_tx, mut prev_rx) = mpsc::channel(1);
        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: next_tx,
            prev_page_tx: prev_tx,
            fetch_direction: std::sync::Arc::default(),
        };

        infinite_observer.fetch_next_page_with(true, false);
        assert!(next_rx.try_recv().is_ok(), "next-page request was queued");

        infinite_observer.fetch_previous_page_with(true, false);
        assert!(
            prev_rx.try_recv().is_ok(),
            "previous-page request was queued"
        );
    }

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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 0,
                fetch_fn,
                get_next_fn: None,
                get_prev_fn: None,
                max_pages: None,
            },
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

        let fetch_fn: PageFetchFn<Vec<i32>, i32> = Arc::new(|_param: i32| {
            Box::pin(async move { Err(QueryError::network("fetch failed")) })
        });

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(1);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 0,
                fetch_fn,
                get_next_fn: None,
                get_prev_fn: None,
                max_pages: None,
            },
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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 0,
                fetch_fn,
                get_next_fn: get_next,
                get_prev_fn: None,
                max_pages: None,
            },
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

        let fetch_fn: PageFetchFn<Vec<i32>, i32> = Arc::new(move |param: i32| {
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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 0,
                fetch_fn,
                get_next_fn: get_next,
                get_prev_fn: None,
                max_pages: None,
            },
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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 5,
                fetch_fn,
                get_next_fn: None,
                get_prev_fn: get_prev,
                max_pages: None,
            },
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

        let fetch_fn: PageFetchFn<Vec<i32>, i32> = Arc::new(move |param: i32| {
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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 5,
                fetch_fn,
                get_next_fn: None,
                get_prev_fn: get_prev,
                max_pages: None,
            },
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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 0,
                fetch_fn,
                get_next_fn: None,
                get_prev_fn: None,
                max_pages: None,
            },
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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 0,
                fetch_fn,
                get_next_fn: None,
                get_prev_fn: None,
                max_pages: None,
            },
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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 0,
                fetch_fn,
                get_next_fn: get_next,
                get_prev_fn: None,
                max_pages: None,
            },
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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 0,
                fetch_fn,
                get_next_fn: None,
                get_prev_fn: get_prev,
                max_pages: None,
            },
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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 99,
                fetch_fn,
                get_next_fn: None,
                get_prev_fn: None,
                max_pages: None,
            },
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
            Some(i32::try_from(all.len()).unwrap_or(i32::MAX))
        }));

        let observer = QueryObserver::new(&client, key.clone());
        let (next_tx, next_rx) = mpsc::channel::<()>(2);
        let (prev_tx, prev_rx) = mpsc::channel::<()>(1);

        let handle = tokio::spawn(run_infinite_query_loop(
            client.clone(),
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 0,
                fetch_fn,
                get_next_fn: get_next,
                get_prev_fn: None,
                max_pages: None,
            },
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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 0,
                fetch_fn,
                get_next_fn: get_next,
                get_prev_fn: None,
                max_pages: Some(3),
            },
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
            assert!(
                start.elapsed() <= Duration::from_secs(1),
                "timed out waiting for eviction"
            );
            if let Some(data) = client.get_infinite_data::<Vec<i32>, i32>(&key)
                && data.page_count() == 3
                && data.pages[0] == vec![1]
            {
                break;
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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 2,
                fetch_fn,
                get_next_fn: None,
                get_prev_fn: get_prev,
                max_pages: Some(3),
            },
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
            assert!(
                start.elapsed() <= Duration::from_secs(1),
                "timed out waiting for eviction"
            );
            if let Some(data) = client.get_infinite_data::<Vec<i32>, i32>(&key)
                && data.page_count() == 3
                && data.pages[0] == vec![-1]
            {
                break;
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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 0,
                fetch_fn,
                get_next_fn: get_next,
                get_prev_fn: None,
                max_pages: None,
            },
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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 0,
                fetch_fn,
                get_next_fn: get_next,
                get_prev_fn: None,
                max_pages: None,
            },
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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 1,
                fetch_fn,
                get_next_fn: get_next,
                get_prev_fn: get_prev,
                max_pages: None,
            },
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
            InfiniteLoopParams {
                key: key.clone(),
                options: QueryOptions::default(),
                initial_param: 0,
                fetch_fn,
                get_next_fn: None,
                get_prev_fn: None,
                max_pages: None,
            },
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

    // ========================================================================
    // spawn_infinite_query (GPUI integration)
    // ========================================================================

    struct SpawnHolder {
        observer: InfiniteQueryObserver<Vec<i32>, i32>,
    }

    impl gpui::Render for SpawnHolder {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::Empty
        }
    }

    #[gpui::test]
    fn test_spawn_infinite_query_fetches_and_appends_pages(cx: &mut gpui::TestAppContext) {
        let client = QueryClient::new();
        let key = QueryKey::new("spawn");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |param: i32| async move {
                // A small delay so the polling loop below actually waits at
                // least once before the initial page lands.
                std::thread::sleep(Duration::from_millis(50));
                Ok(vec![param])
            },
            0,
        )
        .get_next_page_param(|last: &Vec<i32>, _all: &[Vec<i32>]| Some(last[0] + 1));

        let view_client = client.clone();
        let view = cx
            .add_window_view(move |_window, cx| SpawnHolder {
                observer: spawn_infinite_query(cx, &view_client, &query, |_, _, _| {}),
            })
            .0;

        // The spawned thread performs the initial page fetch on its own
        // current-thread runtime.
        let deadline = Instant::now() + Duration::from_secs(5);
        while client.get_infinite_data::<Vec<i32>, i32>(&key).is_none() {
            assert!(Instant::now() < deadline, "initial page never arrived");
            std::thread::sleep(Duration::from_millis(2));
        }
        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);
        assert_eq!(data.pages[0], vec![0]);

        // Requesting the next page through the observer appends it.
        view.read_with(cx, |holder, _| holder.observer.fetch_next_page());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(Instant::now() < deadline, "next page never arrived");
            if let Some(data) = client.get_infinite_data::<Vec<i32>, i32>(&key)
                && data.page_count() >= 2
            {
                assert_eq!(data.pages[1], vec![1]);
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }

        // The loop thread updates its own observer clone; the handle's
        // observer was created before any data existed, so its cached state
        // intentionally stays behind. The client-level flags are meaningful:
        let fetching = view.read_with(cx, |holder, _| holder.observer.is_fetching());
        assert!(!fetching, "both page fetches finished, nothing in flight");

        // Dropping the observer (with the view) closes the request channels
        // and the loop thread exits on its own.
    }
}
