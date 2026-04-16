// src/observer.rs
//! Query observer for reactive state updates

use crate::{QueryClient, QueryError, QueryKey, QueryOptions, QueryState};
use std::marker::PhantomData;
use std::time::Instant;
use tokio::sync::broadcast;

/// Variant of a query state update, used for notifications.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryStateVariant {
    Idle,
    Loading,
    Refetching,
    Success,
    Stale,
    Error,
}

/// A state update notification sent to subscribers.
#[derive(Clone, Debug)]
pub struct QueryStateUpdate {
    pub key: String,
    pub state_variant: QueryStateVariant,
}

/// Reactive observer for a query key.
pub struct QueryObserver<T> {
    pub key: QueryKey,
    state: QueryState<T>,
    client: QueryClient,
    receiver: broadcast::Receiver<QueryStateUpdate>,
    options: QueryOptions,
    fetch_started_at: Option<Instant>,
    _marker: PhantomData<T>,
}

impl<T: Clone + Send + Sync + 'static> QueryObserver<T> {
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
            _marker: PhantomData,
        }
    }

    pub fn with_options(client: &QueryClient, key: QueryKey, options: QueryOptions) -> Self {
        let mut observer = Self::new(client, key);
        observer.options = options;
        observer
    }

    pub fn state(&self) -> &QueryState<T> {
        &self.state
    }

    pub fn client(&self) -> &QueryClient {
        &self.client
    }

    pub fn update(&mut self) {
        let data: Option<T> = self.client.get_query_data(&self.key);
        let is_stale = self.client.is_stale(&self.key);

        self.state = match (&self.state, data, is_stale) {
            (_, None, _) => QueryState::Idle,
            (QueryState::Loading, Some(data), _) => {
                if is_stale {
                    QueryState::Stale(data)
                } else {
                    QueryState::Success(data)
                }
            }
            (QueryState::Refetching(_), Some(data), _) => {
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

    pub fn set_loading(&mut self) {
        self.fetch_started_at = Some(Instant::now());
        let has_data = self.state.data().is_some();
        if has_data {
            let data = self.state.data_cloned().unwrap();
            self.state = QueryState::Refetching(data);
        } else {
            self.state = QueryState::Loading;
        }
    }

    pub fn set_success(&mut self, data: T) {
        self.fetch_started_at = None;
        self.state = QueryState::Success(data);
    }

    pub fn set_error(&mut self, error: QueryError) {
        self.fetch_started_at = None;
        let stale_data = self.state.data_cloned();
        self.state = QueryState::Error { error, stale_data };
    }

    pub fn set_stale(&mut self) {
        if let Some(data) = self.state.data_cloned() {
            self.state = QueryState::Stale(data);
        }
    }

    pub fn try_recv(&mut self) -> Option<QueryStateUpdate> {
        match self.receiver.try_recv() {
            Ok(update) => Some(update),
            Err(broadcast::error::TryRecvError::Empty) => None,
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
            Err(broadcast::error::TryRecvError::Closed) => None,
        }
    }

    pub fn refetch(&mut self) {
        self.client.invalidate_queries(&self.key);
    }

    pub fn options(&self) -> &QueryOptions {
        &self.options
    }

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
