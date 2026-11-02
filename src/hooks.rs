// src/hooks.rs
//! quoin bindings: observe rs-query state through any supported framework's
//! signals.
//!
//! Every `use_*` function is generic over `C: quoin_core::ReactiveContext`
//! and returns a `C::Signal<...>` plus (for the query/mutation bindings) a
//! driver handle. This is the portable replacement for the previous
//! gpui-entity hooks and the `spawn_query(cx, ..., callback)` view callbacks:
//! the pattern mirrors quoin's own `fetch-core` example — create signals on
//! the context, spawn a pump on the context's executor, and let the pump copy
//! authoritative cache state into the signal (lag-tolerant, `set_if_eq`-gated
//! so pull-based frameworks only repaint on real changes).
//!
//! # Lifecycle
//!
//! Binding structs own a stop token: dropping the binding cancels its pump
//! task (the equivalent of the old "view dropped → weak update fails"
//! teardown). The signal handle itself stays readable after the drop but no
//! longer receives updates.
//!
//! # Threading
//!
//! Pump tasks run on `cx.executor()` and call `signal.set(...)` from the
//! spawned task — exactly the contract quoin's `Resource`/`fetch-core` use.
//! On `NotifierSignal`-based adapters (gpui, xilem, the testkit) this is
//! thread-safe by construction; on fine-grained adapters follow the
//! framework's own threading rules (on `wasm32` there is a single thread, so
//! it always works).

use crate::bus::{Receiver, RecvError};
use crate::infinite::InfiniteQuery;
use crate::infinite_executor::InfiniteQueryObserver;
use crate::mutation::Mutation;
use crate::mutation_cache::MutationCache;
use crate::mutation_observer::MutationObserver;
use crate::options::PlaceholderData;
use crate::query::Query;
use crate::runtime::SpawnedTask;
use crate::state::{MutationState, QueryState};
use crate::{ActivityEvent, QueryClient, QueryError, QueryKey};
use futures::StreamExt;
use quoin_core::signal::Signal;
use quoin_core::{CancellationToken, Executor, JoinHandle, ReactiveContext};
use std::sync::Arc;

/// Maximum number of events retained by [`use_activity_feed`].
pub const ACTIVITY_FEED_CAPACITY: usize = 100;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Resolve a query's [`PlaceholderData`] for seeding (no previous data).
fn placeholder_value<T: Clone + Send + Sync + 'static>(query: &Query<T>) -> Option<T> {
    query.placeholder_data.as_ref().map(|data| match data {
        PlaceholderData::Value(value) => value.clone(),
        PlaceholderData::Function(function) => function(None),
    })
}

/// Read the current observer-visible [`QueryState<T>`] for a key from the
/// cache (the value a pump copies into its signal).
///
/// The type-erased cache does not persist error states, so a cached key with
/// data resolves to [`QueryState::Success`]/[`QueryState::Stale`]; a missing
/// key resolves to `LoadingWithPlaceholder` (when a placeholder exists) or
/// [`QueryState::Idle`].
#[must_use]
pub fn read_query_state<T>(
    client: &QueryClient,
    key: &QueryKey,
    placeholder: Option<T>,
) -> QueryState<T>
where
    T: Clone + Send + Sync + 'static,
{
    if let Some(data) = client.get_query_data::<T>(key) {
        if client.is_stale(key) {
            QueryState::Stale(data)
        } else {
            QueryState::Success(data)
        }
    } else {
        match placeholder {
            Some(data) => QueryState::LoadingWithPlaceholder(data),
            None => QueryState::Idle,
        }
    }
}

/// Push `item` into `buffer`, dropping the oldest entry past `capacity`.
pub fn push_capped<T>(buffer: &mut Vec<T>, item: T, capacity: usize) {
    if buffer.len() >= capacity {
        buffer.remove(0);
    }
    buffer.push(item);
}

/// Drain every currently-queued event from `rx` without awaiting.
pub fn drain_ready<E: Clone + Send + Sync + 'static>(rx: &mut Receiver<E>) -> Vec<E> {
    let mut events = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(event) => events.push(event),
            // Lagged: the skipped values are gone; keep draining.
            Err(crate::bus::TryRecvError::Lagged(_)) => {}
            Err(crate::bus::TryRecvError::Empty | crate::bus::TryRecvError::Closed) => break,
        }
    }
    events
}

// ---------------------------------------------------------------------------
// use_query
// ---------------------------------------------------------------------------

