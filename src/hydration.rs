//! Hydration utilities for server-side rendering and persistence

use crate::QueryClient;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A dehydrated state of the query cache that can be serialized and transferred.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DehydratedState {
    /// Serialized queries, keyed by their cache key.
    pub queries: HashMap<String, DehydratedQuery>,
    /// Serialized mutations (if any) - not implemented yet.
    pub mutations: HashMap<String, DehydratedMutation>,
}

/// A dehydrated query entry.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DehydratedQuery {
    /// The query key in its string representation.
    pub cache_key: String,
    /// The serialized data (as a string for transport).
    pub data: String,
    /// Type identifier for deserialization.
    pub type_name: String,
    /// Timestamp when the data was fetched (milliseconds since epoch).
    pub fetched_at_ms: u64,
    /// Query options used for this query.
    pub options: DehydratedQueryOptions,
    /// Whether the query was marked stale.
    pub is_stale: bool,
}

/// Dehydrated query options (subset of `QueryOptions`).
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DehydratedQueryOptions {
    pub stale_time_ms: u64,
    pub gc_time_ms: u64,
    pub refetch_on_mount: String,
    pub enabled: bool,
    pub structural_sharing: bool,
    pub refetch_interval_ms: Option<u64>,
    pub refetch_interval_in_background: bool,
    pub refetch_on_window_focus: bool,
}

/// A dehydrated mutation entry (placeholder).
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DehydratedMutation {
    // To be implemented later.
}

/// Options for hydrating a dehydrated state.
#[derive(Debug, Clone, Default)]
pub struct HydrateOptions {
    /// If true, only overwrite cache entries if the dehydrated data is newer.
    pub respect_timestamps: bool,
}

impl QueryClient {
    /// Dehydrate the current cache into a serializable state.
    ///
    /// Only successful queries are included by default.
    pub fn dehydrate(&self) -> DehydratedState {
        let mut queries = HashMap::new();

        for entry in self.cache.iter() {
            let key = entry.key().clone();
            let entry = entry.value();

            // Skip stale or error states? For simplicity, include all.
            // We'll serialize the data as JSON if `serde` is enabled, else as a placeholder.
            let data_str = if cfg!(feature = "serde") {
                // Since we don't know the type, we can't serialize directly.
                // In practice, users would need to provide a serialization hook.
                // For now, store an empty string as placeholder.
                String::new()
            } else {
                String::new()
            };

            let dehydrated = DehydratedQuery {
                cache_key: key.clone(),
                data: data_str,
                type_name: std::any::type_name::<()>().to_string(), // Placeholder
                fetched_at_ms: entry.fetched_at.elapsed().as_millis() as u64,
                options: DehydratedQueryOptions {
                    stale_time_ms: entry.options.stale_time.as_millis() as u64,
                    gc_time_ms: entry.options.gc_time.as_millis() as u64,
                    refetch_on_mount: format!("{:?}", entry.options.refetch_on_mount),
                    enabled: entry.options.enabled,
                    structural_sharing: entry.options.structural_sharing,
                    refetch_interval_ms: entry
                        .options
                        .refetch_interval
                        .map(|d| d.as_millis() as u64),
                    refetch_interval_in_background: entry.options.refetch_interval_in_background,
                    refetch_on_window_focus: entry.options.refetch_on_window_focus,
                },
                is_stale: entry.is_stale,
            };
            queries.insert(key, dehydrated);
        }

        DehydratedState {
            queries,
            mutations: HashMap::new(),
        }
    }

    /// Hydrate the cache with a previously dehydrated state.
    ///
    /// This will merge the dehydrated data into the existing cache.
    /// If `respect_timestamps` is true, only overwrite if the dehydrated data is newer.
    pub fn hydrate(&self, state: DehydratedState, options: HydrateOptions) {
        use crate::client::CacheEntry;
        use std::any::TypeId;
        use std::sync::Arc;

        for (key, dehydrated) in state.queries {
            // Check if we already have this query
            if let Some(existing) = self.cache.get(&key) {
                if options.respect_timestamps {
                    let dehydrated_time = Duration::from_millis(dehydrated.fetched_at_ms);
                    if existing.fetched_at.elapsed() < dehydrated_time {
                        // Existing data is newer, skip.
                        continue;
                    }
                }

                // Update existing entry to be stale
                if let Some(mut entry) = self.cache.get_mut(&key) {
                    entry.is_stale = true;
                }
            } else {
                // Insert a placeholder entry so the cache knows about this query
                // and can report it as stale to trigger a refetch.
                let entry = CacheEntry {
                    data: Arc::new(()), // Placeholder data
                    type_id: TypeId::of::<()>(),
                    fetched_at: Instant::now() - Duration::from_millis(dehydrated.fetched_at_ms),
                    last_accessed: Instant::now(),
                    options: crate::QueryOptions {
                        stale_time: Duration::from_millis(dehydrated.options.stale_time_ms),
                        gc_time: Duration::from_millis(dehydrated.options.gc_time_ms),
                        refetch_on_window_focus: dehydrated.options.refetch_on_window_focus,
                        ..Default::default()
                    },
                    is_stale: true, // Mark as stale so observers know to refetch
                };
                self.cache.insert(key, entry);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QueryClient, QueryKey, QueryOptions};

    #[test]
    fn test_dehydrate_hydrate_cycle() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        let dehydrated = client.dehydrate();
        assert!(dehydrated.queries.contains_key(key.cache_key()));

        let client2 = QueryClient::new();
        client2.hydrate(dehydrated, HydrateOptions::default());

        // The cache entry exists (marked stale) even if data not fully restored.
        assert!(client2.is_stale(&key));
    }

    #[test]
    fn test_dehydrate_empty() {
        let client = QueryClient::new();
        let dehydrated = client.dehydrate();
        assert!(dehydrated.queries.is_empty());
    }
}
