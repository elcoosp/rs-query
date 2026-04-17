// src/client.rs
//! QueryClient - central cache and query manager

use crate::focus_manager::FocusManager;
use crate::infinite::InfiniteData;
use crate::observer::{QueryStateUpdate, QueryStateVariant};
use crate::sharing::PartialEqAny;
use crate::{QueryKey, QueryOptions};
use dashmap::DashMap;
use std::any::TypeId;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

/// Cache entry for type-erased storage
#[derive(Clone)]
pub(crate) struct CacheEntry {
    pub(crate) data: Arc<dyn PartialEqAny + Send + Sync>,
    pub(crate) type_id: TypeId,
    pub(crate) fetched_at: Instant,
    pub(crate) last_accessed: Instant,
    pub(crate) options: QueryOptions,
    pub(crate) is_stale: bool,
}

/// An in-flight task with cancellation support.
pub(crate) struct InFlightTask {
    pub abort_handle: AbortHandle,
    pub cancel_token: CancellationToken,
}

/// Activity events for global fetching/mutating state.
#[derive(Clone, Debug)]
pub enum ActivityEvent {
    FetchingCountChanged(usize),
    MutatingCountChanged(usize),
}

/// Central query client managing cache and query execution.
pub struct QueryClient {
    pub(crate) cache: Arc<DashMap<String, CacheEntry>>,
    /// In-flight tasks keyed by cache key.
    in_flight: Arc<DashMap<String, InFlightTask>>,
    // Broadcast channels for state updates, one per key.
    subscribers: Arc<DashMap<String, broadcast::Sender<QueryStateUpdate>>>,
    /// Focus manager for window focus refetching
    pub focus_manager: FocusManager,
    // Activity counters
    fetching_count: Arc<AtomicUsize>,
    mutating_count: Arc<AtomicUsize>,
    activity_tx: broadcast::Sender<ActivityEvent>,
}

