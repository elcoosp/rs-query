//! QueryClient - central cache and query manager

use crate::focus_manager::FocusManager;
use crate::infinite::InfiniteData;
use crate::observer::{QueryStateUpdate, QueryStateVariant};
use crate::sharing::replace_equal_deep_any;
use crate::{QueryKey, QueryOptions};
use dashmap::DashMap;
use std::any::{Any, TypeId};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio::task::AbortHandle;

/// Cache entry for type-erased storage
#[derive(Clone)]
pub(crate) struct CacheEntry {
    pub(crate) data: Arc<dyn Any + Send + Sync>,
    pub(crate) type_id: TypeId,
    pub(crate) fetched_at: Instant,
    pub(crate) last_accessed: Instant,
    pub(crate) options: QueryOptions,
    pub(crate) is_stale: bool,
}

/// Central query client managing cache and query execution.
pub struct QueryClient {
    pub(crate) cache: Arc<DashMap<String, CacheEntry>>,
    /// Abort handles for in-flight queries, keyed by cache key.
    abort_handles: Arc<DashMap<String, AbortHandle>>,
    // Broadcast channels for state updates, one per key.
    subscribers: Arc<DashMap<String, broadcast::Sender<QueryStateUpdate>>>,
    /// Focus manager for window focus refetching
    pub focus_manager: FocusManager,
}

