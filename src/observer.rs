// src/observer.rs
//! Reactive observer that subscribes to cache changes for a specific query key.

use crate::client::QueryClient;
use crate::key::QueryKey;
use crate::options::QueryOptions;
use crate::query::{PlaceholderData, Query};
use crate::state::{QueryState, QueryStateUpdate, QueryStateVariant};
use std::marker::PhantomData;
use std::time::Instant;
use tokio::sync::broadcast;

/// Reactive observer for a query.
///
/// Subscribes to cache updates via a broadcast channel and exposes the current
/// [`QueryState<T>`]. Use [`QueryObserver::new`] for simple cases or
/// [`QueryObserver::from_query`] to automatically apply `initialData`.
pub struct QueryObserver<T: Clone + Send + Sync + 'static> {
    key: QueryKey,
    state: QueryState<T>,
    client: QueryClient,
    receiver: broadcast::Receiver<QueryStateUpdate>,
    options: QueryOptions,
    fetch_started_at: Option<Instant>,
    /// Placeholder data resolved from the query definition, used during loading.
    placeholder_data: Option<PlaceholderData<T>>,
    _marker: PhantomData<T>,
}

impl<T: Clone + Send + Sync + 'static> QueryObserver<T> {
    /// Create an observer for a query key using an existing client.
    ///
    /// This reads the current cache state but does **not** apply `initialData`.
    /// Use [`from_query`](Self::from_query) for full initial-data support.
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
            options: client.get_query_options(&key).unwrap_or_default(),
            fetch_started_at: None,
            placeholder_data: None,
            _marker: PhantomData,
        }
    }

    /// Create an observer from a [`Query`] definition.
    ///
    /// This applies `initialData` by inserting it into the cache if no data is
    /// present yet, and stores `placeholderData` for use during loading states.
    pub fn from_query(client: &QueryClient, query: &Query<T>) -> Self {
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
            // Insert initial data into the cache so subsequent reads find it.
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
            _marker: PhantomData,
        }
    }

    /// Returns a reference to the current query state.
    pub fn state(&self) -> &QueryState<T> {
        &self.state
    }

    /// Transition to a loading state.
    ///
    /// If previous data exists, transitions to `Refetching(data)`.
    /// If placeholder data is configured, transitions to `LoadingWithPlaceholder(data)`.
    /// Otherwise transitions to `Loading`.
    pub fn set_loading(&mut self) {
        self.fetch_started_at = Some(Instant::now());
        if let Some(data) = self.state.data_cloned() {
            self.state = QueryState::Refetching(data);
        } else if let Some(placeholder) = &self.placeholder_data {
            let resolved = placeholder.resolve(None);
            self.state = QueryState::LoadingWithPlaceholder(resolved);
        } else {
            self.state = QueryState::Loading;
        }
    }

    /// Refresh the observer's state from the cache.
    pub fn update(&mut self) {
        let data = self.client.get_query_data::<T>(&self.key);
        let is_stale = self.client.is_stale(&self.key);

        self.state = match (data, is_stale) {
            (Some(d), true) => QueryState::Stale(d),
            (Some(d), false) => QueryState::Success(d),
            (None, _) => QueryState::Idle,
        };
    }

    /// Poll for pending broadcast updates without blocking.
    ///
    /// Call this from a GPUI update cycle to react to cache changes.
    pub fn try_recv(&mut self) {
        while let Ok(update) = self.receiver.try_recv() {
            match update.state_variant {
                QueryStateVariant::Success => {
                    if let Some(data) = self.client.get_query_data::<T>(&self.key) {
                        self.state = QueryState::Success(data);
                    }
                }
                QueryStateVariant::Stale => {
                    if let Some(data) = self.client.get_query_data::<T>(&self.key) {
                        self.state = QueryState::Stale(data);
                    }
                }
                QueryStateVariant::Loading | QueryStateVariant::Refetching => {
                    self.set_loading();
                }
                QueryStateVariant::Error => {
                    // Error state is set directly by the executor.
                }
                QueryStateVariant::Idle => {
                    self.state = QueryState::Idle;
                }
            }
        }
    }

    /// Mark the query as stale, triggering a notification to subscribers.
    ///
    /// The actual refetch must be initiated separately (e.g., via `spawn_query`).
    pub fn refetch(&self) {
        self.client.invalidate_queries(&self.key);
    }

    /// Returns the query key this observer is tracking.
    pub fn key(&self) -> &QueryKey {
        &self.key
    }

    /// Returns a reference to the underlying query client.
    pub fn client(&self) -> &QueryClient {
        &self.client
    }

    /// Returns the elapsed time since the current fetch started, if any.
    pub fn fetch_duration(&self) -> Option<std::time::Duration> {
        self.fetch_started_at.map(|t| t.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn test_new_observer_idle_when_no_cache() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let observer: QueryObserver<String> = QueryObserver::new(&client, key.clone());
        assert!(observer.state().is_idle());
    }

    #[test]
    fn test_new_observer_reads_cache() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "cached".to_string(), QueryOptions::default());
        let observer: QueryObserver<String> = QueryObserver::new(&client, key.clone());
        assert!(observer.state().is_success());
        assert_eq!(observer.state().data(), Some(&"cached".to_string()));
    }

    #[test]
    fn test_from_query_applies_initial_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<String> = Query::new(key.clone(), || async { Ok("fetched".to_string()) })
            .initial_data("initial".to_string());

        let observer = QueryObserver::from_query(&client, &query);
        assert_eq!(observer.state().data(), Some(&"initial".to_string()));
        // Verify it was actually inserted into the cache.
        let cached: Option<String> = client.get_query_data(&key);
        assert_eq!(cached, Some("initial".to_string()));
    }

    #[test]
    fn test_from_query_does_not_overwrite_existing_cache() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "existing".to_string(), QueryOptions::default());

        let query: Query<String> = Query::new(key.clone(), || async { Ok("fetched".to_string()) })
            .initial_data("initial".to_string());

        let observer = QueryObserver::from_query(&client, &query);
        assert_eq!(observer.state().data(), Some(&"existing".to_string()));
    }

    #[test]
    fn test_set_loading_with_placeholder() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<String> = Query::new(key.clone(), || async { Ok("fetched".to_string()) })
            .placeholder_data(PlaceholderData::value("ph".to_string()));

        let mut observer = QueryObserver::from_query(&client, &query);
        observer.set_loading();
        assert!(matches!(
            observer.state(),
            QueryState::LoadingWithPlaceholder(ref s) if s == "ph"
        ));
    }

    #[test]
    fn test_set_loading_refetching_when_has_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        let mut observer: QueryObserver<String> = QueryObserver::new(&client, key);
        observer.set_loading();
        assert!(matches!(
            observer.state(),
            QueryState::Refetching(ref s) if s == "data"
        ));
    }
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
        let mut opts = QueryOptions::default();
        opts.stale_time = Duration::from_millis(1);
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
        let mut opts = QueryOptions::default();
        opts.stale_time = Duration::from_millis(1);
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
        let mut opts = QueryOptions::default();
        opts.stale_time = Duration::from_millis(1);
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
        let v = QueryStateVariant::Success;
        let cloned = v.clone();
        assert_eq!(v, cloned);
    }

    #[test]
    fn test_query_state_variant_debug() {
        let v = QueryStateVariant::Error;
        let debug = format!("{:?}", v);
        assert!(debug.contains("Error"));
    }

    #[test]
    fn test_query_state_update_debug() {
        let update = QueryStateUpdate {
            key: "test".to_string(),
            state_variant: QueryStateVariant::Success,
        };
        let debug = format!("{:?}", update);
        assert!(debug.contains("test"));
    }
}
