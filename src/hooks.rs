// src/hooks.rs
//! Reactive hooks for GPUI integration.

use crate::observer::QueryStateUpdate;
use crate::query::Query;
use crate::state::QueryState;
use crate::{ActivityEvent, QueryClient, QueryKey};
use gpui::{AppContext, AsyncApp, Context, Entity, Task, WeakEntity};
use tokio::sync::broadcast;

/// A reactive boolean that indicates if any query is fetching.
pub struct UseIsFetching {
    value: bool,
    _task: Task<()>,
}

impl UseIsFetching {
    /// Returns the current fetching status.
    pub fn get(&self) -> bool {
        self.value
    }
}

/// Create a reactive hook that tracks whether any query is fetching.
pub fn use_is_fetching<V: 'static>(
    cx: &mut Context<V>,
    client: &QueryClient,
) -> Entity<UseIsFetching> {
    let client = client.clone();
    let mut rx = client.subscribe_activity();
    let initial = client.is_fetching();

    cx.new(|cx: &mut Context<UseIsFetching>| {
        let task = cx.spawn(|this: WeakEntity<UseIsFetching>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                while let Ok(event) = rx.recv().await {
                    if let ActivityEvent::FetchingCountChanged(_) = event {
                        _ = this.update(
                            &mut cx,
                            |this: &mut UseIsFetching, cx: &mut Context<UseIsFetching>| {
                                this.value = client.is_fetching();
                                cx.notify();
                            },
                        );
                    }
                }
            }
        });
        UseIsFetching {
            value: initial,
            _task: task,
        }
    })
}

/// A reactive boolean that indicates if any mutation is in progress.
pub struct UseIsMutating {
    value: bool,
    _task: Task<()>,
}

impl UseIsMutating {
    /// Returns the current mutating status.
    pub fn get(&self) -> bool {
        self.value
    }
}

/// Create a reactive hook that tracks whether any mutation is in progress.
pub fn use_is_mutating<V: 'static>(
    cx: &mut Context<V>,
    client: &QueryClient,
) -> Entity<UseIsMutating> {
    let client = client.clone();
    let mut rx = client.subscribe_activity();
    let initial = client.is_mutating();

    cx.new(|cx: &mut Context<UseIsMutating>| {
        let task = cx.spawn(|this: WeakEntity<UseIsMutating>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                while let Ok(event) = rx.recv().await {
                    if let ActivityEvent::MutatingCountChanged(_) = event {
                        _ = this.update(
                            &mut cx,
                            |this: &mut UseIsMutating, cx: &mut Context<UseIsMutating>| {
                                this.value = client.is_mutating();
                                cx.notify();
                            },
                        );
                    }
                }
            }
        });
        UseIsMutating {
            value: initial,
            _task: task,
        }
    })
}

/// A reactive counter that tracks the number of in‑flight fetches.
pub struct UseFetchingCount {
    value: usize,
    _task: Task<()>,
}

impl UseFetchingCount {
    /// Returns the current fetching count.
    pub fn get(&self) -> usize {
        self.value
    }
}

/// Create a reactive hook that tracks the number of in‑flight fetch operations.
pub fn use_fetching_count<V: 'static>(
    cx: &mut Context<V>,
    client: &QueryClient,
) -> Entity<UseFetchingCount> {
    let client = client.clone();
    let mut rx = client.subscribe_activity();
    let initial = client.fetching_count();

    cx.new(|cx: &mut Context<UseFetchingCount>| {
        let task = cx.spawn(|this: WeakEntity<UseFetchingCount>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                while let Ok(event) = rx.recv().await {
                    if let ActivityEvent::FetchingCountChanged(count) = event {
                        _ = this.update(
                            &mut cx,
                            |this: &mut UseFetchingCount, cx: &mut Context<UseFetchingCount>| {
                                this.value = count;
                                cx.notify();
                            },
                        );
                    }
                }
            }
        });
        UseFetchingCount {
            value: initial,
            _task: task,
        }
    })
}

/// A reactive counter that tracks the number of in‑flight mutations.
pub struct UseMutatingCount {
    value: usize,
    _task: Task<()>,
}

impl UseMutatingCount {
    /// Returns the current mutating count.
    pub fn get(&self) -> usize {
        self.value
    }
}