impl QueryClient {
    pub fn new() -> Self {
        let client = Self {
            cache: Arc::new(DashMap::new()),
            abort_handles: Arc::new(DashMap::new()),
            subscribers: Arc::new(DashMap::new()),
            focus_manager: FocusManager::new(),
        };
        // Spawn background garbage collection thread.
        let cache = Arc::clone(&client.cache);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(60));
            Self::gc_internal(&cache);
        });
        client
    }

    /// Subscribe to state changes for a given cache key.
    /// Returns a receiver that will receive updates whenever the query state changes.
    pub fn subscribe(&self, cache_key: &str) -> broadcast::Receiver<QueryStateUpdate> {
        // Get or create a sender for this key.
        let entry = self.subscribers.entry(cache_key.to_string());
        let sender = entry.or_insert_with(|| broadcast::channel(16).0);
        sender.subscribe()
    }

    /// Internal method to notify subscribers of a state change.
    pub fn notify_subscribers(&self, cache_key: &str, variant: QueryStateVariant) {
        if let Some(sender) = self.subscribers.get(cache_key) {
            let update = QueryStateUpdate {
                key: cache_key.to_string(),
                state_variant: variant,
            };
            let _ = sender.send(update);
        }
    }

    /// Get cached data if available and not stale
    pub fn get_query_data<T: Clone + Send + Sync + 'static>(&self, key: &QueryKey) -> Option<T> {
        let cache_key = key.cache_key();
        let entry = self.cache.get(cache_key)?;

        if entry.type_id != TypeId::of::<T>() {
            return None;
        }

        // NOTE: Stale data is intentionally returned here!
        // In React-Query, stale data is still shown to the user while a background refetch occurs.
        // Users should check `client.is_stale(&key)` if they specifically need to know freshness.

        entry.data.downcast_ref::<T>().cloned()
    }

    /// Get the options for a query key (if cached).
    pub fn get_query_options(&self, key: &QueryKey) -> Option<QueryOptions> {
        let cache_key = key.cache_key();
        self.cache.get(cache_key).map(|entry| entry.options.clone())
    }

    /// Get cached infinite data if available.
    pub fn get_infinite_data<T, P>(&self, key: &QueryKey) -> Option<InfiniteData<T, P>>
    where
        T: Clone + Send + Sync + 'static,
        P: Clone + Send + Sync + 'static,
    {
        self.get_query_data::<InfiniteData<T, P>>(key)
    }

    /// Check if the data for a key is stale.
    pub fn is_stale(&self, key: &QueryKey) -> bool {
        let cache_key = key.cache_key();
        if let Some(entry) = self.cache.get(cache_key) {
            let time_stale = !entry.options.stale_time.is_zero()
                && entry.fetched_at.elapsed() > entry.options.stale_time;
            time_stale || entry.is_stale
        } else {
            false
        }
    }

    /// Set cached data with optional structural sharing.
    pub fn set_query_data<T: Clone + Send + Sync + 'static>(
        &self,
        key: &QueryKey,
        data: T,
        options: QueryOptions,
    ) {
        let cache_key = key.cache_key().to_string();
        let new_data_arc = Arc::new(data);

        // Apply structural sharing if enabled and old data exists
        let final_data = if options.structural_sharing {
            if let Some(old_entry) = self.cache.get(&cache_key) {
                if old_entry.type_id == TypeId::of::<T>() {
                    replace_equal_deep_any(old_entry.data.clone(), new_data_arc.clone())
                } else {
                    new_data_arc
                }
            } else {
                new_data_arc
            }
        } else {
            new_data_arc
        };

        self.cache.insert(
            cache_key.clone(),
            CacheEntry {
                data: final_data,
                type_id: TypeId::of::<T>(),
                fetched_at: Instant::now(),
                last_accessed: Instant::now(),
                options,
                is_stale: false,
            },
        );
        self.notify_subscribers(&cache_key, QueryStateVariant::Success);
    }

    /// Set infinite query data.
    pub fn set_infinite_data<T, P>(
        &self,
        key: &QueryKey,
        data: InfiniteData<T, P>,
        options: QueryOptions,
    ) where
        T: Clone + Send + Sync + 'static,
        P: Clone + Send + Sync + 'static,
    {
        self.set_query_data(key, data, options);
    }

    /// Append a new page to existing infinite data.
    /// Returns the updated data if successful.
    pub fn append_infinite_page<T, P>(
        &self,
        key: &QueryKey,
        page: T,
        page_param: P,
        max_pages: Option<usize>,
    ) -> Option<InfiniteData<T, P>>
    where
        T: Clone + Send + Sync + 'static,
        P: Clone + Send + Sync + 'static,
    {
        let cache_key = key.cache_key().to_string();
        let mut entry = self.cache.get_mut(&cache_key)?;

        if let Some(data) = entry.data.downcast_ref::<InfiniteData<T, P>>() {
            let mut new_data = data.clone();
            new_data.pages.push(page);
            new_data.page_params.push(page_param);
            if let Some(max) = max_pages {
                if new_data.pages.len() > max {
                    new_data.pages.remove(0);
                    new_data.page_params.remove(0);
                }
            }
            entry.data = Arc::new(new_data.clone());
            entry.fetched_at = Instant::now();
            entry.last_accessed = Instant::now();
            self.notify_subscribers(&cache_key, QueryStateVariant::Success);
            return Some(new_data);
        }
        None
    }

    /// Prepend a new page to existing infinite data (for bi-directional).
    pub fn prepend_infinite_page<T, P>(
        &self,
        key: &QueryKey,
        page: T,
        page_param: P,
        max_pages: Option<usize>,
    ) -> Option<InfiniteData<T, P>>
    where
        T: Clone + Send + Sync + 'static,
        P: Clone + Send + Sync + 'static,
    {
        let cache_key = key.cache_key().to_string();
        let mut entry = self.cache.get_mut(&cache_key)?;

        if let Some(data) = entry.data.downcast_ref::<InfiniteData<T, P>>() {
            let mut new_data = data.clone();
            new_data.pages.insert(0, page);
            new_data.page_params.insert(0, page_param);
            if let Some(max) = max_pages {
                if new_data.pages.len() > max {
                    new_data.pages.pop();
                    new_data.page_params.pop();
                }
            }
            entry.data = Arc::new(new_data.clone());
            entry.fetched_at = Instant::now();
            entry.last_accessed = Instant::now();
            self.notify_subscribers(&cache_key, QueryStateVariant::Success);
            return Some(new_data);
        }
        None
    }

    /// Invalidate queries matching the key pattern.
    /// If `cancel_in_flight` is true, also abort any ongoing requests.
    pub fn invalidate_queries(&self, pattern: &QueryKey) {
        let pattern_str = pattern.cache_key();
        let keys_to_invalidate: Vec<String> = self
            .cache
            .iter()
            .filter_map(|entry| {
                let key = entry.key();
                if key.starts_with(pattern_str) || key == pattern_str {
                    Some(key.clone())
                } else {
                    None
                }
            })
            .collect();

        for key in keys_to_invalidate {
            // Cancel in-flight request if present
            if let Some((_, handle)) = self.abort_handles.remove(&key) {
                handle.abort();
            }
            if let Some(mut entry) = self.cache.get_mut(&key) {
                entry.is_stale = true;
                self.notify_subscribers(&key, QueryStateVariant::Stale);
            }
        }
    }

    /// Cancel a specific query if it's in flight.
    pub fn cancel_query(&self, key: &QueryKey) {
        let cache_key = key.cache_key();
        if let Some((_, handle)) = self.abort_handles.remove(cache_key) {
            handle.abort();
        }
    }

    /// Check if a query is in flight (for deduplication)
    pub fn is_in_flight(&self, key: &QueryKey) -> bool {
        self.abort_handles.contains_key(key.cache_key())
    }

    /// Store an abort handle for a query key.
    pub fn set_abort_handle(&self, key: &QueryKey, handle: AbortHandle) {
        let cache_key = key.cache_key().to_string();
        self.abort_handles.insert(cache_key, handle);
    }

    /// Remove the abort handle for a query key (e.g., when fetch completes).
    pub fn clear_abort_handle(&self, key: &QueryKey) {
        let cache_key = key.cache_key();
        self.abort_handles.remove(cache_key);
    }

    /// Clear all cached data
    pub fn clear(&self) {
        // Abort all in-flight requests
        for entry in self.abort_handles.iter() {
            entry.value().abort();
        }
        self.abort_handles.clear();
        self.cache.clear();
    }

    /// Run garbage collection on stale entries (public, but also called automatically)
    pub fn gc(&self) {
        Self::gc_internal(&self.cache);
    }

    fn gc_internal(cache: &DashMap<String, CacheEntry>) {
        cache.retain(|_, entry| {
            let age = entry.last_accessed.elapsed();
            age < entry.options.gc_time
        });
    }

    /// Trigger refetch of all active stale queries (e.g., on window focus).
    pub fn refetch_all_stale(&self) {
        let keys: Vec<String> = self.cache.iter().map(|entry| entry.key().clone()).collect();
        for key in keys {
            if let Some(entry) = self.cache.get(&key) {
                let time_stale = !entry.options.stale_time.is_zero()
                    && entry.fetched_at.elapsed() > entry.options.stale_time;
                let is_stale = time_stale || entry.is_stale;

                if is_stale && entry.options.refetch_on_window_focus {
                    drop(entry);
                    if let Some(mut entry_mut) = self.cache.get_mut(&key) {
                        entry_mut.is_stale = true;
                    }
                    self.notify_subscribers(&key, QueryStateVariant::Stale);
                }
            }
        }
    }
}