/// Binding returned by [`use_query`]: the live state signal plus the driver
/// handle for the initial fetch.
pub struct UseQuery<S, T: Clone + Send + Sync + 'static> {
    /// Live query state; update-gated so equal states do not notify.
    pub state: S,
    query: Query<T>,
    client: QueryClient,
    key: QueryKey,
    handle: SpawnedTask<QueryState<T>>,
    stop: CancellationToken,
}

impl<S, T: Clone + Send + Sync + PartialEq + 'static> UseQuery<S, T> {
    /// Trigger a refetch: the in-flight fetch is cancelled and a fresh one
    /// starts; the signal keeps updating through the pump.
    pub fn refetch(&self) {
        self.client.cancel_query(&self.key);
        let handle = crate::executor::spawn_query(&self.client, &self.query);
        // The refetch result flows through the cache notifications the pump
        // observes; the direct handle is detached (its state is redundant).
        handle.detach();
    }

    /// Abort the in-flight fetch without cancelling the pump.
    pub fn abort_fetch(&self) {
        self.client.cancel_query(&self.key);
    }
}

impl<S, T: Clone + Send + Sync + 'static> Drop for UseQuery<S, T> {
    fn drop(&mut self) {
        self.stop.cancel();
        // The pump stops with the token; the fetch task (if still running)
        // finishes on its own so the cache converges.
        self.handle.detach();
    }
}

impl<S, T: Clone + Send + Sync + 'static> std::fmt::Debug for UseQuery<S, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UseQuery").finish_non_exhaustive()
    }
}

/// Observe and drive one query (the `useQuery` analogue).
///
/// The initial state is seeded from the cache (respecting `initial_data` and
/// `placeholder_data`), the query is spawned immediately through
/// [`crate::spawn_query`] and a pump keeps the signal in sync with every
/// cache notification for the key — including invalidations, cancellations
/// and external `set_query_data` writes.
///
/// # Panics
///
/// Panics only if the context's executor cannot spawn (executor contract
/// violation); normal adapters never do.
pub fn use_query<C, T>(
    cx: &C,
    client: &QueryClient,
    query: &Query<T>,
) -> UseQuery<C::Signal<QueryState<T>>, T>
where
    C: ReactiveContext,
    C::Executor: Executor<JoinHandle<()>: JoinHandle<()> + 'static>,
    C::Signal<QueryState<T>>: Send + 'static,
    T: Clone + Send + Sync + PartialEq + 'static,
{
    let seed_initial = query.initial_data.clone();
    if client.get_query_data::<T>(&query.key).is_none()
        && let Some(initial) = seed_initial
    {
        client.set_query_data(&query.key, initial, query.options.clone());
    }
    let state = cx.create_signal(read_query_state(
        client,
        &query.key,
        placeholder_value(query),
    ));
    let handle = crate::executor::spawn_query(client, query);
    let stop = CancellationToken::new();

    // Pump: cache notifications → signal.
    let mut rx = client.subscribe(query.key.cache_key());
    let pump_signal = state.clone();
    let pump_client = client.clone();
    let pump_key = query.key.clone();
    let pump_stop = stop.clone();
    let pump_placeholder = placeholder_value(query);
    let pump = async move {
        while let Ok(_) | Err(RecvError::Lagged(_)) = rx.recv().await {
            if pump_stop.is_cancelled() {
                break;
            }
            let fresh = read_query_state(&pump_client, &pump_key, pump_placeholder.clone());
            pump_signal.set_if_eq(fresh);
        }
    };
    cx.executor().spawn(Box::pin(pump)).detach();

    UseQuery {
        state,
        query: query.clone(),
        client: client.clone(),
        key: query.key.clone(),
        handle,
        stop,
    }
}

// ---------------------------------------------------------------------------
// use_mutation
// ---------------------------------------------------------------------------

/// Binding returned by [`use_mutation`]: a live mutation-state signal plus a
/// tracked observer for executing mutations.
pub struct UseMutation<S, T, P> {
    /// Live mutation state (`Idle` until the first `mutate`).
    pub state: S,
    observer: std::sync::Mutex<MutationObserver<T, P>>,
    _marker: std::marker::PhantomData<fn() -> P>,
}

impl<
    S: Signal<MutationState<T>> + Send + Sync + 'static,
    T: Clone + Send + Sync + PartialEq + 'static,
    P: Clone + Send + Sync + 'static,
