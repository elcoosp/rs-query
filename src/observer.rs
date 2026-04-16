//! Query observer for reactive state management

use crate::{QueryClient, QueryKey, QueryState};
use std::fmt::Debug;
use tokio::sync::broadcast;

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
}

impl<T: Clone + Send + Sync + Debug + 'static> QueryObserver<T> {
    pub fn new(client: &QueryClient, key: QueryKey) -> Self {
        let cache_key = key.cache_key().to_string();
        let rx = client.subscribe(&cache_key);
        let current_state = Self::compute_initial_state(client, &key);
        Self {
            key,
            client: client.clone(),
            current_state,
            rx,
        }
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

    pub fn state(&self) -> &QueryState<T> {
        &self.current_state
    }

    pub fn update(&mut self) {
        // Drain all pending updates and recompute latest state
        while let Ok(update) = self.rx.try_recv() {
            if update.key == self.key.cache_key() {
                self.current_state = Self::compute_initial_state(&self.client, &self.key);
            }
        }
    }

    pub fn refetch(&self) {
        // Will be implemented in executor integration
    }
}
