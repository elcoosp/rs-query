// src/observer.rs
//! Query observer for reactive state updates

use crate::network::can_fetch;
use crate::options::PlaceholderData;
use crate::{QueryClient, QueryError, QueryKey, QueryOptions, QueryState};
use std::marker::PhantomData;
use std::time::Instant;
use tokio::sync::broadcast;

/// Variant of a query state update, used for notifications.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum QueryStateVariant {
    /// No query is registered or the cache entry went away.
    Idle,
    /// A first fetch is in flight and no data is cached yet.
    Loading,
    /// A background refetch of existing (stale) data is in flight.
    Refetching,
    /// The latest fetch completed successfully with fresh data.
    Success,
    /// Data is available but past its stale time.
    Stale,
    /// The latest fetch failed.
    Error,
}

/// A state update notification sent to subscribers.
#[derive(Clone, Debug)]
pub struct QueryStateUpdate {
    /// Cache key of the query whose state changed.
    pub key: String,
    /// The kind of state transition that happened.
    pub state_variant: QueryStateVariant,
}

/// TanStack-style derived result flags for a [`QueryObserver`].
///
/// Mirrors the boolean/timestamp block of `TanStack` Query's observer result:
/// `dataUpdatedAt`, `errorUpdatedAt`, `errorUpdateCount`, `failureCount`,
/// `failureReason`, `isFetched`, `isFetchedAfterMount`, `isLoadingError`,
/// `isRefetchError`, `isPlaceholderData`, `isPaused` and `isEnabled`.
#[derive(Clone, Debug)]
pub struct QueryResultFlags {
    /// When the last successful data write happened (observer-local).
    pub data_updated_at: Option<Instant>,
    /// When the last error happened (observer-local).
    pub error_updated_at: Option<Instant>,
    /// How many errors this observer has observed.
    pub error_update_count: u32,
    /// Failed attempts observed for the current fetch (reset on new fetch
    /// and on success).
    pub failure_count: u32,
    /// The last failure reason, if any.
    pub failure_reason: Option<QueryError>,
    /// Whether any fetch has completed (success or error).
    pub is_fetched: bool,
    /// Whether a fetch completed after the observer was constructed.
    pub is_fetched_after_mount: bool,
    /// The last fetch failed and no data is available.
    pub is_loading_error: bool,
    /// The last fetch failed but data is available.
    pub is_refetch_error: bool,
    /// Placeholder data is currently shown (loading with placeholder).
    pub is_placeholder_data: bool,
    /// The network mode forbids starting a fetch under the current
    /// connectivity (`TanStack` `isPaused`).
    pub is_paused: bool,
    /// Whether the observer's options resolve to enabled.
    pub is_enabled: bool,
}

/// Reactive observer for a query key.
pub struct QueryObserver<T: Clone + Send + Sync + 'static> {
    /// Query key this observer watches.
    pub key: QueryKey,
    state: QueryState<T>,
    client: QueryClient,
    receiver: broadcast::Receiver<QueryStateUpdate>,
    options: QueryOptions,
    fetch_started_at: Option<Instant>,
    placeholder_data: Option<PlaceholderData<T>>,
    data_updated_at: Option<Instant>,
    error_updated_at: Option<Instant>,
    error_update_count: u32,
    failure_count: u32,
    failure_reason: Option<QueryError>,
    is_fetched: bool,
    is_fetched_after_mount: bool,
    _marker: PhantomData<T>,
}

impl<T: Clone + Send + Sync + 'static> Clone for QueryObserver<T> {
    fn clone(&self) -> Self {
        let receiver = self.client.subscribe(self.key.cache_key());
        Self {
            key: self.key.clone(),
            state: self.state.clone(),
            client: self.client.clone(),
            receiver,
            options: self.options.clone(),
            fetch_started_at: self.fetch_started_at,
            placeholder_data: self.placeholder_data.clone(),
            data_updated_at: self.data_updated_at,
            error_updated_at: self.error_updated_at,
            error_update_count: self.error_update_count,
            failure_count: self.failure_count,
            failure_reason: self.failure_reason.clone(),
            is_fetched: self.is_fetched,
            is_fetched_after_mount: self.is_fetched_after_mount,
            _marker: PhantomData,
        }
    }
}