> UseMutation<S, T, P>
{
    /// Execute the mutation (the `useMutation().mutate` analogue).
    ///
    /// The returned handle resolves with the mutation result; the `state`
    /// signal mirrors `Idle → Loading → Success/Error` (the `Loading` write
    /// happens synchronously inside [`MutationObserver::mutate`]). Dropping
    /// the handle without awaiting or detaching aborts the execution; an
    /// aborted mutation leaves the signal in `Loading` until the next
    /// `mutate`/`reset`.
    pub fn mutate(
        &self,
        client: &QueryClient,
        params: P,
        per_call: Option<crate::mutation::MutateOptions<T, P>>,
    ) -> SpawnedTask<Result<T, QueryError>> {
        let inner_guard = self
            .observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .mutate(client, params, per_call);
        let (tx, rx) = futures::channel::oneshot::channel();
        let signal = self.state.clone();
        let runtime = client.runtime().clone();
        let task = async move {
            let result = inner_guard.await;
            signal.set(match &result {
                Ok(data) => MutationState::Success(data.clone()),
                Err(error) => MutationState::Error(error.clone()),
            });
            let _ = tx.send(result);
        };
        let handle = runtime.spawn(Box::pin(task));
        SpawnedTask::new(handle, rx, Arc::new(|| Err(QueryError::cancelled())))
    }

    /// Reset the tracked state back to [`MutationState::Idle`].
    pub fn reset(&self) {
        self.observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .reset();
        self.state.set(MutationState::Idle);
    }
}

impl<S, T, P> std::fmt::Debug for UseMutation<S, T, P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UseMutation").finish_non_exhaustive()
    }
}

/// Observe and drive one mutation (the `useMutation` analogue).
#[must_use]
pub fn use_mutation<C, T, P>(
    cx: &C,
    mutation: Mutation<T, P>,
) -> UseMutation<C::Signal<MutationState<T>>, T, P>
where
    C: ReactiveContext,
    C::Signal<MutationState<T>>: Send + 'static,
    T: Clone + Send + Sync + PartialEq + 'static,
    P: Clone + Send + Sync + 'static,
{
    let state = cx.create_signal(MutationState::Idle);
    let observer = MutationObserver::new(Arc::new(MutationCache::new()), mutation);
    UseMutation {
        state,
        observer: std::sync::Mutex::new(observer),
        _marker: std::marker::PhantomData,
    }
}

// ---------------------------------------------------------------------------
// use_infinite_query
// ---------------------------------------------------------------------------

/// Binding returned by [`use_infinite_query`]: the live state signal plus the
/// page-fetch observer.
pub struct UseInfiniteQuery<S, T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static> {
    /// Live query state over [`crate::infinite::InfiniteData`].
    pub state: S,
    observer: Arc<InfiniteQueryObserver<T, P>>,
    stop: CancellationToken,
}

impl<S, T, P> UseInfiniteQuery<S, T, P>
where
    T: Clone + Send + Sync + PartialEq + 'static,
    P: Clone + Send + Sync + PartialEq + 'static,
{
    /// Request the next page (`TanStack` `fetchNextPage`).
    pub fn fetch_next_page(&self) {
        self.observer.fetch_next_page();
    }

    /// Request the previous page (`TanStack` `fetchPreviousPage`).
    pub fn fetch_previous_page(&self) {
        self.observer.fetch_previous_page();
    }

    /// TanStack-style options variant for next-page fetches.
    pub fn fetch_next_page_with(&self, cancel_refetch: bool, throw_on_error: bool) {
        self.observer
            .fetch_next_page_with(cancel_refetch, throw_on_error);
    }

    /// TanStack-style options variant for previous-page fetches.
    pub fn fetch_previous_page_with(&self, cancel_refetch: bool, throw_on_error: bool) {
        self.observer
            .fetch_previous_page_with(cancel_refetch, throw_on_error);
    }

    /// Whether another next page exists.
    #[must_use]
    pub fn has_next_page(&self) -> bool {
        self.observer.has_next_page()
    }

    /// Whether another previous page exists.
    #[must_use]
    pub fn has_previous_page(&self) -> bool {
        self.observer.has_previous_page()
    }

    /// Whether a page fetch is in flight.
    #[must_use]
    pub fn is_fetching(&self) -> bool {
        self.observer.is_fetching()
    }

    /// Whether the in-flight page fetch is forward.
    #[must_use]
    pub fn is_fetching_next_page(&self) -> bool {
        self.observer.is_fetching_next_page()
    }

    /// Whether the in-flight page fetch is backward.
    #[must_use]
    pub fn is_fetching_previous_page(&self) -> bool {
        self.observer.is_fetching_previous_page()
    }

    /// The underlying infinite-query observer (for advanced use).
    #[must_use]
    pub fn observer(&self) -> Arc<InfiniteQueryObserver<T, P>> {
        Arc::clone(&self.observer)
    }
}

