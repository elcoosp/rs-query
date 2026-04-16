//! Query observer for reactive state management

use crate::{QueryClient, QueryError, QueryKey, QueryOptions, QueryState};
use gpui::{Context, Model, ModelContext};
use std::fmt::Debug;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Observer that subscribes to a query's state changes.
pub struct QueryObserver<T: Clone + Send + Sync + 'static> {
    key: QueryKey,
    client: QueryClient,
    current_state: QueryState<T>,
    rx: broadcast::Receiver<QueryStateUpdate>,
}

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
            // Determine staleness
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
        // Drain all pending updates and compute latest state
        while let Ok(update) = self.rx.try_recv() {
            if update.key == self.key.cache_key() {
                // State changed, recompute
                self.current_state = Self::compute_initial_state(&self.client, &self.key);
            }
        }
    }

    pub fn refetch(&self) {
        // This will be implemented in executor integration
        // For now, placeholder
    }
}

/// GPUI helper: creates a Model<QueryObserver<T>> that automatically updates.
pub fn use_query<V: 'static, T: Clone + Send + Sync + Debug + 'static>(
    cx: &mut Context<V>,
    client: &QueryClient,
    query: &crate::Query<T>,
) -> Model<QueryObserver<T>> {
    let observer = QueryObserver::new(client, query.key.clone());
    let model = cx.new_model(|_| observer);

    // Spawn the query fetch if not already in flight and cache is stale/empty
    crate::spawn_query(cx, client, query, {
        let model = model.clone();
        move |_this, _state, cx| {
            model.update(cx, |observer, _cx| {
                observer.update();
            });
        }
    });

    model
}

impl<T: Clone + Send + Sync + Debug + 'static> gpui::Render for QueryObserver<T> {
    fn render(&mut self, _cx: &mut gpui::ViewContext<Self>) -> impl gpui::IntoElement {
        // This is a placeholder; actual rendering is handled by the component
        gpui::div()
    }
}