impl QueryClient {
    pub fn new() -> Self {
        let (activity_tx, _) = broadcast::channel(16);
        let client = Self {
            cache: Arc::new(DashMap::new()),
            in_flight: Arc::new(DashMap::new()),
            subscribers: Arc::new(DashMap::new()),
            focus_manager: FocusManager::new(),
            fetching_count: Arc::new(AtomicUsize::new(0)),
            mutating_count: Arc::new(AtomicUsize::new(0)),
            activity_tx,
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
    pub fn subscribe(&self, cache_key: &str) -> broadcast::Receiver<QueryStateUpdate> {
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

    /// Get cached data if available and not stale.
    pub fn get_query_data<T: Clone + Send + Sync + 'static>(&self, key: &QueryKey) -> Option<T> {
        let cache_key = key.cache_key();
        let entry = self.cache.get(cache_key)?;

        if entry.type_id != TypeId::of::<T>() {
            return None;
        }

        entry.data.as_any().downcast_ref::<T>().cloned()
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
    pub fn set_query_data<T: Clone + Send + Sync + 'static + PartialEq>(
        &self,
        key: &QueryKey,
        data: T,
        options: QueryOptions,
    ) {
        let cache_key = key.cache_key().to_string();
        let mut final_data: Arc<dyn PartialEqAny + Send + Sync> = Arc::new(data);

        if options.structural_sharing {
            if let Some(old_entry) = self.cache.get(&cache_key) {
                if old_entry.type_id == TypeId::of::<T>() {
                    if final_data.eq_any(old_entry.data.as_any()) {
                        final_data = old_entry.data.clone();
                    }
                }
            }
        }

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
        T: Clone + Send + Sync + 'static + PartialEq,
        P: Clone + Send + Sync + 'static + PartialEq,
    {
        self.set_query_data(key, data, options);
    }

    /// Append a new page to existing infinite data.
    pub fn append_infinite_page<T, P>(
        &self,
        key: &QueryKey,
        page: T,
        page_param: P,
        max_pages: Option<usize>,
    ) -> Option<InfiniteData<T, P>>
    where
        T: Clone + Send + Sync + 'static + PartialEq,
        P: Clone + Send + Sync + 'static + PartialEq,
    {
        let cache_key = key.cache_key().to_string();
        let mut entry = self.cache.get_mut(&cache_key)?;

        if let Some(data) = entry.data.as_any().downcast_ref::<InfiniteData<T, P>>() {
            let mut new_data = data.clone();
            new_data.push_page(page, page_param);
            if let Some(max) = max_pages {
                while new_data.pages.len() > max {
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
        T: Clone + Send + Sync + 'static + PartialEq,
        P: Clone + Send + Sync + 'static + PartialEq,
    {
        let cache_key = key.cache_key().to_string();
        let mut entry = self.cache.get_mut(&cache_key)?;

        if let Some(data) = entry.data.as_any().downcast_ref::<InfiniteData<T, P>>() {
            let mut new_data = data.clone();
            new_data.pages.insert(0, page);
            new_data.page_params.insert(0, page_param);
            if let Some(max) = max_pages {
                while new_data.pages.len() > max {
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
            if let Some((_, task)) = self.in_flight.remove(&key) {
                task.cancel_token.cancel();
                task.abort_handle.abort();
            }
            if let Some(mut entry_mut) = self.cache.get_mut(&key) {
                entry_mut.is_stale = true;
                self.notify_subscribers(&key, QueryStateVariant::Stale);
            }
        }
    }

    /// Cancel a specific query if it's in flight.
    pub fn cancel_query(&self, key: &QueryKey) {
        let cache_key = key.cache_key();
        if let Some((_, task)) = self.in_flight.remove(cache_key) {
            task.cancel_token.cancel();
            task.abort_handle.abort();
            // Wake up any waiting subscribers so they don't hang.
            self.notify_subscribers(cache_key, QueryStateVariant::Error);
        }
    }
    /// Check if a query is in flight (for deduplication)
    pub fn is_in_flight(&self, key: &QueryKey) -> bool {
        self.in_flight.contains_key(key.cache_key())
    }

    /// Store an in-flight task for a query key.
    pub(crate) fn set_in_flight(&self, key: &QueryKey, task: InFlightTask) {
        let cache_key = key.cache_key().to_string();
        self.in_flight.insert(cache_key, task);
    }

    /// Remove the in-flight task for a query key.
    pub(crate) fn clear_in_flight(&self, key: &QueryKey) {
        let cache_key = key.cache_key();
        self.in_flight.remove(cache_key);
    }

    /// Clear all cached data
    pub fn clear(&self) {
        for entry in self.in_flight.iter() {
            entry.value().cancel_token.cancel();
            entry.value().abort_handle.abort();
        }
        self.in_flight.clear();
        self.cache.clear();
    }

    /// Run garbage collection on stale entries.
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

    // ----- Activity tracking -----

    /// Returns true if any query is currently fetching.
    pub fn is_fetching(&self) -> bool {
        self.fetching_count.load(Ordering::Relaxed) > 0
    }

    /// Returns the number of in-flight fetch operations.
    pub fn fetching_count(&self) -> usize {
        self.fetching_count.load(Ordering::Relaxed)
    }

    /// Returns true if any mutation is in progress.
    pub fn is_mutating(&self) -> bool {
        self.mutating_count.load(Ordering::Relaxed) > 0
    }

    /// Returns the number of in-flight mutations.
    pub fn mutating_count(&self) -> usize {
        self.mutating_count.load(Ordering::Relaxed)
    }

    /// Subscribe to activity events (fetching/mutating count changes).
    pub fn subscribe_activity(&self) -> broadcast::Receiver<ActivityEvent> {
        self.activity_tx.subscribe()
    }

    pub(crate) fn inc_fetching(&self) {
        let new = self.fetching_count.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self
            .activity_tx
            .send(ActivityEvent::FetchingCountChanged(new));
    }

    pub(crate) fn dec_fetching(&self) {
        let new = self.fetching_count.fetch_sub(1, Ordering::Relaxed) - 1;
        let _ = self
            .activity_tx
            .send(ActivityEvent::FetchingCountChanged(new));
    }

    pub(crate) fn inc_mutating(&self) {
        let new = self.mutating_count.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self
            .activity_tx
            .send(ActivityEvent::MutatingCountChanged(new));
    }

    pub(crate) fn dec_mutating(&self) {
        let new = self.mutating_count.fetch_sub(1, Ordering::Relaxed) - 1;
        let _ = self
            .activity_tx
            .send(ActivityEvent::MutatingCountChanged(new));
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
            in_flight: Arc::clone(&self.in_flight),
            subscribers: Arc::clone(&self.subscribers),
            focus_manager: self.focus_manager.clone(),
            fetching_count: Arc::clone(&self.fetching_count),
            mutating_count: Arc::clone(&self.mutating_count),
            activity_tx: self.activity_tx.clone(),
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

        // Simulate setting an in-flight task
        let handle = tokio::spawn(async {});
        let token = CancellationToken::new();
        client.set_in_flight(
            &key,
            InFlightTask {
                abort_handle: handle.abort_handle(),
                cancel_token: token,
            },
        );

        assert!(client.is_in_flight(&key));

        client.clear_in_flight(&key);
        assert!(!client.is_in_flight(&key));
    }

    // --- TESTS FOR 100% COVERAGE ---

    #[test]
    fn test_get_query_options() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            stale_time: Duration::from_secs(60),
            gc_time: Duration::from_secs(300),
            ..Default::default()
        };

        assert!(client.get_query_options(&key).is_none());

        client.set_query_data(&key, "data".to_string(), options.clone());
        let retrieved = client.get_query_options(&key).unwrap();
        assert_eq!(retrieved.stale_time, Duration::from_secs(60));
        assert_eq!(retrieved.gc_time, Duration::from_secs(300));
    }

    #[test]
    fn test_get_infinite_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        assert!(client.get_infinite_data::<Vec<i32>, i32>(&key).is_none());

        let data = InfiniteData::with_first_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data.clone(), QueryOptions::default());

        let retrieved = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(retrieved.page_count(), 1);
        assert_eq!(retrieved.pages[0], vec![1, 2, 3]);
    }

    #[test]
    fn test_set_infinite_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let data = InfiniteData::with_first_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let retrieved: Option<InfiniteData<Vec<i32>, i32>> = client.get_query_data(&key);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().pages[0], vec![1, 2, 3]);
    }

    #[test]
    fn test_is_stale_no_cache() {
        let client = QueryClient::new();
        let key = QueryKey::new("nonexistent");
        assert!(!client.is_stale(&key));
    }

    #[test]
    fn test_is_stale_not_stale() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());
        assert!(!client.is_stale(&key));
    }

    #[test]
    fn test_is_stale_flag_stale() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());
        client.invalidate_queries(&key);
        assert!(client.is_stale(&key));
    }

    #[test]
    fn test_is_stale_zero_stale_time() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        // stale_time of ZERO means data is ALWAYS fresh from a time perspective
        let options = QueryOptions {
            stale_time: Duration::ZERO,
            ..Default::default()
        };
        client.set_query_data(&key, "data".to_string(), options);
        std::thread::sleep(Duration::from_millis(10));
        assert!(!client.is_stale(&key));
    }

    #[test]
    fn test_get_query_data_wrong_type() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "string_data".to_string(), QueryOptions::default());

        // Try to get as wrong type
        let retrieved: Option<i32> = client.get_query_data(&key);
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_get_query_data_no_cache() {
        let client = QueryClient::new();
        let key = QueryKey::new("nonexistent");
        let retrieved: Option<String> = client.get_query_data(&key);
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_set_query_data_structural_sharing_disabled() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            structural_sharing: false,
            ..Default::default()
        };

        client.set_query_data(&key, "first".to_string(), options.clone());
        client.set_query_data(&key, "second".to_string(), options);

        let retrieved: Option<String> = client.get_query_data(&key);
        assert_eq!(retrieved, Some("second".to_string()));
    }

    #[test]
    fn test_set_query_data_structural_sharing_enabled_no_old_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            structural_sharing: true,
            ..Default::default()
        };

        // No old data exists, should still work
        client.set_query_data(&key, "data".to_string(), options);
        let retrieved: Option<String> = client.get_query_data(&key);
        assert_eq!(retrieved, Some("data".to_string()));
    }

    #[test]
    fn test_set_query_data_structural_sharing_wrong_type() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        // Store as String
        client.set_query_data(&key, "string".to_string(), QueryOptions::default());

        // Overwrite as i32 with structural sharing enabled
        // The old entry is String, new is i32 - type mismatch branch
        let options = QueryOptions {
            structural_sharing: true,
            ..Default::default()
        };
        client.set_query_data(&key, 42i32, options);

        let retrieved: Option<i32> = client.get_query_data(&key);
        assert_eq!(retrieved, Some(42));
    }

    #[test]
    fn test_append_infinite_page() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let data = InfiniteData::with_first_page(vec![1, 2], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let result = client.append_infinite_page(&key, vec![3, 4], 1, None);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![1, 2]);
        assert_eq!(result.pages[1], vec![3, 4]);
        assert_eq!(result.page_params, vec![0, 1]);
    }

    #[test]
    fn test_append_infinite_page_with_max_pages_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let data = InfiniteData::with_first_page(vec![1], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        // Max 2 pages, add second
        client.append_infinite_page(&key, vec![2], 1, Some(2));

        // Add third - should evict first
        let result = client.append_infinite_page(&key, vec![3], 2, Some(2));
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![2]);
        assert_eq!(result.pages[1], vec![3]);
    }

    #[test]
    fn test_append_infinite_page_with_max_pages_no_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let data = InfiniteData::with_first_page(vec![1], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        // Max 3 pages, only adding 1 more = 2 total, no eviction
        let result = client.append_infinite_page(&key, vec![2], 1, Some(3));
        assert!(result.is_some());
        assert_eq!(result.unwrap().page_count(), 2);
    }

    #[test]
    fn test_append_infinite_page_no_existing_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let result = client.append_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_append_infinite_page_wrong_type() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "wrong_type".to_string(), QueryOptions::default());

        let result = client.append_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_prepend_infinite_page() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let data = InfiniteData::with_first_page(vec![3, 4], 1);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let result = client.prepend_infinite_page(&key, vec![1, 2], 0, None);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![1, 2]);
        assert_eq!(result.pages[1], vec![3, 4]);
        assert_eq!(result.page_params, vec![0, 1]);
    }

    #[test]
    fn test_prepend_infinite_page_with_max_pages_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let data = InfiniteData::with_first_page(vec![3], 2);
        client.set_infinite_data(&key, data, QueryOptions::default());

        client.prepend_infinite_page(&key, vec![2], 1, Some(2));
        let result = client.prepend_infinite_page(&key, vec![1], 0, Some(2));

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![1]);
        assert_eq!(result.pages[1], vec![2]);
        assert_eq!(result.page_params, vec![0, 1]);
    }

    #[test]
    fn test_prepend_infinite_page_with_max_pages_no_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let data = InfiniteData::with_first_page(vec![2], 1);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let result = client.prepend_infinite_page(&key, vec![1], 0, Some(3));
        assert!(result.is_some());
        assert_eq!(result.unwrap().page_count(), 2);
    }

    #[test]
    fn test_prepend_infinite_page_no_existing_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let result = client.prepend_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_prepend_infinite_page_wrong_type() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "wrong_type".to_string(), QueryOptions::default());

        let result = client.prepend_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_invalidate_queries_with_abort_handle() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        // Set an in-flight task
        let handle = tokio::spawn(async {});
        let token = CancellationToken::new();
        client.set_in_flight(
            &key,
            InFlightTask {
                abort_handle: handle.abort_handle(),
                cancel_token: token,
            },
        );

        assert!(client.is_in_flight(&key));

        // Invalidate should cancel in-flight
        client.invalidate_queries(&key);

        assert!(!client.is_in_flight(&key));
        assert!(client.is_stale(&key));
    }

    #[tokio::test]
    async fn test_cancel_query_with_handle() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        // Create a handle via tokio::spawn
        let handle = tokio::spawn(async {
            // Long running task
            tokio::time::sleep(std::time::Duration::from_secs(100)).await;
        });
        let token = CancellationToken::new();
        client.set_in_flight(
            &key,
            InFlightTask {
                abort_handle: handle.abort_handle(),
                cancel_token: token,
            },
        );

        // Should be in flight
        assert!(client.is_in_flight(&key));

        // Cancel should work
        client.cancel_query(&key);

        // Handle should be cleared
        assert!(!client.is_in_flight(&key));
    }

    #[test]
    fn test_cancel_query_no_handle() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        // Should not panic when no handle exists
        client.cancel_query(&key);
    }

    #[tokio::test]
    async fn test_clear_with_abort_handles() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("test1");
        let key2 = QueryKey::new("test2");

        // Create handles
        let handle1 = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(100)).await;
        });
        let handle2 = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(100)).await;
        });

        let token1 = CancellationToken::new();
        let token2 = CancellationToken::new();
        client.set_in_flight(
            &key1,
            InFlightTask {
                abort_handle: handle1.abort_handle(),
                cancel_token: token1,
            },
        );
        client.set_in_flight(
            &key2,
            InFlightTask {
                abort_handle: handle2.abort_handle(),
                cancel_token: token2,
            },
        );

        // Both should be in flight
        assert!(client.is_in_flight(&key1));
        assert!(client.is_in_flight(&key2));

        // Clear should abort all
        client.clear();

        // Both handles should be cleared
        assert!(!client.is_in_flight(&key1));
        assert!(!client.is_in_flight(&key2));
    }

    #[test]
    fn test_clear_empty() {
        let client = QueryClient::new();
        // Should not panic on empty cache
        client.clear();
        assert!(client.cache.is_empty());
    }

    #[test]
    fn test_refetch_all_stale_not_stale() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            stale_time: Duration::from_secs(60),
            refetch_on_window_focus: true,
            ..Default::default()
        };

        client.set_query_data(&key, "data".to_string(), options);

        // Data is fresh, refetch_all_stale should not notify
        client.refetch_all_stale();
        assert!(!client.is_stale(&key));
    }

    #[test]
    fn test_refetch_all_stale_refetch_disabled() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            stale_time: Duration::from_millis(1),
            refetch_on_window_focus: false,
            ..Default::default()
        };

        client.set_query_data(&key, "data".to_string(), options);
        std::thread::sleep(Duration::from_millis(5));

        assert!(client.is_stale(&key));

        // refetch_on_window_focus is false, so refetch_all_stale should not notify
        client.refetch_all_stale();
        // The data should still be stale (not re-fetched)
        assert!(client.is_stale(&key));
    }

    #[test]
    fn test_refetch_all_stale_empty_cache() {
        let client = QueryClient::new();
        // Should not panic on empty cache
        client.refetch_all_stale();
    }

    #[test]
    fn test_refetch_all_stale_flag_stale_with_refetch_enabled() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            stale_time: Duration::ZERO, // time-based not stale
            refetch_on_window_focus: true,
            ..Default::default()
        };

        client.set_query_data(&key, "data".to_string(), options);
        // Manually mark as stale via invalidation
        client.invalidate_queries(&key);

        // Now refetch_all_stale should pick it up (flag is stale, refetch enabled)
        client.refetch_all_stale();
        assert!(client.is_stale(&key));
    }

    #[test]
    fn test_subscribe_multiple_subscribers() {
        let client = QueryClient::new();
        let cache_key = "test".to_string();

        let mut rx1 = client.subscribe(&cache_key);
        let mut rx2 = client.subscribe(&cache_key);

        client.notify_subscribers(&cache_key, QueryStateVariant::Success);

        let u1 = rx1.try_recv().unwrap();
        let u2 = rx2.try_recv().unwrap();
        assert_eq!(u1.key, cache_key);
        assert_eq!(u2.key, cache_key);
    }

    #[test]
    fn test_notify_subscribers_no_subscribers() {
        let client = QueryClient::new();
        // Should not panic when no subscribers exist
        client.notify_subscribers("nonexistent", QueryStateVariant::Success);
    }

    #[test]
    fn test_client_default() {
        let client = QueryClient::default();
        assert!(client.cache.is_empty());
    }

    #[test]
    fn test_client_clone() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        let cloned = client.clone();
        let data: Option<String> = cloned.get_query_data(&key);
        assert_eq!(data, Some("data".to_string()));
    }

    #[test]
    fn test_gc_retains_fresh_entries() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            gc_time: Duration::from_secs(300),
            ..Default::default()
        };

        client.set_query_data(&key, "data".to_string(), options);
        client.gc();
        let data: Option<String> = client.get_query_data(&key);
        assert!(data.is_some());
    }

    #[test]
    fn test_invalidate_queries_no_matching_keys() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        // Pattern doesn't match
        client.invalidate_queries(&QueryKey::new("posts"));
        assert!(!client.is_stale(&key));
    }

    #[test]
    fn test_invalidate_queries_prefix_match() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("users").with("id", 1);
        let key2 = QueryKey::new("users").with("id", 2);
        let key3 = QueryKey::new("posts").with("id", 1);

        client.set_query_data(&key1, "u1".to_string(), QueryOptions::default());
        client.set_query_data(&key2, "u2".to_string(), QueryOptions::default());
        client.set_query_data(&key3, "p1".to_string(), QueryOptions::default());

        client.invalidate_queries(&QueryKey::new("users"));

        assert!(client.is_stale(&key1));
        assert!(client.is_stale(&key2));
        assert!(!client.is_stale(&key3));
    }

    #[tokio::test]
    async fn test_subscribe_creates_sender_lazily() {
        let client = QueryClient::new();
        let cache_key = "new_key".to_string();

        // This should create a new sender
        let mut rx = client.subscribe(&cache_key);
        client.notify_subscribers(&cache_key, QueryStateVariant::Loading);

        let update = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(update.state_variant, QueryStateVariant::Loading));
    }

    #[test]
    fn test_set_query_data_resets_stale_flag() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        client.set_query_data(&key, "data".to_string(), QueryOptions::default());
        client.invalidate_queries(&key);
        assert!(client.is_stale(&key));

        // Setting new data should reset stale flag
        client.set_query_data(&key, "fresh".to_string(), QueryOptions::default());
        assert!(!client.is_stale(&key));
    }
}
