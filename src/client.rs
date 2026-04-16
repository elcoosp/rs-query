//! QueryClient - central cache and query manager

use crate::{QueryKey, QueryOptions};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Instant;

/// Cache entry for type-erased storage
struct CacheEntry {
    data: Box<dyn Any + Send + Sync>,
    type_id: TypeId,
    fetched_at: Instant,
    #[allow(dead_code)]
    last_accessed: Instant,
    options: QueryOptions,
    is_stale: bool,
}

/// Central query client managing cache and query execution.
///
/// Create one per application and share with all components.
///
/// # Example
///
/// ```rust,ignore
/// struct App {
///     query_client: QueryClient,
/// }
///
/// impl App {
///     fn new() -> Self {
///         Self {
///             query_client: QueryClient::new(),
///         }
///     }
/// }
/// ```
pub struct QueryClient {
    cache: Arc<RwLock<HashMap<String, CacheEntry>>>,
    in_flight: Arc<RwLock<HashMap<String, ()>>>,
}

impl QueryClient {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            in_flight: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get cached data if available and not stale
    pub fn get_query_data<T: Clone + Send + Sync + 'static>(&self, key: &QueryKey) -> Option<T> {
        let cache = self.cache.read().ok()?;
        let entry = cache.get(&key.cache_key())?;

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

    /// Set cached data
    pub fn set_query_data<T: Clone + Send + Sync + 'static>(
        &self,
        key: &QueryKey,
        data: T,
        options: QueryOptions,
    ) {
        let mut cache = self.cache.write().unwrap();
        cache.insert(
            key.cache_key(),
            CacheEntry {
                data: Box::new(data),
                type_id: TypeId::of::<T>(),
                fetched_at: Instant::now(),
                last_accessed: Instant::now(),
                options,
                is_stale: false,
            },
        );
    }

    /// Invalidate queries matching the key pattern
    pub fn invalidate_queries(&self, pattern: &QueryKey) {
        let mut cache = self.cache.write().unwrap();
        let pattern_str = pattern.cache_key();

        for (key, entry) in cache.iter_mut() {
            if key.starts_with(&pattern_str) || key == &pattern_str {
                entry.is_stale = true;
            }
        }
    }

    /// Check if a query is in flight (for deduplication)
    pub fn is_in_flight(&self, key: &QueryKey) -> bool {
        self.in_flight
            .read()
            .unwrap()
            .contains_key(&key.cache_key())
    }

    /// Mark query as in flight
    pub fn set_in_flight(&self, key: &QueryKey, in_flight: bool) {
        let mut map = self.in_flight.write().unwrap();
        if in_flight {
            map.insert(key.cache_key(), ());
        } else {
            map.remove(&key.cache_key());
        }
    }

    /// Clear all cached data
    pub fn clear(&self) {
        self.cache.write().unwrap().clear();
    }

    /// Run garbage collection on stale entries
    #[allow(dead_code)]
    pub fn gc(&self) {
        let mut cache = self.cache.write().unwrap();
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
        }
    }
}
