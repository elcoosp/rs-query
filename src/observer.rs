//! Query observer for reactive state management

use crate::{QueryClient, QueryKey, QueryOptions, QueryState};
use std::fmt::Debug;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time;

/// Update message sent to observers when a query's state changes.
#[derive(Debug, Clone)]
pub struct QueryStateUpdate {
    pub key: String,
    pub state_variant: QueryStateVariant,
}

#[derive(Debug, Clone)]
pub enum QueryStateVariant {
    Idle,
    Loading,
    Refetching,
    Success,
    Stale,
    Error,
}

/// Observer that subscribes to a query's state changes.
/// Call `update()` periodically (e.g., in a render loop) to refresh the state.
pub struct QueryObserver<T: Clone + Send + Sync + 'static> {
    key: QueryKey,
    client: QueryClient,
    current_state: QueryState<T>,
    rx: broadcast::Receiver<QueryStateUpdate>,
    interval_handle: Option<JoinHandle<()>>,
}

impl<T: Clone + Send + Sync + Debug + 'static> QueryObserver<T> {
    pub fn new(client: &QueryClient, key: QueryKey) -> Self {
        let cache_key = key.cache_key().to_string();
        let rx = client.subscribe(&cache_key);
        let current_state = Self::compute_initial_state(client, &key);
        let options = client.get_query_options(&key).unwrap_or_default();

        let mut observer = Self {
            key,
            client: client.clone(),
            current_state,
            rx,
            interval_handle: None,
        };

        observer.maybe_start_interval(options);
        observer
    }

    fn compute_initial_state(client: &QueryClient, key: &QueryKey) -> QueryState<T> {
        if let Some(data) = client.get_query_data::<T>(key) {
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

    fn maybe_start_interval(&mut self, options: QueryOptions) {
        if let Some(interval) = options.refetch_interval {
            let client = self.client.clone();
            let key = self.key.clone();
            let in_background = options.refetch_interval_in_background;
            let handle = tokio::spawn(async move {
                let mut ticker = time::interval(interval);
                loop {
                    ticker.tick().await;
                    if !in_background && !client.focus_manager.is_focused() {
                        continue;
                    }
                    // Trigger a refetch by marking as stale and notifying
                    client.invalidate_queries(&key);
                }
            });
            self.interval_handle = Some(handle);
        }
    }

    pub fn state(&self) -> &QueryState<T> {
        &self.current_state
    }

    pub fn update(&mut self) {
        while let Ok(update) = self.rx.try_recv() {
            if update.key == self.key.cache_key() {
                self.current_state = Self::compute_initial_state(&self.client, &self.key);
            }
        }
    }

    pub fn refetch(&self) {
        self.client.invalidate_queries(&self.key);
    }
}

impl<T: Clone + Send + Sync + 'static> Drop for QueryObserver<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.interval_handle.take() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QueryClient, QueryKey, QueryOptions};
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_observer_initial_state() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let observer = QueryObserver::<String>::new(&client, key.clone());
        assert!(matches!(observer.state(), QueryState::Idle));

        // Set data and create new observer
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());
        let observer2 = QueryObserver::<String>::new(&client, key);
        assert!(matches!(observer2.state(), QueryState::Success(_)));
    }

    #[tokio::test]
    async fn test_observer_updates_on_notification() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let mut observer = QueryObserver::<String>::new(&client, key.clone());
        assert!(matches!(observer.state(), QueryState::Idle));

        // Set data triggers notification
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        // Wait a tiny bit for notification to propagate
        sleep(Duration::from_millis(10)).await;

        observer.update();
        assert!(matches!(observer.state(), QueryState::Success(ref s) if s == "data"));
    }

    #[tokio::test]
    async fn test_observer_refetch() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            stale_time: Duration::from_millis(10),
            ..Default::default()
        };

        client.set_query_data(&key, "old".to_string(), options);
        sleep(Duration::from_millis(20)).await;

        let observer = QueryObserver::<String>::new(&client, key.clone());
        // Should be stale
        assert!(matches!(observer.state(), QueryState::Stale(_)));

        observer.refetch();
        assert!(client.is_stale(&key));
    }

    #[tokio::test]
    async fn test_observer_interval_refetch() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            refetch_interval: Some(Duration::from_millis(50)),
            refetch_interval_in_background: true,
            ..Default::default()
        };

        client.set_query_data(&key, "data".to_string(), options.clone());

        let observer = QueryObserver::<String>::new(&client, key.clone());

        // Wait for interval to tick
        sleep(Duration::from_millis(120)).await;

        // Observer still alive
        assert!(client.get_query_data::<String>(&key).is_some());
        drop(observer);
    }
}
