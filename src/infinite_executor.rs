// src/infinite_executor.rs
//! Infinite query execution

use crate::client::InFlightTask;
use crate::infinite::{InfiniteData, InfiniteQuery, PageFetchFn, PageParamFn};
use crate::observer::{QueryObserver, QueryStateVariant};
use crate::{QueryClient, QueryState};
use quoin_core::CancellationToken;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Direction marker for forward (next-page) fetch requests.
const DIRECTION_FORWARD: u8 = 0;
/// Direction marker for backward (previous-page) fetch requests.
const DIRECTION_BACKWARD: u8 = 1;

/// A page-fetch request sent from the observer to the loop task.
#[derive(Debug)]
pub(crate) enum PageRequest {
    /// Fetch the next page (forward).
    Next,
    /// Fetch the previous page (backward).
    Previous,
}

/// Capacity-1 request slot between the observer and the loop task.
///
/// Mirrors the bounded channel of the pre-migration implementation: a request
/// is queued when the slot is free and **dropped** when a request is already
/// pending (the loop is busy). The loop parks on the release watcher and
/// exits when the observer drops the sending side (which wakes the parked
/// receiver).
#[derive(Debug)]
pub(crate) struct PageRequestSender {
    inner: std::sync::Arc<SlotInner>,
}

/// Receiver half of the slot.
#[derive(Debug)]
pub(crate) struct PageRequestReceiver {
    inner: std::sync::Arc<SlotInner>,
}

#[derive(Debug)]
struct SlotInner {
    slot: std::sync::Mutex<Option<PageRequest>>,
    release: crate::watch::Watch<()>,
}

impl PageRequestSender {
    /// Builds a connected (sender, receiver) pair.
    pub(crate) fn pair() -> (PageRequestSender, PageRequestReceiver) {
        let (release, _rx) = crate::watch::Watch::new(());
        let inner = std::sync::Arc::new(SlotInner {
            slot: std::sync::Mutex::new(None),
            release,
        });
        (
            PageRequestSender {
                inner: Arc::clone(&inner),
            },
            PageRequestReceiver { inner },
        )
    }
}

impl PageRequestSender {
    /// Queue a request; **dropped** (returns `false`) when a request is
    /// already pending (slot capacity 1, matching the pre-migration
    /// bounded-`try_send` semantics).
    pub(crate) fn try_send(&self, request: PageRequest) -> bool {
        let mut guard = self
            .inner
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_some() {
            return false;
        }
        *guard = Some(request);
        self.inner.release.send(());
        true
    }
}

impl Drop for PageRequestSender {
    fn drop(&mut self) {
        // Wake a receiver parked in `recv` so it can observe the end of the
        // request stream (the strong-count check below).
        self.inner.release.send(());
    }
}

impl PageRequestReceiver {
    /// Waits for a request and takes it. Returns `None` once the sender side
    /// is gone and the slot is empty (the loop should exit).
    pub(crate) async fn recv(&mut self) -> Option<PageRequest> {
        loop {
            if let Some(request) = self
                .inner
                .slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                return Some(request);
            }
            // The sender lives in the observer: when it is dropped, stop.
            if Arc::strong_count(&self.inner) == 1 {
                return None;
            }
            // Park until a new request arrives (or the sender drops, which
            // wakes this watcher via its `Drop`).
            let _ = self.inner.release.subscribe().changed().await;
        }
    }
}

/// Observer for an infinite query with fetch-next/previous capabilities.
pub struct InfiniteQueryObserver<T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static>
{
    /// The underlying query observer over [`InfiniteData`]: the loop task
    /// keeps its state in sync with the cache.
    pub observer: QueryObserver<InfiniteData<T, P>>,
    query: Arc<InfiniteQuery<T, P>>,
    page_tx: PageRequestSender,
    /// Direction of the most recent page-fetch request; stands in for
    /// `TanStack`'s `fetchMeta.fetchMore.direction` (only one page fetch can be
    /// in flight at a time in this port).
    fetch_direction: Arc<AtomicU8>,
}

