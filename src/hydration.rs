// src/hydration.rs
//! Hydration and persistence support

use crate::{QueryClient, QueryKey, QueryOptions};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// A single dehydrated query entry.
#[derive(Clone, Debug)]
pub struct DehydratedQuery {
    pub key: String,
    pub data: String,
    pub fetched_at_secs: u64,
    pub stale_time_ms: u64,
    pub gc_time_ms: u64,
}

/// Dehydrated state containing all serializable query cache entries.
#[derive(Clone, Debug, Default)]
pub struct DehydratedState {
    pub queries: HashMap<String, DehydratedQuery>,
}

/// Options for hydrating the query cache.
pub struct HydrateOptions {
    pub should_overwrite: bool,
    pub should_hydrate: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
}

impl Clone for HydrateOptions {
    fn clone(&self) -> Self {
        Self {
            should_overwrite: self.should_overwrite,
            should_hydrate: self.should_hydrate.clone(),
        }
    }
}

impl fmt::Debug for HydrateOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HydrateOptions")
            .field("should_overwrite", &self.should_overwrite)
            .field(
                "should_hydrate",
                &self.should_hydrate.as_ref().map(|_| "[Fn]"),
            )
            .finish()
    }
}

impl Default for HydrateOptions {
    fn default() -> Self {
        Self {
            should_overwrite: true,
            should_hydrate: None,
        }
    }
}

impl QueryClient {
    pub fn dehydrate(&self) -> DehydratedState {
        let mut state = DehydratedState::default();
        for entry in self.cache.iter() {
            let key = entry.key().clone();
            let value = entry.value();

            if let Some(data) = value.data.downcast_ref::<String>() {
                state.queries.insert(
                    key.clone(),
                    DehydratedQuery {
                        key,
                        data: data.clone(),
                        fetched_at_secs: value.fetched_at.elapsed().as_secs(),
                        stale_time_ms: value.options.stale_time.as_millis() as u64,
                        gc_time_ms: value.options.gc_time.as_millis() as u64,
                    },
                );
            }
        }
        state
    }

    pub fn hydrate(&self, state: DehydratedState, options: HydrateOptions) {
        for (key, dehydrated) in state.queries {
            if let Some(ref filter) = options.should_hydrate {
                if !filter(&key) {
                    continue;
                }
            }

            if !options.should_overwrite {
                if self.cache.contains_key(&key) {
                    continue;
                }
            }

            let query_options = QueryOptions {
                stale_time: Duration::from_millis(dehydrated.stale_time_ms),
                gc_time: Duration::from_millis(dehydrated.gc_time_ms),
                ..Default::default()
            };

            let query_key = QueryKey::new(&key);
            self.set_query_data(&query_key, dehydrated.data, query_options);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dehydrate_empty_cache() {
        let client = QueryClient::new();
        let state = client.dehydrate();
        assert!(state.queries.is_empty());
    }

    #[test]
    fn test_dehydrate_with_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        let opts = QueryOptions {
            stale_time: Duration::from_secs(60),
            gc_time: Duration::from_secs(300),
            ..Default::default()
        };
        client.set_query_data(&key, "user_data".to_string(), opts);

        let state = client.dehydrate();
        assert_eq!(state.queries.len(), 1);
        let entry = state.queries.get("users").unwrap();
        assert_eq!(entry.data, "user_data");
        assert_eq!(entry.stale_time_ms, 60_000);
        assert_eq!(entry.gc_time_ms, 300_000);
    }

    #[test]
    fn test_dehydrate_non_string_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("count");
        client.set_query_data(&key, 42i32, QueryOptions::default());

        let state = client.dehydrate();
        assert!(state.queries.is_empty());
    }

    #[test]
    fn test_dehydrate_multiple_entries() {
        let client = QueryClient::new();
        client.set_query_data(
            &QueryKey::new("a"),
            "data_a".to_string(),
            QueryOptions::default(),
        );
        client.set_query_data(
            &QueryKey::new("b"),
            "data_b".to_string(),
            QueryOptions::default(),
        );

        let state = client.dehydrate();
        assert_eq!(state.queries.len(), 2);
    }

    #[test]
    fn test_hydrate_into_empty_cache() {
        let client = QueryClient::new();
        let mut queries = HashMap::new();
        queries.insert(
            "users".to_string(),
            DehydratedQuery {
                key: "users".to_string(),
                data: "restored".to_string(),
                fetched_at_secs: 0,
                stale_time_ms: 60_000,
                gc_time_ms: 300_000,
            },
        );
        let state = DehydratedState { queries };

        client.hydrate(state, HydrateOptions::default());

        let data: Option<String> = client.get_query_data(&QueryKey::new("users"));
        assert_eq!(data, Some("restored".to_string()));
    }

    #[test]
    fn test_hydrate_with_should_overwrite_false() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        client.set_query_data(&key, "existing".to_string(), QueryOptions::default());

        let mut queries = HashMap::new();
        queries.insert(
            "users".to_string(),
            DehydratedQuery {
                key: "users".to_string(),
                data: "new_data".to_string(),
                fetched_at_secs: 0,
                stale_time_ms: 0,
                gc_time_ms: 0,
            },
        );
        let state = DehydratedState { queries };