impl Default for QueryClient {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for QueryClient {
    fn clone(&self) -> Self {
        Self {
            cache: Arc::clone(&self.cache),
            abort_handles: Arc::clone(&self.abort_handles),
            subscribers: Arc::clone(&self.subscribers),
            focus_manager: self.focus_manager.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QueryKey, QueryOptions};
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_set_and_get_query_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let data = "hello".to_string();

        client.set_query_data(&key, data.clone(), QueryOptions::default());
        let retrieved: Option<String> = client.get_query_data(&key);
        assert_eq!(retrieved, Some(data));
    }

    #[tokio::test]
    async fn test_get_query_data_stale() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let data = "hello".to_string();
        let options = QueryOptions {
            stale_time: Duration::from_millis(10),
            ..Default::default()
        };

        client.set_query_data(&key, data.clone(), options);
        sleep(Duration::from_millis(20)).await;

        let retrieved: Option<String> = client.get_query_data(&key);

        // Stale data should STILL be returned to the UI
        assert_eq!(retrieved, Some(data));
        // But the client should correctly identify it as stale
        assert!(client.is_stale(&key));
    }
    #[tokio::test]
    async fn test_invalidate_queries() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("users").with("id", 1);
        let key2 = QueryKey::new("users").with("id", 2);
        let data = "user".to_string();

        client.set_query_data(&key1, data.clone(), QueryOptions::default());
        client.set_query_data(&key2, data.clone(), QueryOptions::default());

        // Invalidate all users
        client.invalidate_queries(&QueryKey::new("users"));

        // Both should be stale now
        assert!(client.is_stale(&key1));
        assert!(client.is_stale(&key2));
    }

    #[tokio::test]
    async fn test_invalidate_queries_exact_match() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("users").with("id", 1);
        let key2 = QueryKey::new("users").with("id", 2);
        let data = "user".to_string();

        client.set_query_data(&key1, data.clone(), QueryOptions::default());
        client.set_query_data(&key2, data.clone(), QueryOptions::default());

        // Invalidate only key1
        client.invalidate_queries(&key1);

        assert!(client.is_stale(&key1));
        assert!(!client.is_stale(&key2));
    }

    #[tokio::test]
    async fn test_subscribe_and_notify() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let cache_key = key.cache_key().to_string();

        let mut rx = client.subscribe(&cache_key);

        // Set data triggers notification
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        let update = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("should receive notification")
            .expect("should be ok");

        assert_eq!(update.key, cache_key);
        assert!(matches!(update.state_variant, QueryStateVariant::Success));
    }

    #[tokio::test]
    async fn test_garbage_collection() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            gc_time: Duration::from_millis(50),
            ..Default::default()
        };

        client.set_query_data(&key, "data".to_string(), options);
        assert!(client.get_query_data::<String>(&key).is_some());

        // Wait for GC to run (background thread runs every 60s, so manually call gc)
        sleep(Duration::from_millis(60)).await;
        client.gc();

        assert!(client.get_query_data::<String>(&key).is_none());
    }

    #[tokio::test]
    async fn test_refetch_all_stale() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            stale_time: Duration::from_millis(10),
            refetch_on_window_focus: true,
            ..Default::default()
        };

        client.set_query_data(&key, "data".to_string(), options);
        sleep(Duration::from_millis(20)).await;

        // Should be stale
        assert!(client.is_stale(&key));

        // Trigger refetch all stale (just ensures no panic)
        client.refetch_all_stale();
    }

    #[tokio::test]
    async fn test_is_in_flight_and_abort_handle() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        assert!(!client.is_in_flight(&key));

        // Simulate setting an abort handle
        let (tx, _rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(async {
            let _ = tx.send(());
        });
        let abort_handle = handle.abort_handle();
        client.set_abort_handle(&key, abort_handle);

        assert!(client.is_in_flight(&key));

        client.clear_abort_handle(&key);
        assert!(!client.is_in_flight(&key));
    }
}
