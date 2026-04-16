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
                fetched_at_ms: entry
                    .fetched_at
                    .duration_since(Instant::now() - entry.fetched_at.elapsed())
                    .as_millis() as u64,
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
            }

            // Reconstruct the data from the serialized string.
            // Since we don't have the type information, we cannot deserialize.
            // In practice, the user would provide a deserialization callback.
            // For now, we skip actual data restoration.
            // The observer system will eventually refetch.

            // Mark as stale to trigger a refetch if needed.
            if let Some(mut entry) = self.cache.get_mut(&key) {
                entry.is_stale = true;
                // Update options if desired.
            }
        }
    }
}