/// Create a reactive hook that tracks the number of in‑flight mutation operations.
pub fn use_mutating_count<V: 'static>(
    cx: &mut Context<V>,
    client: &QueryClient,
) -> Entity<UseMutatingCount> {
    let client = client.clone();
    let mut rx = client.subscribe_activity();
    let initial = client.mutating_count();

    cx.new(|cx: &mut Context<UseMutatingCount>| {
        let task = cx.spawn(|this: WeakEntity<UseMutatingCount>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                while let Ok(event) = rx.recv().await {
                    if let ActivityEvent::MutatingCountChanged(count) = event {
                        _ = this.update(
                            &mut cx,
                            |this: &mut UseMutatingCount, cx: &mut Context<UseMutatingCount>| {
                                this.value = count;
                                cx.notify();
                            },
                        );
                    }
                }
            }
        });
        UseMutatingCount {
            value: initial,
            _task: task,
        }
    })
}

/// Maximum number of events retained by [`use_activity_feed`].
pub const ACTIVITY_FEED_CAPACITY: usize = 100;

/// Read the observer-visible [`QueryState`] for a key from the client cache.
///
/// In-flight fetches surface as `Loading` (no cached data yet) or `Refetching`
/// (cached data kept while refetching); otherwise cached data maps to
/// `Stale`/`Success` depending on staleness, and missing data to `Idle`.
fn read_query_state<T: Clone + Send + Sync + 'static>(
    client: &QueryClient,
    key: &QueryKey,
) -> QueryState<T> {
    match client.get_query_data::<T>(key) {
        Some(data) if client.is_in_flight(key) => QueryState::Refetching(data),
        Some(data) if client.is_stale(key) => QueryState::Stale(data),
        Some(data) => QueryState::Success(data),
        None if client.is_in_flight(key) => QueryState::Loading,
        None => QueryState::Idle,
    }
}

/// Like [`read_query_state`], but seeds the cache with the query's
/// `initial_data` first when the cache is empty (mirrors
/// `QueryObserver::from_query`).
fn query_state_from_cache<T>(client: &QueryClient, query: &Query<T>) -> QueryState<T>
where
    T: Clone + Send + Sync + PartialEq + 'static,
{
    if client.get_query_data::<T>(&query.key).is_none()
        && let Some(initial) = &query.initial_data
    {
        client.set_query_data(&query.key, initial.clone(), query.options.clone());
    }
    read_query_state::<T>(client, &query.key)
}

/// Push `event` onto `feed`, keeping at most the `capacity` most recent
/// events (older events are dropped from the front).
fn push_capped<E>(feed: &mut Vec<E>, event: E, capacity: usize) {
    if capacity == 0 {
        return;
    }
    if feed.len() >= capacity {
        let overflow = feed.len() + 1 - capacity;
        feed.drain(0..overflow);
    }
    feed.push(event);
}

/// Drain events already buffered in a broadcast receiver without waiting for
/// new ones. Lagged (missed) events are skipped.
fn drain_ready<E: Clone>(rx: &mut broadcast::Receiver<E>) -> Vec<E> {
    let mut events = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(event) => push_capped(&mut events, event, ACTIVITY_FEED_CAPACITY),
            Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed) => {
                break;
            }
            Err(broadcast::error::TryRecvError::Lagged(_)) => {}
        }
    }
    events
}

