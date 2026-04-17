// src/query.rs
//! Query definition with builder pattern for configuring fetch behaviour.

use crate::{QueryError, QueryKey, QueryOptions};
use std::any::Any;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Placeholder data that is shown while a query is loading.
///
/// Can be a static value or a function that receives the previous data
/// (if any) and returns placeholder data, mirroring TanStack Query's
/// `placeholderData` option.
#[derive(Clone)]
pub enum PlaceholderData<T: Clone + Send + Sync + 'static> {
    /// A fixed value used as placeholder.
    Value(T),
    /// A function that receives the previous data (if any) and produces placeholder data.
    Function(Arc<dyn Fn(Option<&T>) -> T + Send + Sync>),
}

impl<T: Clone + Send + Sync + 'static> std::fmt::Debug for PlaceholderData<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Value(_) => f.write_str("PlaceholderData::Value(..)"),
            Self::Function(_) => f.write_str("PlaceholderData::Function(..)"),
        }
    }
}

impl<T: Clone + Send + Sync + 'static> PlaceholderData<T> {
    /// Create a placeholder from a static value.
    pub fn value(data: T) -> Self {
        Self::Value(data)
    }

    /// Create a placeholder from a function that may use previous data.
    pub fn function(f: impl Fn(Option<&T>) -> T + Send + Sync + 'static) -> Self {
        Self::Function(Arc::new(f))
    }

    /// Resolve the placeholder data, optionally using previous data.
    pub fn resolve(&self, previous_data: Option<&T>) -> T {
        match self {
            Self::Value(v) => v.clone(),
            Self::Function(f) => f(previous_data),
        }
    }
}

/// A query definition: a unique key, an async fetch function, and configuration options.
///
/// Use the builder methods to configure stale time, retries, structural sharing, etc.
pub struct Query<T: Clone + Send + Sync + 'static> {
    pub key: QueryKey,
    pub fetch_fn: Arc<
        dyn Fn()
                -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, QueryError>> + Send>>
            + Send
            + Sync,
    >,
    pub select: Option<Arc<dyn Fn(&T) -> T + Send + Sync>>,
    pub options: QueryOptions,

    /// Optional initial data that is placed into the cache on first observation.
    pub initial_data: Option<T>,
    /// Timestamp when initial data was set (used for stale-time calculations).
    pub initial_data_updated_at: Option<Instant>,
    /// Optional placeholder data shown during loading states.
    pub placeholder_data: Option<PlaceholderData<T>>,

    /// Structural sharing closure, created when `.structural_sharing(true)` is called.
    /// Captures the `T: PartialEq` bound at construction time so downstream code
    /// doesn't need the bound.
    pub(crate) share_fn: Option<
        Arc<dyn Fn(Arc<dyn Any + Send + Sync>, Arc<T>) -> Arc<dyn Any + Send + Sync> + Send + Sync>,
    >,
}

impl<T: Clone + Send + Sync + 'static> Clone for Query<T> {
    fn clone(&self) -> Self {
        Self {
            key: self.key.clone(),
            fetch_fn: Arc::clone(&self.fetch_fn),
            select: self.select.clone(),
            options: self.options.clone(),
            initial_data: self.initial_data.clone(),
            initial_data_updated_at: self.initial_data_updated_at,
            placeholder_data: self.placeholder_data.clone(),
            share_fn: self.share_fn.clone(),
        }
    }
}

