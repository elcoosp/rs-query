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
        let options = client
            .get_query_options(&key)
            .unwrap_or_default();

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