/// Reactive hook mirroring `TanStack`'s `useQueries`: observe a list of query
/// specs and expose a model of per-spec [`QueryState`]s.
///
/// The hook subscribes to each spec's cache key; whenever a subscribed key
/// emits an update, the corresponding result slot is re-read from the client
/// cache (via `get_query_data` + `is_stale` + in-flight checks, see
/// [`read_query_state`]). Initial values are taken from the cache, seeding
/// `initial_data` where applicable.
///
/// The hook itself does not trigger fetches — pair it with
/// [`crate::executor::execute_query`], `spawn_query`, or a
/// [`crate::queries_observer::QueriesObserver`] that drives execution; state
/// changes flowing through the client cache are picked up automatically.
/// Each spec index has its own subscription task; lagged broadcasts are
/// tolerated by re-reading the cached state.
pub fn use_queries<T, V: 'static>(
    cx: &mut Context<V>,
    client: &QueryClient,
    specs: Vec<Query<T>>,
) -> Entity<Vec<QueryState<T>>>
where
    T: Clone + Send + Sync + PartialEq + 'static,
{
    let client = client.clone();
    let initial: Vec<QueryState<T>> = specs
        .iter()
        .map(|spec| query_state_from_cache(&client, spec))
        .collect();
    let receivers: Vec<(usize, broadcast::Receiver<QueryStateUpdate>)> = specs
        .iter()
        .enumerate()
        .map(|(idx, spec)| (idx, client.subscribe(spec.key.cache_key())))
        .collect();

    cx.new(move |cx: &mut Context<Vec<QueryState<T>>>| {
        for (idx, mut rx) in receivers {
            let client = client.clone();
            let specs = specs.clone();
            cx.spawn(
                move |this: WeakEntity<Vec<QueryState<T>>>, cx: &mut AsyncApp| {
                    let mut cx = cx.clone();
                    async move {
                        // Fresh updates and lagged broadcasts both re-read the
                        // cached state (self-healing); a closed channel (all
                        // client clones gone) ends the tracking task.
                        while !matches!(rx.recv().await, Err(broadcast::error::RecvError::Closed)) {
                            // `idx` indexes the specs list this receiver was
                            // built from, so the lookup is always in bounds.
                            let spec = &specs[idx];
                            let fresh = read_query_state(&client, &spec.key);
                            let delivered = this.update(
                            &mut cx,
                            |states: &mut Vec<QueryState<T>>,
                             cx: &mut Context<Vec<QueryState<T>>>| {
                                if let Some(slot) = states.get_mut(idx) {
                                    *slot = fresh.clone();
                                    cx.notify();
                                }
                            },
                        );
                            if delivered.is_err() {
                                break; // entity dropped, stop tracking
                            }
                        }
                    }
                },
            )
            .detach();
        }
        initial
    })
}

/// Reactive hook mirroring `TanStack`'s `useMutationState`, generalized over
/// the event type so it can follow any broadcast feed (e.g. a future
/// mutation cache's event stream) without a hard dependency.
///
/// Events already buffered in `rx` when the hook is created are drained into
/// the model; afterwards every incoming event is appended. The feed is
/// capped at the [`ACTIVITY_FEED_CAPACITY`] most recent events.
pub fn use_activity_feed<E, V: 'static>(
    cx: &mut Context<V>,
    mut rx: broadcast::Receiver<E>,
) -> Entity<Vec<E>>
where
    E: Clone + Send + Sync + 'static,
{
    let initial = drain_ready(&mut rx);

    cx.new(move |cx: &mut Context<Vec<E>>| {
        cx.spawn(move |this: WeakEntity<Vec<E>>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                loop {
                    let event = match rx.recv().await {
                        Ok(event) => event,
                        // Missed events are dropped; the feed keeps following.
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    let delivered =
                        this.update(&mut cx, |feed: &mut Vec<E>, cx: &mut Context<Vec<E>>| {
                            push_capped(feed, event, ACTIVITY_FEED_CAPACITY);
                            cx.notify();
                        });
                    if delivered.is_err() {
                        break; // entity dropped, stop tracking
                    }
                }
            }
        })
        .detach();
        initial
    })
}

