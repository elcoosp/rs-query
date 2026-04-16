//! QueryClient - central cache and query manager

use crate::observer::{QueryStateUpdate, QueryStateVariant};
use crate::{QueryKey, QueryOptions};
use dashmap::DashMap;
use std::any::{Any, TypeId};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Cache entry for type-erased storage
struct CacheEntry {
    data: Box<dyn Any + Send + Sync>,
    type_id: TypeId,
    fetched_at: Instant,
    last_accessed: Instant,
    options: QueryOptions,
    is_stale: bool,
}

/// Central query client managing cache and query execution.
pub struct QueryClient {
    cache: Arc<DashMap<String, CacheEntry>>,
    in_flight: Arc<DashMap<String, ()>>,
    // Broadcast channels for state updates, one per key.
    subscribers: Arc<DashMap<String, broadcast::Sender<QueryStateUpdate>>>,
}

impl QueryClient {
    pub fn new() -> Self {
        let client = Self {
            cache: Arc::new(DashMap::new()),
            in_flight: Arc::new(DashMap::new()),
            subscribers: Arc::new(DashMap::new()),
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

        // Check if stale
        let age = entry.fetched_at.elapsed();
        if age > entry.options.stale_time && !entry.is_stale {
            return None;
        }

        entry.data.downcast_ref::<T>().cloned()
    }

    /// Check if the data for a key is stale.
    pub fn is_stale(&self, key: &QueryKey) -> bool {
        let cache_key = key.cache_key();
        if let Some(entry) = self.cache.get(cache_key) {
            let age = entry.fetched_at.elapsed();
            age > entry.options.stale_time || entry.is_stale
        } else {
            false
        }
    }

    /// Set cached data
    pub fn set_query_data<T: Clone + Send + Sync + 'static>(
        &self,
        key: &QueryKey,
        data: T,
        options: QueryOptions,
    ) {
        let cache_key = key.cache_key().to_string();
        self.cache.insert(
            cache_key.clone(),
            CacheEntry {
                data: Box::new(data),
                type_id: TypeId::of::<T>(),
                fetched_at: Instant::now(),
                last_accessed: Instant::now(),
                options,
                is_stale: false,
            },
        );
        self.notify_subscribers(&cache_key, QueryStateVariant::Success);
    }

    /// Invalidate queries matching the key pattern
    pub fn invalidate_queries(&self, pattern: &QueryKey) {
        let pattern_str = pattern.cache_key();
        // DashMap iteration is lock‑free but we must collect keys to modify.
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
            if let Some(mut entry) = self.cache.get_mut(&key) {
                entry.is_stale = true;
                self.notify_subscribers(&key, QueryStateVariant::Stale);
            }
        }
    }

    /// Check if a query is in flight (for deduplication)
    pub fn is_in_flight(&self, key: &QueryKey) -> bool {
        self.in_flight.contains_key(key.cache_key())
    }

    /// Mark query as in flight
    pub fn set_in_flight(&self, key: &QueryKey, in_flight: bool) {
        let cache_key = key.cache_key().to_string();
        if in_flight {
            self.in_flight.insert(cache_key, ());
        } else {
            self.in_flight.remove(&cache_key);
        }
    }

    /// Clear all cached data
    pub fn clear(&self) {
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
        }
    }
}