impl<T: Clone + Send + Sync + 'static> Query<T> {
    /// Create a new query with a key and async fetch function.
    pub fn new<F, Fut>(key: QueryKey, fetch_fn: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<T, QueryError>> + Send + 'static,
    {
        Self {
            key,
            fetch_fn: Arc::new(move || Box::pin(fetch_fn())),
            select: None,
            options: QueryOptions::default(),
            initial_data: None,
            initial_data_updated_at: None,
            placeholder_data: None,
            share_fn: None,
        }
    }

    /// Set how long data is considered fresh before becoming stale.
    pub fn stale_time(mut self, duration: Duration) -> Self {
        self.options.stale_time = duration;
        self
    }

    /// Set how long unused cache entries are kept before garbage collection.
    pub fn gc_time(mut self, duration: Duration) -> Self {
        self.options.gc_time = duration;
        self
    }

    /// Set custom retry behaviour.
    pub fn retry(mut self, config: crate::options::RetryConfig) -> Self {
        self.options.retry = config;
        self
    }

    /// Conditionally enable or disable the query.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.options.enabled = enabled;
        self
    }

    /// Transform fetched data before it is stored in the cache.
    pub fn select(mut self, f: impl Fn(&T) -> T + Send + Sync + 'static) -> Self {
        self.select = Some(Arc::new(f));
        self
    }

    /// Set an interval for automatic background refetching.
    pub fn refetch_interval(mut self, duration: Duration) -> Self {
        self.options.refetch_interval = duration;
        self
    }

    /// Enable or disable refetching when the window regains focus.
    pub fn refetch_on_window_focus(mut self, enabled: bool) -> Self {
        self.options.refetch_on_window_focus = enabled;
        self
    }

    /// Enable structural sharing.
    ///
    /// When enabled, if the newly fetched data is `PartialEq`-equal to the
    /// existing cached data, the old `Arc` is kept so that downstream
    /// consumers can skip re-renders via `Arc::ptr_eq`.
    ///
    /// **Requires `T: PartialEq`.** This bound is enforced at compile time.
    pub fn structural_sharing(mut self, enabled: bool) -> Self
    where
        T: PartialEq,
    {
        if enabled {
            self.options.structural_sharing = true;
            self.share_fn = Some(Arc::new(
                |old_any: Arc<dyn Any + Send + Sync>, new_arc: Arc<T>| {
                    if let Some(old_typed) = old_any.downcast_ref::<T>() {
                        if *old_typed == *new_arc {
                            return old_any;
                        }
                    }
                    new_arc
                },
            ));
        } else {
            self.options.structural_sharing = false;
            self.share_fn = None;
        }
        self
    }

    /// Set all options at once.
    pub fn options(mut self, options: QueryOptions) -> Self {
        self.options = options;
        self
    }

    /// Provide initial data that is placed into the cache when the query
    /// is first observed via [`QueryObserver::from_query`].
    ///
    /// This mirrors TanStack Query's `initialData` option.
    pub fn initial_data(mut self, data: T) -> Self {
        self.initial_data = Some(data);
        self.initial_data_updated_at = Some(Instant::now());
        self
    }

    /// Provide placeholder data shown while the query is loading.
    ///
    /// Accepts a [`PlaceholderData`] which can be a static value or a
    /// function that receives the previous data (if any).
    pub fn placeholder_data(mut self, data: PlaceholderData<T>) -> Self {
        self.placeholder_data = Some(data);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_placeholder_data_value() {
        let ph = PlaceholderData::value("hello".to_string());
        assert_eq!(ph.resolve(None), "hello");
        assert_eq!(ph.resolve(Some(&"world".to_string())), "hello");
    }

    #[test]
    fn test_placeholder_data_function() {
        let ph: PlaceholderData<String> =
            PlaceholderData::function(|prev| prev.map(|s| s.clone()).unwrap_or_default());
        assert_eq!(ph.resolve(None), "");
        assert_eq!(ph.resolve(Some(&"world".to_string())), "world");
    }

    #[test]
    fn test_query_with_initial_data() {
        use crate::QueryClient;

        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<String> = Query::new(key.clone(), || async { Ok("fetched".to_string()) })
            .initial_data("initial".to_string());

        let observer = crate::QueryObserver::from_query(&client, &query);
        assert_eq!(observer.state().data(), Some(&"initial".to_string()));
    }

    #[tokio::test]
    async fn test_placeholder_data_shown_during_loading() {
        use crate::QueryClient;
        use std::time::Duration;

        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<String> = Query::new(key.clone(), || async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok("fetched".to_string())
        })
        .placeholder_data(PlaceholderData::value("placeholder".to_string()));

        let mut observer = crate::QueryObserver::from_query(&client, &query);
        observer.set_loading();
        assert!(matches!(
            observer.state(),
            QueryState::LoadingWithPlaceholder(ref s) if s == "placeholder"
        ));
    }

    #[test]
    fn test_structural_sharing_requires_partial_eq() {
        // This test verifies that structural_sharing(true) compiles only when T: PartialEq.
        let _query: Query<Vec<i32>> =
            Query::new(QueryKey::new("test"), || async { Ok(vec![1, 2, 3]) })
                .structural_sharing(true);
    }

    #[test]
    fn test_query_state_idle() {
        let state = QueryState::<String>::Idle;
        assert!(state.is_idle());
        assert!(!state.is_loading());
        assert!(!state.is_success());
        assert!(!state.is_error());
        assert_eq!(state.data(), None);
    }

    #[test]
    fn test_query_state_loading() {
        let state = QueryState::<String>::Loading;
        assert!(!state.is_idle());
        assert!(state.is_loading());
        assert!(!state.is_success());
        assert!(!state.is_error());
        assert_eq!(state.data(), None);
    }

    #[test]
    fn test_query_state_success() {
        let state = QueryState::Success("hello".to_string());
        assert!(!state.is_idle());
        assert!(!state.is_loading());
        assert!(state.is_success());
        assert!(!state.is_error());
        assert_eq!(state.data(), Some(&"hello".to_string()));
    }

    #[test]
    fn test_query_state_error() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("test error"),
            stale_data: None,
        };
        assert!(!state.is_idle());
        assert!(!state.is_loading());
        assert!(!state.is_success());
        assert!(state.is_error());
        assert_eq!(state.data(), None);
        assert_eq!(state.error(), Some(&QueryError::custom("test error")));
    }

    #[test]
    fn test_query_state_error_with_stale_data() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("test error"),
            stale_data: Some("stale".to_string()),
        };
        assert!(state.is_error());
        assert_eq!(state.data(), Some(&"stale".to_string()));
    }

    #[test]
    fn test_query_state_refetching() {
        let state = QueryState::Refetching("hello".to_string());
        assert!(!state.is_idle());
        assert!(!state.is_loading());
        assert!(!state.is_success());
        assert!(!state.is_error());
        assert!(state.is_refetching());
        assert_eq!(state.data(), Some(&"hello".to_string()));
    }

    #[test]
    fn test_query_state_stale() {
        let state = QueryState::Stale("hello".to_string());
        assert!(!state.is_idle());
        assert!(!state.is_loading());
        assert!(!state.is_success());
        assert!(!state.is_error());
        assert!(state.is_stale());
        assert_eq!(state.data(), Some(&"hello".to_string()));
    }

    #[test]
    fn test_data_mut_idle() {
        let mut state: QueryState<String> = QueryState::Idle;
        assert_eq!(state.data_mut(), None);
    }

    #[test]
    fn test_data_mut_loading() {
        let mut state: QueryState<String> = QueryState::Loading;
        assert_eq!(state.data_mut(), None);
    }

    #[test]
    fn test_data_mut_success() {
        let mut state = QueryState::Success("hello".to_string());
        if let Some(data) = state.data_mut() {
            *data = "world".to_string();
        }
        assert_eq!(state.data(), Some(&"world".to_string()));
    }

    #[test]
    fn test_data_mut_error() {
        let mut state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("test error"),
            stale_data: Some("stale".to_string()),
        };
        if let Some(data) = state.data_mut() {
            *data = "updated".to_string();
        }
        assert_eq!(state.data(), Some(&"updated".to_string()));
    }

    #[test]
    fn test_data_mut_error_no_stale() {
        let mut state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("test error"),
            stale_data: None,
        };
        assert_eq!(state.data_mut(), None);
    }

    #[test]
    fn test_data_mut_refetching() {
        let mut state = QueryState::Refetching("hello".to_string());
        if let Some(data) = state.data_mut() {
            *data = "world".to_string();
        }
        assert_eq!(state.data(), Some(&"world".to_string()));
    }

    #[test]
    fn test_data_mut_stale() {
        let mut state = QueryState::Stale("hello".to_string());
        if let Some(data) = state.data_mut() {
            *data = "world".to_string();
        }
        assert_eq!(state.data(), Some(&"world".to_string()));
    }
    #[test]
    fn test_has_data_true() {
        let state = QueryState::Success("hello".to_string());
        assert!(state.has_data());
    }

    #[test]
    fn test_has_data_false() {
        let state = QueryState::<String>::Idle;
        assert!(!state.has_data());
    }

    #[test]
    fn test_has_data_loading() {
        let state = QueryState::<String>::Loading;
        assert!(!state.has_data());
    }

    #[test]
    fn test_has_data_error_no_stale() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: None,
        };
        assert!(!state.has_data());
    }

    #[test]
    fn test_has_data_error_with_stale() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: Some("stale".to_string()),
        };
        assert!(state.has_data());
    }

    #[test]
    fn test_map_success() {
        let state = QueryState::Success("hello".to_string());
        let result = state.map(|d| d.len());
        assert_eq!(result, Some(5));
    }

    #[test]
    fn test_map_idle() {
        let state = QueryState::<String>::Idle;
        let result = state.map(|d| d.len());
        assert_eq!(result, None);
    }

    #[test]
    fn test_map_loading() {
        let state = QueryState::<String>::Loading;
        let result = state.map(|d| d.len());
        assert_eq!(result, None);
    }

    #[test]
    fn test_map_error() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: None,
        };
        let result = state.map(|d| d.len());
        assert_eq!(result, None);
    }

    #[test]
    fn test_map_error_with_stale() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: Some("stale".to_string()),
        };
        let result = state.map(|d| d.len());
        assert_eq!(result, Some(5));
    }

    #[test]
    fn test_map_refetching() {
        let state = QueryState::Refetching("hello".to_string());
        let result = state.map(|d| d.len());
        assert_eq!(result, Some(5));
    }

    #[test]
    fn test_map_stale() {
        let state = QueryState::Stale("hello".to_string());
        let result = state.map(|d| d.len());
        assert_eq!(result, Some(5));
    }

    #[test]
    fn test_data_cloned_success() {
        let state = QueryState::Success("hello".to_string());
        assert_eq!(state.data_cloned(), Some("hello".to_string()));
    }

    #[test]
    fn test_data_cloned_idle() {
        let state = QueryState::<String>::Idle;
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_data_cloned_loading() {
        let state = QueryState::<String>::Loading;
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_data_cloned_error_no_stale() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: None,
        };
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_data_cloned_error_with_stale() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: Some("stale".to_string()),
        };
        assert_eq!(state.data_cloned(), Some("stale".to_string()));
    }

    #[test]
    fn test_data_cloned_refetching() {
        let state = QueryState::Refetching("hello".to_string());
        assert_eq!(state.data_cloned(), Some("hello".to_string()));
    }

    #[test]
    fn test_data_cloned_stale() {
        let state = QueryState::Stale("hello".to_string());
        assert_eq!(state.data_cloned(), Some("hello".to_string()));
    }

    #[test]
    fn test_unwrap_success() {
        let state = QueryState::Success("hello".to_string());
        assert_eq!(state.unwrap(), "hello".to_string());
    }

    #[test]
    fn test_unwrap_refetching() {
        let state = QueryState::Refetching("hello".to_string());
        assert_eq!(state.unwrap(), "hello".to_string());
    }

    #[test]
    fn test_unwrap_stale() {
        let state = QueryState::Stale("hello".to_string());
        assert_eq!(state.unwrap(), "hello".to_string());
    }

    #[test]
    fn test_unwrap_error_with_stale() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: Some("stale".to_string()),
        };
        assert_eq!(state.unwrap(), "stale".to_string());
    }

    #[test]
    #[should_panic(expected = "called unwrap on a QueryState without data")]
    fn test_unwrap_idle_panics() {
        let state = QueryState::<String>::Idle;
        state.unwrap();
    }

    #[test]
    #[should_panic(expected = "called unwrap on a QueryState without data")]
    fn test_unwrap_loading_panics() {
        let state = QueryState::<String>::Loading;
        state.unwrap();
    }

    #[test]
    #[should_panic(expected = "called unwrap on a QueryState without data")]
    fn test_unwrap_error_no_stale_panics() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: None,
        };
        state.unwrap();
    }

    #[test]
    fn test_mutation_state_data_cloned_success() {
        let state = MutationState::Success("data".to_string());
        assert_eq!(state.data_cloned(), Some("data".to_string()));
    }

    #[test]
    fn test_mutation_state_data_cloned_idle() {
        let state = MutationState::<String>::Idle;
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_mutation_state_data_cloned_loading() {
        let state = MutationState::<String>::Loading;
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_mutation_state_data_cloned_error() {
        let state = MutationState::<String>::Error(QueryError::custom("err"));
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_mutation_state_default() {
        let state = MutationState::<String>::default();
        assert!(state.is_idle());
    }

    #[test]
    fn test_query_state_default() {
        let state = QueryState::<String>::default();
        assert!(state.is_idle());
    }
}