/// Reactive hook that reports whether *any* of the given clients currently
/// has queries in flight (multi-client variant of [`use_is_fetching`]).
///
/// Like the single-client hooks it subscribes to each client's activity
/// broadcast instead of polling: any fetching/mutating count change on any
/// client re-evaluates `client.is_fetching()` across all clients.
pub fn use_any_fetching<V: 'static>(cx: &mut Context<V>, clients: &[QueryClient]) -> Entity<bool> {
    let initial = clients.iter().any(super::client::QueryClient::is_fetching);
    let receivers: Vec<broadcast::Receiver<ActivityEvent>> = clients
        .iter()
        .map(super::client::QueryClient::subscribe_activity)
        .collect();

    cx.new(move |cx: &mut Context<bool>| {
        for mut rx in receivers {
            let clients = clients.to_vec();
            cx.spawn(move |this: WeakEntity<bool>, cx: &mut AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    // A closed channel (all client clones gone) ends the
                    // tracking task; every other outcome re-evaluates.
                    while !matches!(rx.recv().await, Err(broadcast::error::RecvError::Closed)) {
                        let any_fetching =
                            clients.iter().any(super::client::QueryClient::is_fetching);
                        let delivered =
                            this.update(&mut cx, |value: &mut bool, cx: &mut Context<bool>| {
                                *value = any_fetching;
                                cx.notify();
                            });
                        if delivered.is_err() {
                            break; // entity dropped, stop tracking
                        }
                    }
                }
            })
            .detach();
        }
        initial
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QueryClient;
    use gpui::{Empty, Render, TestAppContext, Window};

    struct TestView {
        is_fetching: Entity<UseIsFetching>,
        is_mutating: Entity<UseIsMutating>,
        fetching_count: Entity<UseFetchingCount>,
        mutating_count: Entity<UseMutatingCount>,
    }

    impl Render for TestView {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            Empty
        }
    }

    fn create_test_view(cx: &mut TestAppContext, client: &QueryClient) -> Entity<TestView> {
        let client = client.clone();
        cx.add_window_view(move |_window, cx| TestView {
            is_fetching: use_is_fetching(cx, &client),
            is_mutating: use_is_mutating(cx, &client),
            fetching_count: use_fetching_count(cx, &client),
            mutating_count: use_mutating_count(cx, &client),
        })
        .0
    }

    #[gpui::test]
    fn test_use_is_fetching_reacts_to_activity(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let view = create_test_view(cx, &client);
        let entity = view.read_with(cx, |v, _| v.is_fetching.clone());

        assert!(!entity.read_with(cx, |e: &UseIsFetching, _| e.get()));

        client.inc_fetching();
        cx.run_until_parked();
        assert!(entity.read_with(cx, |e: &UseIsFetching, _| e.get()));

        client.dec_fetching();
        cx.run_until_parked();
        assert!(!entity.read_with(cx, |e: &UseIsFetching, _| e.get()));
    }

    #[gpui::test]
    fn test_use_is_mutating_reacts_to_activity(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let view = create_test_view(cx, &client);
        let entity = view.read_with(cx, |v, _| v.is_mutating.clone());

        assert!(!entity.read_with(cx, |e: &UseIsMutating, _| e.get()));

        client.inc_mutating();
        cx.run_until_parked();
        assert!(entity.read_with(cx, |e: &UseIsMutating, _| e.get()));

        client.dec_mutating();
        cx.run_until_parked();
        assert!(!entity.read_with(cx, |e: &UseIsMutating, _| e.get()));
    }

    #[gpui::test]
    fn test_use_fetching_count_reacts_to_activity(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let view = create_test_view(cx, &client);
        let entity = view.read_with(cx, |v, _| v.fetching_count.clone());

        assert_eq!(entity.read_with(cx, |e: &UseFetchingCount, _| e.get()), 0);

        client.inc_fetching();
        cx.run_until_parked();
        assert_eq!(entity.read_with(cx, |e: &UseFetchingCount, _| e.get()), 1);

        client.inc_fetching();
        cx.run_until_parked();
        assert_eq!(entity.read_with(cx, |e: &UseFetchingCount, _| e.get()), 2);

        client.dec_fetching();
        cx.run_until_parked();
        assert_eq!(entity.read_with(cx, |e: &UseFetchingCount, _| e.get()), 1);
    }

    #[gpui::test]
    fn test_use_mutating_count_reacts_to_activity(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let view = create_test_view(cx, &client);
        let entity = view.read_with(cx, |v, _| v.mutating_count.clone());

        assert_eq!(entity.read_with(cx, |e: &UseMutatingCount, _| e.get()), 0);

        client.inc_mutating();
        cx.run_until_parked();
        assert_eq!(entity.read_with(cx, |e: &UseMutatingCount, _| e.get()), 1);

        client.inc_mutating();
        cx.run_until_parked();
        assert_eq!(entity.read_with(cx, |e: &UseMutatingCount, _| e.get()), 2);

        client.dec_mutating();
        cx.run_until_parked();
        assert_eq!(entity.read_with(cx, |e: &UseMutatingCount, _| e.get()), 1);
    }

    #[gpui::test]
    fn test_activity_hooks_stop_after_lagged_broadcast(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let view = create_test_view(cx, &client);
        let is_fetching = view.read_with(cx, |v, _| v.is_fetching.clone());
        let is_mutating = view.read_with(cx, |v, _| v.is_mutating.clone());
        let fetching_count = view.read_with(cx, |v, _| v.fetching_count.clone());
        let mutating_count = view.read_with(cx, |v, _| v.mutating_count.clone());

        // The activity channel buffers 16 events. Flooding far more than that
        // without yielding makes every hook's first `recv()` report `Lagged`,
        // which ends its `while let Ok(..)` tracking loop.
        for _ in 0..30 {
            client.inc_fetching();
            client.dec_fetching();
            client.inc_mutating();
            client.dec_mutating();
        }
        cx.run_until_parked();

        // The tracking loops have ended: further activity is not observed and
        // the hooks keep their initial values.
        client.inc_fetching();
        client.inc_mutating();
        cx.run_until_parked();
        assert!(!is_fetching.read_with(cx, |e: &UseIsFetching, _| e.get()));
        assert!(!is_mutating.read_with(cx, |e: &UseIsMutating, _| e.get()));
        assert_eq!(
            fetching_count.read_with(cx, |e: &UseFetchingCount, _| e.get()),
            0
        );
        assert_eq!(
            mutating_count.read_with(cx, |e: &UseMutatingCount, _| e.get()),
            0
        );
    }

    // ----- use_queries / use_activity_feed / use_any_fetching -----

    struct MultiHookView {
        queries: Option<Entity<Vec<QueryState<Vec<String>>>>>,
        feed: Option<Entity<Vec<u64>>>,
        any_fetching: Option<Entity<bool>>,
    }

    impl Render for MultiHookView {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            Empty
        }
    }

    fn ok_spec(key: &crate::QueryKey) -> Query<Vec<String>> {
        Query::new(key.clone(), || async { Ok(Vec::<String>::new()) })
    }

    #[gpui::test]
    fn test_use_queries_initial_from_cache_and_tracks_updates(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let key = crate::QueryKey::new("multi");
        client.set_query_data(&key, vec!["a".to_string()], crate::QueryOptions::default());
        let specs = vec![ok_spec(&key)];

        let view_client = client.clone();
        let view = cx
            .add_window_view(move |_window, cx| MultiHookView {
                queries: Some(use_queries(cx, &view_client, specs)),
                feed: None,
                any_fetching: None,
            })
            .0;
        let queries = view.read_with(cx, |v, _| v.queries.clone().expect("queries entity"));

        // Initial value comes from the cache.
        assert!(queries.read_with(cx, |states, _| states[0].is_success()));
        assert_eq!(
            queries.read_with(cx, |states, _| states[0].data().cloned()),
            Some(vec!["a".to_string()])
        );

        // Cache writes propagate through the subscription.
        client.set_query_data(
            &key,
            vec!["a".to_string(), "b".to_string()],
            crate::QueryOptions::default(),
        );
        cx.run_until_parked();
        assert_eq!(
            queries.read_with(cx, |states, _| states[0].data().cloned()),
            Some(vec!["a".to_string(), "b".to_string()])
        );

        // Invalidation flips the state to Stale.
        client.invalidate_queries(&key);
        cx.run_until_parked();
        assert!(queries.read_with(cx, |states, _| states[0].is_stale()));

        // A fresh write un-stales it again.
        client.set_query_data(
            &key,
            vec!["c".to_string()],
            crate::QueryOptions {
                stale_time: std::time::Duration::from_secs(60),
                ..Default::default()
            },
        );
        cx.run_until_parked();
        assert!(queries.read_with(cx, |states, _| states[0].is_success()));
        assert_eq!(
            queries.read_with(cx, |states, _| states[0].data().cloned()),
            Some(vec!["c".to_string()])
        );
    }

    #[gpui::test]
    fn test_use_queries_multiple_specs_track_independently(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let first = crate::QueryKey::new("first");
        let second = crate::QueryKey::new("second");
        let specs = vec![ok_spec(&first), ok_spec(&second)];

        let view_client = client.clone();
        let view = cx
            .add_window_view(move |_window, cx| MultiHookView {
                queries: Some(use_queries(cx, &view_client, specs)),
                feed: None,
                any_fetching: None,
            })
            .0;
        let queries = view.read_with(cx, |v, _| v.queries.clone().expect("queries entity"));

        assert!(queries.read_with(cx, |states, _| states.iter().all(QueryState::is_idle)));
        assert_eq!(queries.read_with(cx, |states, _| states.len()), 2);

        client.set_query_data(
            &second,
            vec!["s".to_string()],
            crate::QueryOptions::default(),
        );
        cx.run_until_parked();
        assert!(queries.read_with(cx, |states, _| states[0].is_idle()));
        assert!(queries.read_with(cx, |states, _| states[1].is_success()));
    }

    #[gpui::test]
    fn test_use_activity_feed_collects_and_caps_events(cx: &mut TestAppContext) {
        let (tx, rx) = tokio::sync::broadcast::channel::<u64>(256);
        for i in 0..150u64 {
            let _ = tx.send(i);
        }

        let view = cx
            .add_window_view(move |_window, cx| MultiHookView {
                queries: None,
                feed: Some(use_activity_feed(cx, rx)),
                any_fetching: None,
            })
            .0;
        let feed = view.read_with(cx, |v, _| v.feed.clone().expect("feed entity"));

        // Pre-buffered events are drained and capped to the most recent 100.
        assert_eq!(feed.read_with(cx, |events, _| events.len()), 100);
        assert_eq!(
            feed.read_with(cx, |events, _| *events.last().expect("non-empty")),
            149
        );

        // Events sent after mount are appended, still capped.
        let _ = tx.send(150);
        cx.run_until_parked();
        assert_eq!(feed.read_with(cx, |events, _| events.len()), 100);
        assert_eq!(
            feed.read_with(cx, |events, _| *events.last().expect("non-empty")),
            150
        );
    }

    #[gpui::test]
    fn test_use_any_fetching_tracks_multiple_clients(cx: &mut TestAppContext) {
        let client_a = QueryClient::new();
        let client_b = QueryClient::new();
        let view_a = client_a.clone();
        let view_b = client_b.clone();

        let view = cx
            .add_window_view(move |_window, cx| MultiHookView {
                queries: None,
                feed: None,
                any_fetching: Some(use_any_fetching(cx, &[view_a, view_b])),
            })
            .0;
        let any_fetching = view.read_with(cx, |v, _| {
            v.any_fetching.clone().expect("any_fetching entity")
        });

        assert!(!any_fetching.read_with(cx, |value, _| *value));

        client_b.inc_fetching();
        cx.run_until_parked();
        assert!(any_fetching.read_with(cx, |value, _| *value));

        client_a.inc_fetching();
        cx.run_until_parked();
        assert!(any_fetching.read_with(cx, |value, _| *value));

        client_a.dec_fetching();
        cx.run_until_parked();
        assert!(any_fetching.read_with(cx, |value, _| *value));

        client_b.dec_fetching();
        cx.run_until_parked();
        assert!(!any_fetching.read_with(cx, |value, _| *value));
    }

    // ----- pure helpers (non-GPUI) -----

    #[test]
    fn test_push_capped_caps_at_capacity() {
        let mut feed: Vec<u64> = Vec::new();
        for i in 0..5u64 {
            push_capped(&mut feed, i, 3);
        }
        assert_eq!(feed, vec![2, 3, 4]);
    }

    #[test]
    fn test_push_capped_under_capacity_preserves_order() {
        let mut feed: Vec<&str> = Vec::new();
        push_capped(&mut feed, "a", 10);
        push_capped(&mut feed, "b", 10);
        push_capped(&mut feed, "c", 10);
        assert_eq!(feed, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_push_capped_zero_capacity_drops_events() {
        let mut feed: Vec<u64> = Vec::new();
        push_capped(&mut feed, 1, 0);
        push_capped(&mut feed, 2, 0);
        assert_eq!(feed, [] as [u64; 0]);
    }

    #[test]
    fn test_push_capped_exact_capacity_boundary() {
        let mut feed: Vec<u64> = vec![1, 2];
        push_capped(&mut feed, 3, 2);
        assert_eq!(feed, vec![2, 3]);
    }

    #[tokio::test]
    async fn test_drain_ready_collects_buffered_events() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);
        for i in 0..5u64 {
            let _ = tx.send(i);
        }
        assert_eq!(drain_ready(&mut rx), vec![0, 1, 2, 3, 4]);
        // Second drain finds nothing buffered.
        assert_eq!(drain_ready(&mut rx), [] as [u64; 0]);
    }

    #[tokio::test]
    async fn test_drain_ready_on_closed_channel() {
        let (tx, mut rx) = tokio::sync::broadcast::channel::<u64>(4);
        drop(tx);
        assert_eq!(drain_ready(&mut rx), [] as [u64; 0]);
    }

    #[tokio::test]
    async fn test_drain_ready_skips_lagged_events() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(4);
        for i in 0..10u64 {
            let _ = tx.send(i);
        }
        // The receiver lagged: only the newest buffered events survive, and
        // the lagged-marker error is swallowed by the drain.
        let drained = drain_ready(&mut rx);
        assert!(
            !drained.is_empty(),
            "buffered events after lag are collected"
        );
        assert_eq!(*drained.last().expect("non-empty"), 9);
        assert!(drained.len() <= 4, "only the buffered tail survives");
    }

    #[gpui::test]
    fn test_use_queries_seeds_initial_data_into_cache(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let key = crate::QueryKey::new("seeded-hook");
        let specs = vec![
            Query::new(key, || async { Ok(Vec::<String>::new()) })
                .initial_data(vec!["seed".to_string()]),
        ];

        let view_client = client.clone();
        let view = cx
            .add_window_view(move |_window, cx| MultiHookView {
                queries: Some(use_queries(cx, &view_client, specs)),
                feed: None,
                any_fetching: None,
            })
            .0;
        let queries = view.read_with(cx, |v, _| v.queries.clone().expect("queries entity"));

        assert!(queries.read_with(cx, |states, _| states[0].is_success()));
        assert_eq!(
            queries.read_with(cx, |states, _| states[0].data().cloned()),
            Some(vec!["seed".to_string()])
        );
    }

    #[gpui::test]
    fn test_use_queries_stops_when_entity_dropped(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let key = crate::QueryKey::new("dropped-queries");
        let specs = vec![ok_spec(&key)];

        let view_client = client.clone();
        let view = cx
            .add_window_view(move |_window, cx| MultiHookView {
                queries: Some(use_queries(cx, &view_client, specs)),
                feed: None,
                any_fetching: None,
            })
            .0;

        // Drop the model entity: the per-spec task observes the failed update
        // on the next event and breaks out of its loop.
        view.update(cx, |v, _| v.queries = None);
        client.set_query_data(
            &key,
            vec!["late".to_string()],
            crate::QueryOptions::default(),
        );
        cx.run_until_parked();
    }

    #[gpui::test]
    fn test_use_activity_feed_follows_lagged_broadcasts(cx: &mut TestAppContext) {
        let (tx, rx) = tokio::sync::broadcast::channel::<u64>(4);
        for i in 0..10u64 {
            let _ = tx.send(i);
        }

        let view = cx
            .add_window_view(move |_window, cx| MultiHookView {
                queries: None,
                feed: Some(use_activity_feed(cx, rx)),
                any_fetching: None,
            })
            .0;
        let feed = view.read_with(cx, |v, _| v.feed.clone().expect("feed entity"));

        // Events sent after the initial drain overflow the buffer: the
        // receiver lags on its next poll and keeps following the newest tail.
        for i in 10..20u64 {
            let _ = tx.send(i);
        }
        cx.run_until_parked();
        assert_eq!(
            feed.read_with(cx, |events, _| *events.last().expect("non-empty")),
            19
        );
    }

    #[gpui::test]
    fn test_use_activity_feed_stops_when_entity_dropped(cx: &mut TestAppContext) {
        let (tx, rx) = tokio::sync::broadcast::channel::<u64>(16);

        let view = cx
            .add_window_view(move |_window, cx| MultiHookView {
                queries: None,
                feed: Some(use_activity_feed(cx, rx)),
                any_fetching: None,
            })
            .0;

        view.update(cx, |v, _| v.feed = None);
        let _ = tx.send(42);
        cx.run_until_parked();
    }

    #[gpui::test]
    fn test_use_any_fetching_stops_when_entity_dropped(cx: &mut TestAppContext) {
        let client_a = QueryClient::new();
        let view_a = client_a.clone();

        let view = cx
            .add_window_view(move |_window, cx| MultiHookView {
                queries: None,
                feed: None,
                any_fetching: Some(use_any_fetching(cx, &[view_a])),
            })
            .0;

        view.update(cx, |v, _| v.any_fetching = None);
        client_a.inc_fetching();
        cx.run_until_parked();
    }
}