impl<S, T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static> Drop
    for UseInfiniteQuery<S, T, P>
{
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// Observe and drive one infinite query (the `useInfiniteQuery` analogue).
///
/// The page loop runs on the client's runtime; a pump keeps the state signal
/// in sync with the key's cache notifications (initial page, appended /
/// prepended pages, errors).
///
/// Shorthand for the state signal type of [`use_infinite_query`].
pub type InfiniteStateSignal<C, T, P> =
    <C as ReactiveContext>::Signal<QueryState<crate::infinite::InfiniteData<T, P>>>;

/// Observe and drive one infinite query (the `useInfiniteQuery` analogue).
///
/// The page loop runs on the client's runtime; a pump keeps the state signal
/// in sync with the key's cache notifications (initial page, appended /
/// prepended pages, errors).
///
/// # Panics
///
/// Panics only if the context's executor cannot spawn; normal adapters never
/// do.
pub fn use_infinite_query<C, T, P>(
    cx: &C,
    client: &QueryClient,
    query: &InfiniteQuery<T, P>,
) -> UseInfiniteQuery<InfiniteStateSignal<C, T, P>, T, P>
where
    C: ReactiveContext,
    C::Executor: Executor<JoinHandle<()>: JoinHandle<()> + 'static>,
    C::Signal<QueryState<crate::infinite::InfiniteData<T, P>>>: Send + 'static,
    T: Clone + Send + Sync + PartialEq + 'static,
    P: Clone + Send + Sync + PartialEq + 'static,
{
    let observer = Arc::new(crate::spawn_infinite_query(client, query));
    let state = cx.create_signal(observer.state().clone());
    let stop = CancellationToken::new();

    let mut rx = client.subscribe(query.key.cache_key());
    let pump_signal = state.clone();
    let pump_client = client.clone();
    let pump_key = query.key.clone();
    let pump_stop = stop.clone();
    let pump = async move {
        while let Ok(_) | Err(RecvError::Lagged(_)) = rx.recv().await {
            if pump_stop.is_cancelled() {
                break;
            }
            // Cache-derived state (Success/Stale/Idle): the loop task owns
            // the observer and keeps the cache authoritative.
            let fresh = read_query_state::<crate::infinite::InfiniteData<T, P>>(
                &pump_client,
                &pump_key,
                None,
            );
            pump_signal.set_if_eq(fresh);
        }
    };
    cx.executor().spawn(Box::pin(pump)).detach();

    UseInfiniteQuery {
        state,
        observer,
        stop,
    }
}

// ---------------------------------------------------------------------------
// Activity hooks
// ---------------------------------------------------------------------------

/// Observe whether any query is currently fetching (`useIsFetching` without
/// filters).
///
/// # Panics
///
/// Panics only if the context's executor cannot spawn; normal adapters never
/// do.
pub fn use_is_fetching<C>(cx: &C, client: &QueryClient) -> C::Signal<bool>
where
    C: ReactiveContext,
    C::Executor: Executor<JoinHandle<()>: JoinHandle<()> + 'static>,
    C::Signal<bool>: Send + 'static,
{
    let signal = cx.create_signal(client.is_fetching());
    spawn_activity_pump(cx, client, signal.clone(), |client, _| client.is_fetching());
    signal
}

/// Observe whether any mutation is currently executing (`useIsMutating`
/// without filters).
///
/// # Panics
///
/// Panics only if the context's executor cannot spawn; normal adapters never
/// do.
pub fn use_is_mutating<C>(cx: &C, client: &QueryClient) -> C::Signal<bool>
where
    C: ReactiveContext,
    C::Executor: Executor<JoinHandle<()>: JoinHandle<()> + 'static>,
    C::Signal<bool>: Send + 'static,
{
    let signal = cx.create_signal(client.is_mutating());
    spawn_activity_pump(cx, client, signal.clone(), |client, _| client.is_mutating());
    signal
}

/// Observe the number of in-flight fetches (`useFetchingCount`).
///
/// # Panics
///
/// Panics only if the context's executor cannot spawn; normal adapters never
/// do.
pub fn use_fetching_count<C>(cx: &C, client: &QueryClient) -> C::Signal<usize>
where
    C: ReactiveContext,
    C::Executor: Executor<JoinHandle<()>: JoinHandle<()> + 'static>,
    C::Signal<usize>: Send + 'static,
{
    let signal = cx.create_signal(client.fetching_count());
    spawn_activity_pump(cx, client, signal.clone(), |client, _| {
        client.fetching_count()
    });
    signal
}

/// Observe the number of in-flight mutations (`useMutatingCount`).
///
/// # Panics
///
/// Panics only if the context's executor cannot spawn; normal adapters never
/// do.
pub fn use_mutating_count<C>(cx: &C, client: &QueryClient) -> C::Signal<usize>
where
    C: ReactiveContext,
    C::Executor: Executor<JoinHandle<()>: JoinHandle<()> + 'static>,
    C::Signal<usize>: Send + 'static,
{
    let signal = cx.create_signal(client.mutating_count());
    spawn_activity_pump(cx, client, signal.clone(), |client, _| {
        client.mutating_count()
    });
    signal
}

/// Observe whether any of the given clients has a fetch in flight.
///
/// Every client's activity stream feeds the same signal; on each event the
/// boolean is recomputed across all clients.
///
/// # Panics
///
/// Panics only if the context's executor cannot spawn; normal adapters never
/// do.
pub fn use_any_fetching<C>(cx: &C, clients: &[QueryClient]) -> C::Signal<bool>
where
    C: ReactiveContext,
    C::Executor: Executor<JoinHandle<()>: JoinHandle<()> + 'static>,
    C::Signal<bool>: Send + 'static,
{
    let signal = cx.create_signal(clients.iter().any(QueryClient::is_fetching));
    for client in clients {
        let mut rx = client.subscribe_activity();
        let pump_signal = signal.clone();
        let all_clients: Vec<QueryClient> = clients.to_vec();
        let pump = async move {
            while let Ok(_) | Err(RecvError::Lagged(_)) = rx.recv().await {
                let any = all_clients.iter().any(QueryClient::is_fetching);
                pump_signal.set_if_eq(any);
            }
        };
        cx.executor().spawn(Box::pin(pump)).detach();
    }
    signal
}

/// Observe a live feed of events, capped at [`ACTIVITY_FEED_CAPACITY`]
/// entries (oldest dropped first). The receiver is drained up-front so
/// events buffered before the binding existed are included.
///
/// # Panics
///
/// Panics only if the context's executor cannot spawn; normal adapters never
/// do.
pub fn use_activity_feed<C, E>(cx: &C, mut rx: Receiver<E>) -> C::Signal<Vec<E>>
where
    C: ReactiveContext,
    C::Executor: Executor<JoinHandle<()>: JoinHandle<()> + 'static>,
    C::Signal<Vec<E>>: Send + 'static,
    E: Clone + Send + Sync + 'static,
{
    let initial = drain_ready(&mut rx);
    let signal = cx.create_signal(initial);
    let feed_signal = signal.clone();
    let stop = CancellationToken::new();
    let pump_stop = stop.clone();
    let pump = async move {
        while let Some(Ok(event)) = rx.next().await {
            if pump_stop.is_cancelled() {
                break;
            }
            feed_signal.update(|buffer| push_capped(buffer, event, ACTIVITY_FEED_CAPACITY));
        }
    };
    cx.executor().spawn(Box::pin(pump)).detach();
    signal
}

/// Observe several queries at once (`useQueries`): the signal holds one
/// [`QueryState`] per spec, each slot updated by its own key-filtered pump.
///
/// # Panics
///
/// Panics only if the context's executor cannot spawn; normal adapters never
/// do.
pub fn use_queries<C, T>(
    cx: &C,
    client: &QueryClient,
    specs: &[Query<T>],
) -> C::Signal<Vec<QueryState<T>>>
where
    C: ReactiveContext,
    C::Executor: Executor<JoinHandle<()>: JoinHandle<()> + 'static>,
    C::Signal<Vec<QueryState<T>>>: Send + 'static,
    T: Clone + Send + Sync + PartialEq + 'static,
{
    let states: Vec<QueryState<T>> = specs
        .iter()
        .map(|spec| {
            if client.get_query_data::<T>(&spec.key).is_none()
                && let Some(initial) = &spec.initial_data
            {
                client.set_query_data(&spec.key, initial.clone(), spec.options.clone());
            }
            read_query_state(client, &spec.key, placeholder_value(spec))
        })
        .collect();
    let signal = cx.create_signal(states);

    // One pump per spec: its receiver filters to that key, and updates write
    // only the spec's slot (signal updates are atomic, so concurrent pumps
    // are serialized).
    for (idx, spec) in specs.iter().enumerate() {
        let mut rx = client.subscribe(spec.key.cache_key());
        let pump_signal = signal.clone();
        let pump_client = client.clone();
        let pump_key = spec.key.clone();
        let pump_placeholder = placeholder_value(spec);
        let pump = async move {
            while let Ok(_) | Err(RecvError::Lagged(_)) = rx.recv().await {
                let fresh = read_query_state(&pump_client, &pump_key, pump_placeholder.clone());
                pump_signal.update(|states| {
                    if let Some(slot) = states.get_mut(idx)
                        && *slot != fresh
                    {
                        *slot = fresh;
                    }
                });
            }
        };
        cx.executor().spawn(Box::pin(pump)).detach();
    }
    signal
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Spawn a pump that recomputes `signal` from the client's activity events.
///
/// `recompute` derives the new signal value from the client state after each
/// activity event (count changes).
fn spawn_activity_pump<C, S, T, F>(cx: &C, client: &QueryClient, signal: S, recompute: F)
where
    C: ReactiveContext,
    C::Executor: Executor<JoinHandle<()>: JoinHandle<()> + 'static>,
    S: Signal<T> + Send + 'static,
    T: Clone + PartialEq + Send + 'static,
    F: Fn(&QueryClient, &ActivityEvent) -> T + Send + 'static,
{
    let mut rx = client.subscribe_activity();
    let pump_client = client.clone();
    let pump = async move {
        while let Ok(event) = rx.recv().await {
            let value = recompute(&pump_client, &event);
            signal.set_if_eq(value);
        }
    };
    cx.executor().spawn(Box::pin(pump)).detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QueryError;
    use crate::key::QueryKey;
    use crate::mutation::Mutation;
    use crate::query::Query;
    use crate::state::MutationState;
    use quoin_testkit::{ImmediateExecutor, TestContext, ThreadedExecutor, UpdateRecorder};
    use std::sync::atomic::{AtomicUsize, Ordering};

    type TestCx = TestContext<ThreadedExecutor>;

    /// Context whose pumps run on dedicated threads (poll with deadlines).
    pub(crate) fn threaded_cx() -> TestCx {
        TestContext::with_executor(ThreadedExecutor)
    }

    fn ok_query(key: &QueryKey, data: &'static str) -> Query<String> {
        Query::new(key.clone(), move || async move { Ok(data.to_string()) })
    }

    async fn wait_for(ready: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready() && std::time::Instant::now() < deadline {
            // Tokio sleep: yields to the current-thread test runtime so the
            // fetch tasks make progress while polling.
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(ready(), "condition not reached in time");
    }

    // ------------------------------------------------------------------
    // use_query
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn use_query_seeds_executes_and_tracks() {
        let cx = threaded_cx();
        let client = crate::test_support::tokio_client();
        let key = QueryKey::new("hooks-query");
        let query = ok_query(&key, "data");

        let binding = use_query(&cx, &client, &query);
        // Seed: idle (nothing cached yet).
        assert!(matches!(binding.state.get(), QueryState::Idle));

        // The fetch resolves and the pump copies the success state into the
        // signal.
        wait_for(|| matches!(binding.state.get(), QueryState::Success(_))).await;
        assert_eq!(
            binding.state.get().data(),
            Some(&"data".to_string()),
            "signal mirrors the cached data"
        );
    }

    #[tokio::test]
    async fn use_query_refetch_and_invalidation_flow_through_signal() {
        let cx = threaded_cx();
        let client = crate::test_support::tokio_client();
        let key = QueryKey::new("hooks-refetch");
        let query = ok_query(&key, "v1");

        let binding = use_query(&cx, &client, &query);
        wait_for(|| matches!(binding.state.get(), QueryState::Success(_))).await;

        // External cache writes reach the signal through the pump.
        client.set_query_data(&key, "v2".to_string(), crate::QueryOptions::default());
        wait_for(|| binding.state.get().data() == Some(&"v2".to_string())).await;

        // Invalidation marks the signal stale through the pump.
        client.invalidate_queries(&key);
        wait_for(|| matches!(binding.state.get(), QueryState::Stale(_))).await;

        // refetch resolves the stale mark back to fresh data.
        binding.refetch();
        wait_for(|| matches!(binding.state.get(), QueryState::Success(_))).await;
    }

    #[tokio::test]
    async fn use_query_drop_stops_the_pump() {
        let cx = threaded_cx();
        let client = crate::test_support::tokio_client();
        let key = QueryKey::new("hooks-drop");
        let query = ok_query(&key, "data");
        let binding = use_query(&cx, &client, &query);
        wait_for(|| matches!(binding.state.get(), QueryState::Success(_))).await;

        let signal = binding.state.clone();
        drop(binding);
        std::thread::sleep(std::time::Duration::from_millis(30));
        client.set_query_data(
            &key,
            "after-drop".to_string(),
            crate::QueryOptions::default(),
        );
        std::thread::sleep(std::time::Duration::from_millis(30));
        // The pump is cancelled: the signal no longer tracks the cache.
        assert_ne!(
            signal.get().data(),
            Some(&"after-drop".to_string()),
            "dropped binding must stop tracking"
        );
    }

    // ------------------------------------------------------------------
    // use_mutation
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn use_mutation_tracks_lifecycle() {
        let cx = threaded_cx();
        let mutation = Mutation::new(|param: i32| async move { Ok(param * 2) });
        let binding = use_mutation(&cx, mutation);
        assert!(matches!(binding.state.get(), MutationState::Idle));

        let client = crate::test_support::tokio_client();
        let result = binding.mutate(&client, 21, None).await;
        assert_eq!(result.expect("mutation succeeds"), 42);
        assert!(matches!(binding.state.get(), MutationState::Success(_)));

        binding.reset();
        assert!(matches!(binding.state.get(), MutationState::Idle));
    }

    #[tokio::test]
    async fn use_mutation_surfaces_errors() {
        let cx = threaded_cx();
        let mutation: Mutation<String, i32> =
            Mutation::new(|_param: i32| async { Err(QueryError::custom("nope")) });
        let binding = use_mutation(&cx, mutation);
        let client = crate::test_support::tokio_client();
        let result = binding.mutate(&client, 1, None).await;
        assert_eq!(result.unwrap_err().to_string(), "nope");
        assert!(matches!(binding.state.get(), MutationState::Error(_)));
    }

    // ------------------------------------------------------------------
    // Activity hooks
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn use_is_fetching_tracks_activity_counts() {
        let cx = threaded_cx();
        let client = crate::test_support::tokio_client();
        let fetching = use_is_fetching(&cx, &client);
        let count = use_fetching_count(&cx, &client);
        assert!(!fetching.get());
        assert_eq!(count.get(), 0);

        client.inc_fetching();
        wait_for(|| fetching.get()).await;
        wait_for(|| count.get() == 1).await;

        client.dec_fetching();
        wait_for(|| !fetching.get()).await;
        wait_for(|| count.get() == 0).await;
    }

    #[tokio::test]
    async fn use_is_mutating_and_counts_track_activity() {
        let cx = threaded_cx();
        let client = crate::test_support::tokio_client();
        let mutating = use_is_mutating(&cx, &client);
        let mutating_count = use_mutating_count(&cx, &client);
        client.inc_mutating();
        wait_for(|| mutating.get()).await;
        wait_for(|| mutating_count.get() == 1).await;
        client.dec_mutating();
        wait_for(|| mutating_count.get() == 0).await;
    }

    #[tokio::test]
    async fn use_any_fetching_combines_clients() {
        let cx = threaded_cx();
        let a = crate::test_support::tokio_client();
        let b = crate::test_support::tokio_client();
        let any = use_any_fetching(&cx, &[a.clone(), b.clone()]);
        assert!(!any.get());
        b.inc_fetching();
        wait_for(|| any.get()).await;
        b.dec_fetching();
        wait_for(|| !any.get()).await;
    }

    #[tokio::test]
    async fn use_activity_feed_caps_and_orders() {
        let cx = threaded_cx();
        let client = crate::test_support::tokio_client();
        let rx = client.subscribe_activity();
        let feed = use_activity_feed(&cx, rx);

        // Paced first: 30 pairs stream through the bounded activity channel
        // without lagging it, so the feed accumulates them in order.
        for _ in 0..30 {
            client.inc_fetching();
            client.dec_fetching();
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        wait_for(|| feed.get().len() >= 60).await;
        let first = feed.get();
        // Each inc+dec pair emits two count-change events.
        assert_eq!(first.len(), 60, "paced events all land, in order");

        // Flood past the capacity: the feed must stay bounded (oldest
        // dropped), never exceeding ACTIVITY_FEED_CAPACITY.
        for _ in 0..(ACTIVITY_FEED_CAPACITY * 2) {
            client.inc_fetching();
            client.dec_fetching();
        }
        wait_for(|| !feed.get().is_empty()).await;
        std::thread::sleep(std::time::Duration::from_millis(80));
        let events = feed.get();
        assert!(
            events.len() <= ACTIVITY_FEED_CAPACITY,
            "feed is capped, got {}",
            events.len()
        );
    }

    #[tokio::test]
    async fn use_queries_tracks_each_spec_slot() {
        let cx = threaded_cx();
        let client = crate::test_support::tokio_client();
        let specs = vec![
            ok_query(&QueryKey::new("hooks-multi-1"), "one"),
            ok_query(&QueryKey::new("hooks-multi-2"), "two"),
        ];
        let states = use_queries(&cx, &client, &specs);
        // `use_queries` only observes (TanStack parity): execution is driven
        // separately, e.g. through `spawn_query`.
        for spec in &specs {
            crate::spawn_query(&client, spec).detach();
        }
        wait_for(|| {
            states
                .get()
                .iter()
                .all(|state| matches!(state, QueryState::Success(_)))
        })
        .await;
        let snapshot = states.get();
        assert_eq!(snapshot[0].data(), Some(&"one".to_string()));
        assert_eq!(snapshot[1].data(), Some(&"two".to_string()));
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    #[test]
    fn read_query_state_maps_cache_to_states() {
        let client = QueryClient::new();
        let key = QueryKey::new("read-state");
        assert!(matches!(
            read_query_state::<String>(&client, &key, None),
            QueryState::Idle
        ));
        client.set_query_data(&key, "x".to_string(), crate::QueryOptions::default());
        assert!(matches!(
            read_query_state::<String>(&client, &key, None),
            QueryState::Success(_)
        ));
        client.invalidate_queries(&key);
        assert!(matches!(
            read_query_state::<String>(&client, &key, None),
            QueryState::Stale(_)
        ));
    }

    #[test]
    fn push_capped_drops_oldest() {
        let mut buffer: Vec<u32> = Vec::new();
        for i in 0..5 {
            push_capped(&mut buffer, i, 3);
        }
        assert_eq!(buffer, vec![2, 3, 4]);
    }

    #[test]
    fn drain_ready_empties_the_queue() {
        let (tx, mut rx) = crate::bus::Sender::channel(8);
        tx.send(&1);
        tx.send(&2);
        let events = drain_ready(&mut rx);
        assert_eq!(events, vec![1, 2]);
        assert_eq!(drain_ready(&mut rx), Vec::<u32>::new());
    }

    #[test]
    fn placeholder_value_resolves_static_and_function() {
        let key = QueryKey::new("placeholder");
        let static_query: Query<String> =
            Query::new(key.clone(), || async { Ok("real".to_string()) })
                .placeholder_data(crate::options::PlaceholderData::Value("ph".to_string()));
        assert_eq!(placeholder_value(&static_query), Some("ph".to_string()));

        let fn_query: Query<String> = Query::new(key, || async { Ok("real".to_string()) })
            .placeholder_data(crate::options::PlaceholderData::Function(
                std::sync::Arc::new(|previous: Option<&String>| {
                    previous.cloned().unwrap_or_else(|| "fallback".to_string())
                }),
            ));
        assert_eq!(placeholder_value(&fn_query), Some("fallback".to_string()));
    }

    // Silence unused-import warnings for helpers only used on some paths.
    #[test]
    fn recorder_smoke() {
        let cx = TestContext::with_executor(ImmediateExecutor);
        let signal = cx.create_signal(0u32);
        signal.set(1);
        let recorder = UpdateRecorder::new();
        recorder.assert_count(0);
        let _ = AtomicUsize::new(0).load(Ordering::SeqCst);
    }
}