        let options = HydrateOptions {
            should_overwrite: false,
            should_hydrate: None,
        };
        client.hydrate(state, options);

        let data: Option<String> = client.get_query_data(&key);
        assert_eq!(data, Some("existing".to_string()));
    }

    #[test]
    fn test_hydrate_with_should_overwrite_true() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        client.set_query_data(&key, "existing".to_string(), QueryOptions::default());

        let mut queries = HashMap::new();
        queries.insert(
            "users".to_string(),
            DehydratedQuery {
                key: "users".to_string(),
                data: "overwritten".to_string(),
                fetched_at_secs: 0,
                stale_time_ms: 0,
                gc_time_ms: 0,
            },
        );
        let state = DehydratedState { queries };

        let options = HydrateOptions {
            should_overwrite: true,
            should_hydrate: None,
        };
        client.hydrate(state, options);

        let data: Option<String> = client.get_query_data(&key);
        assert_eq!(data, Some("overwritten".to_string()));
    }

    #[test]
    fn test_hydrate_with_filter() {
        let client = QueryClient::new();
        let mut queries = HashMap::new();
        queries.insert(
            "users".to_string(),
            DehydratedQuery {
                key: "users".to_string(),
                data: "user_data".to_string(),
                fetched_at_secs: 0,
                stale_time_ms: 0,
                gc_time_ms: 0,
            },
        );
        queries.insert(
            "posts".to_string(),
            DehydratedQuery {
                key: "posts".to_string(),
                data: "post_data".to_string(),
                fetched_at_secs: 0,
                stale_time_ms: 0,
                gc_time_ms: 0,
            },
        );
        let state = DehydratedState { queries };

        let options = HydrateOptions {
            should_overwrite: true,
            should_hydrate: Some(Arc::new(|key| key.starts_with("users"))),
        };
        client.hydrate(state, options);

        let users: Option<String> = client.get_query_data(&QueryKey::new("users"));
        let posts: Option<String> = client.get_query_data(&QueryKey::new("posts"));
        assert_eq!(users, Some("user_data".to_string()));
        assert_eq!(posts, None);
    }

    #[test]
    fn test_hydrate_with_filter_rejecting_all() {
        let client = QueryClient::new();
        let mut queries = HashMap::new();
        queries.insert(
            "users".to_string(),
            DehydratedQuery {
                key: "users".to_string(),
                data: "data".to_string(),
                fetched_at_secs: 0,
                stale_time_ms: 0,
                gc_time_ms: 0,
            },
        );
        let state = DehydratedState { queries };

        let options = HydrateOptions {
            should_overwrite: true,
            should_hydrate: Some(Arc::new(|_| false)),
        };
        client.hydrate(state, options);

        let users: Option<String> = client.get_query_data(&QueryKey::new("users"));
        assert_eq!(users, None);
    }

    #[test]
    fn test_hydrate_empty_state() {
        let client = QueryClient::new();
        let state = DehydratedState::default();
        client.hydrate(state, HydrateOptions::default());
        assert!(client.cache.is_empty());
    }

    #[test]
    fn test_dehydrated_state_default() {
        let state = DehydratedState::default();
        assert!(state.queries.is_empty());
    }

    #[test]
    fn test_dehydrated_query_clone() {
        let query = DehydratedQuery {
            key: "test".to_string(),
            data: "data".to_string(),
            fetched_at_secs: 100,
            stale_time_ms: 500,
            gc_time_ms: 1000,
        };
        let cloned = query.clone();
        assert_eq!(query.key, cloned.key);
        assert_eq!(query.data, cloned.data);
    }

    #[test]
    fn test_dehydrated_query_debug() {
        let query = DehydratedQuery {
            key: "test".to_string(),
            data: "data".to_string(),
            fetched_at_secs: 100,
            stale_time_ms: 500,
            gc_time_ms: 1000,
        };
        let debug = format!("{:?}", query);
        assert!(debug.contains("test"));
    }

    #[test]
    fn test_hydrate_options_default() {
        let opts = HydrateOptions::default();
        assert!(opts.should_overwrite);
        assert!(opts.should_hydrate.is_none());
    }

    #[test]
    fn test_hydrate_options_clone() {
        let opts = HydrateOptions {
            should_overwrite: false,
            should_hydrate: Some(Arc::new(|_| true)),
        };
        let cloned = opts.clone();
        assert!(!cloned.should_overwrite);
        assert!(cloned.should_hydrate.is_some());
    }

    #[test]
    fn test_hydrate_options_debug() {
        let opts = HydrateOptions::default();
        let debug = format!("{:?}", opts);
        assert!(debug.contains("should_overwrite"));
    }

    #[test]
    fn test_roundtrip_dehydrate_hydrate() {
        let client = QueryClient::new();
        client.set_query_data(
            &QueryKey::new("test"),
            "roundtrip".to_string(),
            QueryOptions {
                stale_time: Duration::from_secs(30),
                gc_time: Duration::from_secs(120),
                ..Default::default()
            },
        );

        let state = client.dehydrate();
        let client2 = QueryClient::new();
        client2.hydrate(state, HydrateOptions::default());

        let data: Option<String> = client2.get_query_data(&QueryKey::new("test"));
        assert_eq!(data, Some("roundtrip".to_string()));
    }
}