impl<T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static>
    InfiniteQueryObserver<T, P>
{
    /// Snapshot of the observer's current state.
    #[must_use]
    pub fn state(&self) -> &QueryState<InfiniteData<T, P>> {
        self.observer.state()
    }

    /// Whether another next page exists, derived from the newest known
    /// pages (the observer state, else the authoritative cache — the loop
    /// task owns the state updates, so the cache is the fresher view).
    #[must_use]
    pub fn has_next_page(&self) -> bool {
        let data = self.observer.state().data().cloned().or_else(|| {
            self.observer
                .client()
                .get_infinite_data::<T, P>(&self.query.key)
        });
        if let Some(data) = data
            && let Some(ref get_next) = self.query.get_next_page_param
            && let Some(last_page) = data.last_page()
        {
            return get_next(last_page, &data.pages).is_some();
        }
        false
    }

    /// Whether another previous page exists (see [`Self::has_next_page`]).
    #[must_use]
    pub fn has_previous_page(&self) -> bool {
        let data = self.observer.state().data().cloned().or_else(|| {
            self.observer
                .client()
                .get_infinite_data::<T, P>(&self.query.key)
        });
        if let Some(data) = data
            && let Some(ref get_prev) = self.query.get_previous_page_param
            && let Some(first_page) = data.first_page()
        {
            return get_prev(first_page, &data.pages).is_some();
        }
        false
    }

    /// Request fetching the next page.
    ///
    /// The request is queued (slot capacity 1, matching the previous
    /// release); when a request is already pending it is **dropped**, same as
    /// the bounded-`try_send` behaviour before the migration.
    pub fn fetch_next_page(&self) {
        self.fetch_direction
            .store(DIRECTION_FORWARD, Ordering::Relaxed);
        self.page_tx.try_send(PageRequest::Next);
    }

    /// Request fetching the previous page.
    pub fn fetch_previous_page(&self) {
        self.fetch_direction
            .store(DIRECTION_BACKWARD, Ordering::Relaxed);
        self.page_tx.try_send(PageRequest::Previous);
    }

    /// Whether a page fetch (either direction) is currently in flight.
    #[must_use]
    pub fn is_fetching(&self) -> bool {
        self.observer.client().is_in_flight(&self.query.key)
    }

    /// Whether the in-flight fetch is a forward page fetch (`TanStack`
    /// `isFetchingNextPage`); the direction is the one of the most recent
    /// page-fetch request.
    #[must_use]
    pub fn is_fetching_next_page(&self) -> bool {
        self.is_fetching() && self.fetch_direction.load(Ordering::Relaxed) == DIRECTION_FORWARD
    }

    /// Whether the in-flight fetch is a backward page fetch (`TanStack`
    /// `isFetchingPreviousPage`).
    #[must_use]
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

/// Core infinite query execution loop: fetches the initial page, then serves
/// page requests until every sender (i.e. the observer) is dropped.
pub(crate) async fn run_infinite_query_loop<T, P>(
    client: QueryClient,
    params: InfiniteLoopParams<T, P>,
    mut observer: QueryObserver<InfiniteData<T, P>>,
    mut page_rx: PageRequestReceiver,
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

    // Serve page requests until the observer (the only sender) is dropped.
    while let Some(request) = page_rx.recv().await {
        let (forward, param) = match request {
            PageRequest::Next => {
                let Some(data) = client.get_infinite_data::<T, P>(&key) else {
                    continue;
                };
                if client.is_in_flight(&key) {
                    continue;
                }
                let Some(Some(next_param)) = get_next_fn.as_ref().and_then(|get_next| {
                    data.last_page()
                        .map(|last_page| get_next(last_page, &data.pages))
                }) else {
                    continue;
                };
                (true, next_param)
            }
            PageRequest::Previous => {
                let Some(data) = client.get_infinite_data::<T, P>(&key) else {
                    continue;
                };
                if client.is_in_flight(&key) {
                    continue;
                }
                let Some(Some(prev_param)) = get_prev_fn.as_ref().and_then(|get_prev| {
                    data.first_page()
                        .map(|first_page| get_prev(first_page, &data.pages))
                }) else {
                    continue;
                };
                (false, prev_param)
            }
        };

        client.set_in_flight(
            &key,
            InFlightTask {
                // The page fetch runs inline in this loop task; hard abort is
                // not available, cooperative cancellation is (the token is
                // checked by `cancellable_fetch`-style fetch functions).
                abort: Box::new(|| {}),
                cancel_token: CancellationToken::new(),
            },
        );
        match fetch_fn(param.clone()).await {
            Ok(page) => {
                if forward {
                    client.append_infinite_page(&key, page, param, max_pages);
                } else {
                    client.prepend_infinite_page(&key, page, param, max_pages);
                }
                observer.update();
            }
            Err(_) => {
                client.notify_subscribers(key.cache_key(), QueryStateVariant::Error);
            }
        }
        client.clear_in_flight(&key);
    }
}

/// Execute an infinite query.
///
/// The page loop runs as one task on the client's runtime and keeps the
/// [`InfiniteQueryObserver`] in sync with the cache: `fetch_next_page` /
/// `fetch_previous_page` request one additional page at a time
/// (bidirectional, with `max_pages` trimming), refetch resets pages through
/// the ordinary cache path, and the loop parks on its page-request channels
/// until both observer clones are dropped.
///
/// There is no view callback anymore: observe the query through the returned
/// observer ([`InfiniteQueryObserver::state`], per-key subscriptions) or the
/// quoin binding hooks.
///
/// # Panics
///
/// Never panics under normal operation; a panicking page fetch is surfaced as
/// an error notification on the query key (the loop keeps running).
pub fn spawn_infinite_query<T, P>(
    client: &QueryClient,
    query: &InfiniteQuery<T, P>,
) -> InfiniteQueryObserver<T, P>
where
    T: Clone + Send + Sync + 'static + PartialEq,
    P: Clone + Send + Sync + 'static + PartialEq,
{
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

    // One capacity-1 slot carries page requests; the loop ends when the
    // observer (the only sender) is dropped.
    let (page_tx, page_rx) = PageRequestSender::pair();

    let runtime = client.runtime().clone();
    let task = run_infinite_query_loop(client, params, observer_thread, page_rx);
    let handle = runtime.spawn(Box::pin(task));
    // The loop lives as long as the observer: when the observer (and its
    // channel sender) is dropped the stream terminates and the task exits on
    // its own, so detach the handle instead of tying it to a scope.
    handle.detach();

    InfiniteQueryObserver {
        observer,
        query: Arc::new(query.clone()),
        page_tx,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
    async fn test_request_slot_send_recv() {
        let (tx, mut rx) = PageRequestSender::pair();
        assert!(tx.try_send(PageRequest::Next));
        assert!(matches!(rx.recv().await, Some(PageRequest::Next)));
    }

    #[tokio::test]
    async fn test_request_slot_full_drops_new_request() {
        let (tx, mut rx) = PageRequestSender::pair();
        assert!(tx.try_send(PageRequest::Next));
        // Capacity 1: a second request while one is pending is dropped.
        assert!(!tx.try_send(PageRequest::Previous));
        assert!(matches!(rx.recv().await, Some(PageRequest::Next)));
        // After the slot drains, new requests are queued again.
        assert!(tx.try_send(PageRequest::Previous));
    }

    #[tokio::test]
    async fn test_request_slot_ends_when_sender_dropped() {
        let (tx, mut rx) = PageRequestSender::pair();
        drop(tx);
        assert!(rx.recv().await.is_none());
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
            fetch_direction: std::sync::Arc::default(),
        };

        assert!(!infinite_observer.has_previous_page());
    }

    #[tokio::test]
    async fn test_fetch_next_page_slot_refills() {
        let (tx, mut rx) = PageRequestSender::pair();

        assert!(tx.try_send(PageRequest::Next));
        assert!(!tx.try_send(PageRequest::Next), "slot is full");

        let _ = rx.recv().await;
        assert!(tx.try_send(PageRequest::Next));
    }

    #[tokio::test]
    async fn test_fetch_previous_page_slot_refills() {
        let (tx, mut rx) = PageRequestSender::pair();

        assert!(tx.try_send(PageRequest::Previous));
        assert!(!tx.try_send(PageRequest::Previous), "slot is full");

        let _ = rx.recv().await;
        assert!(tx.try_send(PageRequest::Previous));
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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

        let (page_tx, _page_rx) = PageRequestSender::pair();
        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            page_tx,
            fetch_direction: std::sync::Arc::default(),
        };

        // The request is queued into the free slot; must not panic.
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

        let (page_tx, _page_rx) = PageRequestSender::pair();
        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            page_tx,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
            fetch_direction: std::sync::Arc::default(),
        };

        infinite_observer.fetch_next_page();
        infinite_observer.fetch_next_page();
        infinite_observer.fetch_next_page();
    }

    #[tokio::test]
    async fn test_fetch_next_page_drain_and_refill() {
        let (tx, mut rx) = PageRequestSender::pair();

        assert!(tx.try_send(PageRequest::Next));
        assert!(!tx.try_send(PageRequest::Next), "slot is full");

        let _ = rx.recv().await;
        assert!(tx.try_send(PageRequest::Next));

        let _ = rx.recv().await;
        assert!(tx.try_send(PageRequest::Next));
        assert!(!tx.try_send(PageRequest::Next), "slot is full again");
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
            page_tx: PageRequestSender::pair().0,
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
            page_tx: PageRequestSender::pair().0,
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
                abort: {
                    let handle = handle;
                    Box::new(move || handle.abort())
                },
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
            page_tx: PageRequestSender::pair().0,
            fetch_direction: std::sync::Arc::default(),
        };

        let handle = tokio::spawn(async {});
        client.set_in_flight(
            &key,
            InFlightTask {
                abort: {
                    let handle = handle;
                    Box::new(move || handle.abort())
                },
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
            page_tx: PageRequestSender::pair().0,
            fetch_direction: std::sync::Arc::default(),
        };

        let handle = tokio::spawn(async {});
        client.set_in_flight(
            &key,
            InFlightTask {
                abort: {
                    let handle = handle;
                    Box::new(move || handle.abort())
                },
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

        let (page_tx, mut page_rx) = PageRequestSender::pair();
        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            page_tx,
            fetch_direction: std::sync::Arc::default(),
        };

        infinite_observer.fetch_next_page_with(true, false);
        assert!(
            matches!(
                futures::executor::block_on(page_rx.recv()),
                Some(crate::infinite_executor::PageRequest::Next)
            ),
            "next-page request was queued"
        );

        infinite_observer.fetch_previous_page_with(true, false);
        assert!(
            matches!(
                futures::executor::block_on(page_rx.recv()),
                Some(crate::infinite_executor::PageRequest::Previous)
            ),
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
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);
        assert_eq!(data.pages[0], vec![0, 1, 2]);

        drop(page_tx);
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
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(client.get_infinite_data::<Vec<i32>, i32>(&key).is_none());

        drop(page_tx);
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
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        assert!(page_tx.try_send(PageRequest::Next), "request queued");

        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 2, Duration::from_secs(1)).await
        );

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 2);
        assert_eq!(data.pages[0], vec![0, 1]);
        assert_eq!(data.pages[1], vec![10, 11]);

        drop(page_tx);
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
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        assert!(page_tx.try_send(PageRequest::Next), "request queued");

        tokio::time::sleep(Duration::from_millis(100)).await;

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);

        drop(page_tx);
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
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        assert!(page_tx.try_send(PageRequest::Previous), "request queued");

        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 2, Duration::from_secs(1)).await
        );

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 2);
        assert_eq!(data.pages[0], vec![40, 41]);
        assert_eq!(data.pages[1], vec![50, 51]);

        drop(page_tx);
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
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        assert!(page_tx.try_send(PageRequest::Previous), "request queued");

        tokio::time::sleep(Duration::from_millis(100)).await;

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);

        drop(page_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_no_get_next_fn_skips_next() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let observer = QueryObserver::new(&client, key.clone());
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        assert!(page_tx.try_send(PageRequest::Next), "request queued");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);

        drop(page_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_no_get_prev_fn_skips_prev() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let observer = QueryObserver::new(&client, key.clone());
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        assert!(page_tx.try_send(PageRequest::Previous), "request queued");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);

        drop(page_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_get_next_returns_none_no_more_pages() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let get_next = Some(make_page_fn(|_last: &Vec<i32>, _all: &[Vec<i32>]| None));

        let observer = QueryObserver::new(&client, key.clone());
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        assert!(page_tx.try_send(PageRequest::Next), "request queued");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);

        drop(page_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_get_prev_returns_none_no_more_pages() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let get_prev = Some(make_page_fn(|_first: &Vec<i32>, _all: &[Vec<i32>]| None));

        let observer = QueryObserver::new(&client, key.clone());
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        assert!(page_tx.try_send(PageRequest::Previous), "request queued");
        tokio::time::sleep(Duration::from_millis(50)).await;

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);

        drop(page_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_loop_channel_close_breaks() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let observer = QueryObserver::new(&client, key.clone());
        let (page_tx, page_rx) = PageRequestSender::pair();

        // Drop the sender side immediately — the loop should exit via the
        // sender-gone check after serving the (empty) queue.
        drop(page_tx);

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
            page_rx,
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
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 1, Duration::from_secs(1)).await
        );

        assert!(page_tx.try_send(PageRequest::Next), "request queued");
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 2, Duration::from_secs(1)).await
        );

        assert!(page_tx.try_send(PageRequest::Next), "request queued");
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 3, Duration::from_secs(1)).await
        );

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 3);
        assert_eq!(data.pages[0], vec![0]);
        assert_eq!(data.pages[1], vec![1]);
        assert_eq!(data.pages[2], vec![2]);

        drop(page_tx);
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
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        // Initial: [[0]]
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 1, Duration::from_secs(1)).await
        );

        // Next: [[0], [1]]
        assert!(page_tx.try_send(PageRequest::Next), "request queued");
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 2, Duration::from_secs(1)).await
        );

        // Next: [[0], [1], [2]]
        assert!(page_tx.try_send(PageRequest::Next), "request queued");
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 3, Duration::from_secs(1)).await
        );

        // Next should trigger eviction: [[0], [1], [2]] -> [[1], [2], [3]]
        assert!(page_tx.try_send(PageRequest::Next), "request queued");
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

        drop(page_tx);
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
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        // Initial: [[2]]
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 1, Duration::from_secs(1)).await
        );

        // Prev: [[1], [2]]
        assert!(page_tx.try_send(PageRequest::Previous), "request queued");
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 2, Duration::from_secs(1)).await
        );

        // Prev: [[0], [1], [2]]
        assert!(page_tx.try_send(PageRequest::Previous), "request queued");
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 3, Duration::from_secs(1)).await
        );

        // Prev should trigger eviction: [[0], [1], [2]] -> [[-1], [0], [1]]
        assert!(page_tx.try_send(PageRequest::Previous), "request queued");
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

        drop(page_tx);
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
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        // Send next request while initial fetch is in progress (0-200ms window)
        assert!(page_tx.try_send(PageRequest::Next), "request queued");

        // No data yet since initial fetch hasn't completed
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(client.get_infinite_data::<Vec<i32>, i32>(&key).is_none());

        // Wait for initial fetch to complete
        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        // The loop processed the queued next request while data didn't exist,
        // so get_infinite_data returned None and it was silently dropped.
        // Should still only have 1 page.
        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);

        drop(page_tx);
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
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        assert!(page_tx.try_send(PageRequest::Next), "request queued");

        // While the first page fetch is still running the loop is busy; the
        // slot holds the request, so a second request is dropped.
        assert!(!page_tx.try_send(PageRequest::Next), "slot full while busy");

        tokio::time::sleep(Duration::from_millis(300)).await;

        // The loop consumed the first request; the slot is free again.
        assert!(page_tx.try_send(PageRequest::Next));

        drop(page_tx);
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
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 1, Duration::from_secs(1)).await
        );

        assert!(page_tx.try_send(PageRequest::Next), "request queued");
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 2, Duration::from_secs(1)).await
        );

        assert!(page_tx.try_send(PageRequest::Previous), "request queued");
        assert!(
            wait_for_page_count::<Vec<i32>, i32>(&client, &key, 3, Duration::from_secs(1)).await
        );

        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 3);
        assert_eq!(data.pages[0], vec![0]);
        assert_eq!(data.pages[1], vec![1]);
        assert_eq!(data.pages[2], vec![2]);

        drop(page_tx);
        let _ = handle.await;
    }
    #[tokio::test]
    async fn test_loop_empty_pages_after_initial_next_returns_none() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let fetch_fn = make_fetch_fn(|param: i32| Box::pin(async move { Ok(vec![param]) }));

        let observer = QueryObserver::new(&client, key.clone());
        let (page_tx, page_rx) = PageRequestSender::pair();

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
            page_rx,
        ));

        assert!(wait_for_data::<Vec<i32>, i32>(&client, &key, Duration::from_secs(1)).await);

        assert!(page_tx.try_send(PageRequest::Next), "request queued");

        tokio::time::sleep(Duration::from_millis(50)).await;
        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);

        drop(page_tx);
        let _ = handle.await;
    }

    // ========================================================================
    // spawn_infinite_query (runtime integration)
    // ========================================================================

    #[tokio::test]
    async fn test_spawn_infinite_query_fetches_and_appends_pages() {
        let client = crate::test_support::tokio_client();
        let key = QueryKey::new("spawn");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |param: i32| async move {
                // A small delay so the polling loop below actually waits at
                // least once before the initial page lands.
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(vec![param])
            },
            0,
        )
        .get_next_page_param(|last: &Vec<i32>, _all: &[Vec<i32>]| Some(last[0] + 1));

        let observer = spawn_infinite_query(&client, &query);

        // The loop task performs the initial page fetch on the client runtime.
        let deadline = Instant::now() + Duration::from_secs(5);
        while client.get_infinite_data::<Vec<i32>, i32>(&key).is_none() {
            assert!(Instant::now() < deadline, "initial page never arrived");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let data = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(data.page_count(), 1);
        assert_eq!(data.pages[0], vec![0]);

        // Requesting the next page through the observer appends it.
        observer.fetch_next_page();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(Instant::now() < deadline, "next page never arrived");
            if let Some(data) = client.get_infinite_data::<Vec<i32>, i32>(&key)
                && data.page_count() >= 2
            {
                assert_eq!(data.pages[1], vec![1]);
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        // The observer state converges right after the cache write (the
        // loop task calls `update()` post-append).
        let deadline = Instant::now() + Duration::from_secs(5);
        while !observer.has_next_page() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert!(observer.has_next_page());
        assert!(observer.is_fetching_next_page() || !observer.is_fetching());
    }
}