impl<T: Clone + Send + Sync + 'static> QueryObserver<T> {
    /// Create a new observer for a key.
    #[must_use]
    pub fn new(client: &QueryClient, key: QueryKey) -> Self {
        let cache_key = key.cache_key().to_string();
        let receiver = client.subscribe(&cache_key);

        let state = if let Some(data) = client.get_query_data::<T>(&key) {
            if client.is_stale(&key) {
                QueryState::Stale(data)
            } else {
                QueryState::Success(data)
            }
        } else {
            QueryState::Idle
        };

        Self {
            key,
            state,
            client: client.clone(),
            receiver,
            options: QueryOptions::default(),
            fetch_started_at: None,
            placeholder_data: None,
            data_updated_at: None,
            error_updated_at: None,
            error_update_count: 0,
            failure_count: 0,
            failure_reason: None,
            is_fetched: false,
            is_fetched_after_mount: false,
            _marker: PhantomData,
        }
    }

    /// Create an observer from a query definition, applying initial/placeholder data.
    pub fn from_query(client: &QueryClient, query: &crate::query::Query<T>) -> Self
    where
        T: PartialEq, // required because set_query_data needs PartialEq
    {
        let key = query.key.clone();
        let cache_key = key.cache_key().to_string();
        let receiver = client.subscribe(&cache_key);

        let state = if let Some(data) = client.get_query_data::<T>(&key) {
            if client.is_stale(&key) {
                QueryState::Stale(data)
            } else {
                QueryState::Success(data)
            }
        } else if let Some(initial) = &query.initial_data {
            client.set_query_data(&key, initial.clone(), query.options.clone());
            QueryState::Success(initial.clone())
        } else {
            QueryState::Idle
        };

        Self {
            key,
            state,
            client: client.clone(),
            receiver,
            options: query.options.clone(),
            fetch_started_at: None,
            placeholder_data: query.placeholder_data.clone(),
            data_updated_at: None,
            error_updated_at: None,
            error_update_count: 0,
            failure_count: 0,
            failure_reason: None,
            is_fetched: false,
            is_fetched_after_mount: false,
            _marker: PhantomData,
        }
    }

    /// Create an observer with explicit query options.
    #[must_use]
    pub fn with_options(client: &QueryClient, key: QueryKey, options: QueryOptions) -> Self {
        let mut observer = Self::new(client, key);
        observer.options = options;
        observer
    }

    /// Current observer state.
    pub fn state(&self) -> &QueryState<T> {
        &self.state
    }

    /// The client this observer is attached to.
    pub fn client(&self) -> &QueryClient {
        &self.client
    }

    /// Re-read the cache and recompute the observer state.
    ///
    /// Maps the freshly observed data (if any) onto the current state:
    /// fetch-in-progress states (`Loading`, `LoadingWithPlaceholder`,
    /// `Refetching`) resolve into `Stale`/`Success` once data appears, an
    /// empty cache yields `Idle`.
    pub fn update(&mut self) {
        let data: Option<T> = self.client.get_query_data(&self.key);
        let is_stale = self.client.is_stale(&self.key);

        self.state = match (&self.state, data, is_stale) {
            (_, None, _) => QueryState::Idle,
            // Any fetch-in-progress state resolves the same way once data is
            // present: stale data marks the result stale, fresh data succeeds.
            (
                QueryState::Loading
                | QueryState::LoadingWithPlaceholder(_)
                | QueryState::Refetching(_),
                Some(data),
                _,
            ) => {
                if is_stale {
                    QueryState::Stale(data)
                } else {
                    QueryState::Success(data)
                }
            }
            (_, Some(data), true) => QueryState::Stale(data),
            (_, Some(data), false) => QueryState::Success(data),
        };
    }

    /// Mark a fetch as started, resetting failure bookkeeping.
    ///
    /// Existing data moves to `Refetching`; otherwise placeholder data (if
    /// configured) resolves into `LoadingWithPlaceholder`, else `Loading`.
    pub fn set_loading(&mut self) {
        self.fetch_started_at = Some(Instant::now());
        // A new fetch resets the failure bookkeeping (TanStack `'fetch'`).
        self.failure_count = 0;
        self.failure_reason = None;
        if let Some(data) = self.state.data_cloned() {
            self.state = QueryState::Refetching(data);
        } else if let Some(placeholder) = &self.placeholder_data {
            let resolved = placeholder.resolve(None);
            self.state = QueryState::LoadingWithPlaceholder(resolved);
        } else {
            self.state = QueryState::Loading;
        }
    }

    /// Record a successful fetch, updating temporal bookkeeping.
    pub fn set_success(&mut self, data: T) {
        let had_fetch = self.fetch_started_at.is_some();
        self.fetch_started_at = None;
        self.state = QueryState::Success(data);
        self.data_updated_at = Some(Instant::now());
        self.is_fetched = true;
        if had_fetch {
            self.is_fetched_after_mount = true;
        }
        self.failure_count = 0;
        self.failure_reason = None;
    }

    /// Record a failed fetch, keeping stale data and incrementing the
    /// `TanStack` failure counters.
    pub fn set_error(&mut self, error: QueryError) {
        let had_fetch = self.fetch_started_at.is_some();
        self.fetch_started_at = None;
        let stale_data = self.state.data_cloned();
        self.state = QueryState::Error {
            error: error.clone(),
            stale_data,
        };
        self.error_updated_at = Some(Instant::now());
        self.error_update_count = self.error_update_count.saturating_add(1);
        self.failure_count = self.failure_count.saturating_add(1);
        self.failure_reason = Some(error);
        self.is_fetched = true;
        if had_fetch {
            self.is_fetched_after_mount = true;
        }
    }

    /// Mark the current data as stale (if any data is present).
    pub fn set_stale(&mut self) {
        if let Some(data) = self.state.data_cloned() {
            self.state = QueryState::Stale(data);
            self.data_updated_at = Some(Instant::now());
        }
    }

    /// Non-blockingly try to receive the next state update.
    ///
    /// Skips over messages lost to a lagged broadcast receiver and returns
    /// the last one recovered, if any; `None` when nothing is pending or the
    /// channel closed.
    pub fn try_recv(&mut self) -> Option<QueryStateUpdate> {
        match self.receiver.try_recv() {
            Ok(update) => Some(update),
            Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed) => {
                None
            }
            Err(broadcast::error::TryRecvError::Lagged(n)) => {
                let mut last = None;
                for _ in 0..n {
                    match self.receiver.try_recv() {
                        Ok(u) => last = Some(u),
                        Err(_) => break,
                    }
                }
                last
            }
        }
    }

    /// Invalidate the query, triggering a refetch on the next render.
    pub fn refetch(&mut self) {
        self.client.invalidate_queries(&self.key);
    }

    /// Refetch with `TanStack` `RefetchOptions`: when `cancel_refetch` is true
    /// (the `TanStack` default) any in-flight fetch for this key is cancelled
    /// through the client before invalidating.
    pub fn refetch_with(&mut self, cancel_refetch: bool) {
        if cancel_refetch {
            self.client.cancel_query(&self.key);
        }
        self.refetch();
    }

    /// TanStack-style derived result flags for the current observer state.
    ///
    /// The temporal flags (`data_updated_at`, `error_updated_at`,
    /// `error_update_count`, `failure_count`, `failure_reason`, `is_fetched`,
    /// `is_fetched_after_mount`) are tracked on the observer; the derived
    /// flags (`is_loading_error`, `is_refetch_error`, `is_placeholder_data`,
    /// `is_paused`, `is_enabled`) are computed from the current state,
    /// options and connectivity.
    pub fn flags(&self) -> QueryResultFlags {
        QueryResultFlags {
            data_updated_at: self.data_updated_at,
            error_updated_at: self.error_updated_at,
            error_update_count: self.error_update_count,
            failure_count: self.failure_count,
            failure_reason: self.failure_reason.clone(),
            is_fetched: self.is_fetched,
            is_fetched_after_mount: self.is_fetched_after_mount,
            is_loading_error: self.state.is_error() && !self.state.has_data(),
            is_refetch_error: self.state.is_error() && self.state.has_data(),
            is_placeholder_data: matches!(self.state, QueryState::LoadingWithPlaceholder(_)),
            is_paused: !can_fetch(
                self.options.network_mode,
                self.client.online_manager().is_online(),
            ),
            is_enabled: self.options.is_enabled(),
        }
    }

    /// The query options currently configured on this observer.
    pub fn options(&self) -> &QueryOptions {
        &self.options
    }

    /// When the currently running fetch started, if one is in flight.
    pub fn fetch_started_at(&self) -> Option<Instant> {
        self.fetch_started_at
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_observer_new_empty_cache() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let observer: QueryObserver<String> = QueryObserver::new(&client, key.clone());
        assert!(observer.state().is_idle());
        assert_eq!(observer.key, key);
    }

    #[test]
    fn test_observer_new_with_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "hello".to_string(), QueryOptions::default());

        let observer: QueryObserver<String> = QueryObserver::new(&client, key.clone());
        assert!(observer.state().is_success());
        assert_eq!(observer.state().data(), Some(&"hello".to_string()));
    }

    #[test]
    fn test_observer_new_with_stale_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let opts = QueryOptions {
            stale_time: Duration::from_millis(1),
            ..Default::default()
        };
        client.set_query_data(&key, "stale".to_string(), opts);
        std::thread::sleep(Duration::from_millis(5));

        let observer: QueryObserver<String> = QueryObserver::new(&client, key.clone());
        assert!(observer.state().is_stale());
    }

    #[test]
    fn test_observer_with_options() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let opts = QueryOptions {
            stale_time: Duration::from_secs(60),
            ..Default::default()
        };
        let observer: QueryObserver<String> =
            QueryObserver::with_options(&client, key, opts.clone());
        assert_eq!(observer.options().stale_time, Duration::from_secs(60));
    }

    #[test]
    fn test_observer_set_loading_no_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);
        observer.set_loading();
        assert!(observer.state().is_loading());
        assert!(observer.fetch_started_at().is_some());
    }

    #[test]
    fn test_observer_set_loading_with_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key.clone());
        observer.set_success("existing".to_string());
        observer.set_loading();
        assert!(observer.state().is_refetching());
        assert_eq!(observer.state().data(), Some(&"existing".to_string()));
    }

    #[test]
    fn test_observer_set_success() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);
        observer.set_loading();
        observer.set_success("data".to_string());
        assert!(observer.state().is_success());
        assert_eq!(observer.state().data(), Some(&"data".to_string()));
        assert!(observer.fetch_started_at().is_none());
    }

    #[test]
    fn test_observer_set_error_no_stale_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);
        observer.set_loading();
        observer.set_error(QueryError::network("fail"));
        assert!(observer.state().is_error());
        assert_eq!(observer.state().data(), None);
    }

    #[test]
    fn test_observer_set_error_with_stale_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);
        observer.set_success("old".to_string());
        observer.set_error(QueryError::timeout("30s"));
        assert!(observer.state().is_error());
        assert_eq!(observer.state().data(), Some(&"old".to_string()));
    }

    #[test]
    fn test_observer_set_stale() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);
        observer.set_success("data".to_string());
        observer.set_stale();
        assert!(observer.state().is_stale());
        assert_eq!(observer.state().data(), Some(&"data".to_string()));
    }

    #[test]
    fn test_observer_set_stale_no_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);
        observer.set_stale();
        assert!(observer.state().is_idle());
    }

    #[test]
    fn test_observer_update_no_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);
        observer.set_success("data".to_string());
        observer.update();
        assert!(observer.state().is_idle());
    }

    #[test]
    fn test_observer_update_with_fresh_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "fresh".to_string(), QueryOptions::default());
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);
        observer.set_loading();
        observer.update();
        assert!(observer.state().is_success());
    }

    #[test]
    fn test_observer_update_with_stale_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let opts = QueryOptions {
            stale_time: Duration::from_millis(1),
            ..Default::default()
        };
        client.set_query_data(&key, "stale".to_string(), opts);
        std::thread::sleep(Duration::from_millis(5));

        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);
        observer.set_loading();
        observer.update();
        assert!(observer.state().is_stale());
    }

    #[test]
    fn test_observer_update_refetching_to_fresh() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "fresh".to_string(), QueryOptions::default());
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);
        observer.set_success("old".to_string());
        observer.set_loading();
        observer.update();
        assert!(observer.state().is_success());
    }

    #[test]
    fn test_observer_update_refetching_to_stale() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let opts = QueryOptions {
            stale_time: Duration::from_millis(1),
            ..Default::default()
        };
        client.set_query_data(&key, "stale".to_string(), opts);
        std::thread::sleep(Duration::from_millis(5));

        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);
        observer.set_success("old".to_string());
        observer.set_loading();
        observer.update();
        assert!(observer.state().is_stale());
    }

    #[test]
    fn test_observer_try_recv_empty() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);
        assert!(observer.try_recv().is_none());
    }

    #[test]
    fn test_observer_try_recv_lagged() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let cache_key = key.cache_key().to_string();
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);

        for _ in 0..20 {
            client.notify_subscribers(&cache_key, QueryStateVariant::Success);
        }

        let _result = observer.try_recv();
    }

    #[test]
    fn test_observer_refetch() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key.clone());
        observer.refetch();
        assert!(client.is_stale(&key));
    }

    #[test]
    fn test_observer_client() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let observer: QueryObserver<String> = QueryObserver::new(&client, key);
        let _ = observer.client();
    }

    #[test]
    fn test_observer_clone() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let observer: QueryObserver<String> = QueryObserver::new(&client, key.clone());
        let cloned = observer.clone();
        assert_eq!(cloned.key, key);
        // Cloned observer has a new receiver
        assert!(cloned.state().is_idle());
    }

    #[test]
    fn test_query_state_variant_equality() {
        assert_eq!(QueryStateVariant::Idle, QueryStateVariant::Idle);
        assert_eq!(QueryStateVariant::Loading, QueryStateVariant::Loading);
        assert_ne!(QueryStateVariant::Success, QueryStateVariant::Error);
    }

    #[test]
    fn test_query_state_variant_clone() {
        // `QueryStateVariant` is `Copy`: cloning is a trivial copy.
        let v = QueryStateVariant::Success;
        let cloned = v;
        assert_eq!(v, cloned);
    }

    #[test]
    fn test_query_state_variant_debug() {
        let v = QueryStateVariant::Error;
        let debug = format!("{v:?}");
        assert!(debug.contains("Error"));
    }

    #[test]
    fn test_query_state_update_debug() {
        let update = QueryStateUpdate {
            key: "test".to_string(),
            state_variant: QueryStateVariant::Success,
        };
        let debug = format!("{update:?}");
        assert!(debug.contains("test"));
    }

    // ----- TanStack-style result flags -----

    #[test]
    fn test_flags_defaults() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let observer: QueryObserver<String> = QueryObserver::new(&client, key);

        let flags = observer.flags();
        assert!(flags.data_updated_at.is_none());
        assert!(flags.error_updated_at.is_none());
        assert_eq!(flags.error_update_count, 0);
        assert_eq!(flags.failure_count, 0);
        assert!(flags.failure_reason.is_none());
        assert!(!flags.is_fetched);
        assert!(!flags.is_fetched_after_mount);
        assert!(!flags.is_loading_error);
        assert!(!flags.is_refetch_error);
        assert!(!flags.is_placeholder_data);
        assert!(!flags.is_paused, "online by default");
        assert!(flags.is_enabled);
    }

    #[test]
    fn test_flags_after_success_sets_data_updated_at_and_is_fetched() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);

        let before = Instant::now();
        observer.set_loading();
        observer.set_success("data".to_string());

        let flags = observer.flags();
        assert!(flags.is_fetched);
        assert!(flags.is_fetched_after_mount, "fetch completed after mount");
        assert!(flags.data_updated_at.is_some());
        assert!(flags.data_updated_at.unwrap() >= before);
        assert!(flags.error_updated_at.is_none());
        assert_eq!(flags.failure_count, 0);
    }

    #[test]
    fn test_flags_success_without_fetch_is_not_fetched_after_mount() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);

        // No fetch was in progress: is_fetched_after_mount stays false.
        observer.set_success("data".to_string());
        let flags = observer.flags();
        assert!(flags.is_fetched);
        assert!(!flags.is_fetched_after_mount);
    }

    #[test]
    fn test_flags_error_bookkeeping() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);

        observer.set_loading();
        observer.set_error(QueryError::network("fail"));

        let flags = observer.flags();
        assert_eq!(flags.error_update_count, 1);
        assert_eq!(flags.failure_count, 1);
        assert_eq!(flags.failure_reason, Some(QueryError::network("fail")));
        assert!(flags.error_updated_at.is_some());
        assert!(flags.is_fetched);
        assert!(flags.is_fetched_after_mount);
        assert!(
            flags.is_loading_error,
            "error without data is a loading error"
        );
        assert!(!flags.is_refetch_error);

        // A second error increments the counters again.
        observer.set_error(QueryError::timeout("late"));
        let flags = observer.flags();
        assert_eq!(flags.error_update_count, 2);
        assert_eq!(flags.failure_count, 2);
    }

    #[test]
    fn test_flags_refetch_error_with_stale_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);

        observer.set_success("old".to_string());
        observer.set_error(QueryError::network("refetch failed"));

        let flags = observer.flags();
        assert!(flags.is_refetch_error);
        assert!(!flags.is_loading_error);
    }

    #[test]
    fn test_flags_new_fetch_resets_failures_and_success_clears_them() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);

        observer.set_error(QueryError::network("fail"));
        assert_eq!(observer.flags().failure_count, 1);

        observer.set_loading();
        let flags = observer.flags();
        assert_eq!(flags.failure_count, 0);
        assert!(flags.failure_reason.is_none());

        observer.set_error(QueryError::network("fail again"));
        observer.set_success("data".to_string());
        let flags = observer.flags();
        assert_eq!(flags.failure_count, 0);
        assert!(flags.failure_reason.is_none());
    }

    #[test]
    fn test_flags_placeholder_data() {
        let client = QueryClient::new();
        let query: crate::query::Query<String> =
            crate::query::Query::new(QueryKey::new("test"), || async { Ok("real".to_string()) })
                .placeholder_data(PlaceholderData::value("placeholder".to_string()));

        let mut observer: QueryObserver<String> = QueryObserver::from_query(&client, &query);
        observer.set_loading();
        assert!(observer.flags().is_placeholder_data);

        observer.set_success("real".to_string());
        assert!(!observer.flags().is_placeholder_data);
    }

    #[test]
    fn test_flags_paused_when_offline() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let observer: QueryObserver<String> = QueryObserver::new(&client, key.clone());

        assert!(!observer.flags().is_paused);
        client.online_manager().set_online(false);
        assert!(
            observer.flags().is_paused,
            "online mode gates on connectivity"
        );

        // `always` mode never pauses.
        let observer: QueryObserver<String> = QueryObserver::with_options(
            &client,
            key,
            QueryOptions {
                network_mode: crate::network::NetworkMode::Always,
                ..Default::default()
            },
        );
        assert!(!observer.flags().is_paused);
    }

    #[test]
    fn test_flags_disabled_options() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let observer: QueryObserver<String> =
            QueryObserver::with_options(&client, key, QueryOptions::default().enabled(false));
        assert!(!observer.flags().is_enabled);
    }

    #[test]
    fn test_flags_clone_and_debug() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);
        observer.set_success("data".to_string());

        let flags = observer.flags();
        let cloned = flags.clone();
        assert_eq!(cloned.error_update_count, flags.error_update_count);
        let debug = format!("{flags:?}");
        assert!(debug.contains("is_fetched"));
    }

    #[tokio::test]
    async fn test_refetch_with_cancels_in_flight_first() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key.clone());
        let spawn_sleeper = || {
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            })
        };
        // One spawn site serves both needs: the first task runs the spawned
        // async block to completion, while a second instance backs the
        // in-flight registration that `refetch_with(true)` cancels below.
        spawn_sleeper().await.expect("sleeper task completes");
        let handle = spawn_sleeper();
        client.set_in_flight(
            &key,
            crate::client::InFlightTask {
                abort_handle: handle.abort_handle(),
                cancel_token: tokio_util::sync::CancellationToken::new(),
            },
        );

        observer.refetch_with(true);

        assert!(!client.is_in_flight(&key), "in-flight fetch was cancelled");
        assert!(client.is_stale(&key));
    }

    #[tokio::test]
    async fn test_refetch_with_no_cancel_still_invalidates() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key.clone());
        observer.refetch_with(false);

        assert!(client.is_stale(&key));
    }

    // ----- Coverage: from_query cache/initial-data branches -----

    #[tokio::test]
    async fn test_observer_from_query_uses_cached_fresh_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "cached".to_string(), QueryOptions::default());

        let query = crate::query::Query::new(key, || async {
            Ok::<String, QueryError>("fetched".to_string())
        });
        let observer: QueryObserver<String> = QueryObserver::from_query(&client, &query);
        assert!(observer.state().is_success());
        assert_eq!(observer.state().data(), Some(&"cached".to_string()));

        // The registered fetch function is runnable too: execute the query
        // once and observe its data land.
        let state = crate::executor::execute_query(
            &client,
            &query,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
        assert_eq!(state.data(), Some(&"fetched".to_string()));
    }

    #[tokio::test]
    async fn test_observer_from_query_uses_cached_stale_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let opts = QueryOptions {
            stale_time: Duration::from_millis(1),
            ..Default::default()
        };
        client.set_query_data(&key, "cached".to_string(), opts);
        std::thread::sleep(Duration::from_millis(5));

        let query = crate::query::Query::new(key, || async {
            Ok::<String, QueryError>("fetched".to_string())
        });
        let observer: QueryObserver<String> = QueryObserver::from_query(&client, &query);
        assert!(observer.state().is_stale());

        // The registered fetch function is runnable too: executing the query
        // replaces the stale data with the fetched value.
        let state = crate::executor::execute_query(
            &client,
            &query,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"fetched".to_string()));
    }

    #[tokio::test]
    async fn test_observer_from_query_applies_initial_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query = crate::query::Query::new(key.clone(), || async {
            Ok::<String, QueryError>("fetched".to_string())
        })
        .initial_data("initial".to_string());

        let observer: QueryObserver<String> = QueryObserver::from_query(&client, &query);
        assert!(observer.state().is_success());
        assert_eq!(observer.state().data(), Some(&"initial".to_string()));
        // Initial data is also written to the cache.
        let cached: Option<String> = client.get_query_data(&key);
        assert_eq!(cached, Some("initial".to_string()));

        // The registered fetch function is runnable too: executing the query
        // overwrites the seeded initial data.
        let state = crate::executor::execute_query(
            &client,
            &query,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
        assert_eq!(state.data(), Some(&"fetched".to_string()));
    }

    // ----- Coverage: update()/try_recv branches -----

    #[test]
    fn test_observer_update_success_state_to_stale() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key.clone());
        assert!(observer.state().is_success(), "fresh cached data");

        client.invalidate_queries(&key);
        observer.update();

        assert!(observer.state().is_stale());
        assert_eq!(observer.state().data(), Some(&"data".to_string()));
    }

    #[test]
    fn test_observer_update_error_state_with_fresh_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key.clone());
        observer.set_error(QueryError::network("fail"));

        client.set_query_data(&key, "fresh".to_string(), QueryOptions::default());
        observer.update();

        assert!(observer.state().is_success());
    }

    #[test]
    fn test_observer_try_recv_returns_pending_update() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let cache_key = key.cache_key().to_string();
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);

        client.notify_subscribers(&cache_key, QueryStateVariant::Stale);

        let update = observer
            .try_recv()
            .expect("update should be pending on the channel");
        assert_eq!(update.key, cache_key);
        assert_eq!(update.state_variant, QueryStateVariant::Stale);
    }

    #[test]
    fn test_observer_try_recv_lagged_recovers_last_surviving_update() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let cache_key = key.cache_key().to_string();
        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);

        // Far more notifications than the broadcast capacity (16): the first
        // try_recv reports lag, then drains the surviving messages and the
        // drain loop stops on `Empty`.
        for i in 0..40 {
            client.notify_subscribers(
                &cache_key,
                if i == 39 {
                    QueryStateVariant::Error
                } else {
                    QueryStateVariant::Success
                },
            );
        }

        let update = observer.try_recv().expect("lagged recv should recover");
        assert_eq!(update.key, cache_key);
        assert_eq!(update.state_variant, QueryStateVariant::Error);

        // The channel is fully drained afterwards.
        assert!(observer.try_recv().is_none());
    }
}
