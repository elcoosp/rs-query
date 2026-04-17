// src/query.rs
//! Query definition and builder

use crate::options::{PlaceholderData, QueryOptions};
use crate::QueryError;
use crate::QueryKey;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

/// Definition of a query.
pub struct Query<T: Clone + Send + Sync + 'static> {
    /// Query key.
    pub key: QueryKey,
    /// Function to fetch data.
    pub fetch_fn: Arc<
        dyn Fn() -> Pin<Box<dyn Future<Output = Result<T, QueryError>> + Send + Sync>>
            + Send
            + Sync,
    >,
    /// Optional select transformation.
    pub select: Option<Arc<dyn Fn(&T) -> T + Send + Sync>>,
    /// Query options.
    pub options: QueryOptions,
    /// Initial data to seed the cache.
    pub initial_data: Option<T>,
    /// Timestamp when initial data was set (used for staleness).
    pub initial_data_updated_at: Option<Instant>,
    /// Placeholder data shown during loading.
    pub placeholder_data: Option<PlaceholderData<T>>,
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
        }
    }
}

impl<T: Clone + Send + Sync + 'static> Query<T> {
    /// Create a new query.
    pub fn new<F, Fut>(key: QueryKey, fetch_fn: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, QueryError>> + Send + Sync + 'static,
    {
        Self {
            key,
            fetch_fn: Arc::new(move || Box::pin(fetch_fn())),
            select: None,
            options: QueryOptions::default(),
            initial_data: None,
            initial_data_updated_at: None,
            placeholder_data: None,
        }
    }

    /// Set initial data to seed the cache.
    pub fn initial_data(mut self, data: T) -> Self {
        self.initial_data = Some(data);
        self.initial_data_updated_at = Some(Instant::now());
        self
    }

    /// Set placeholder data shown during loading.
    pub fn placeholder_data(mut self, data: PlaceholderData<T>) -> Self {
        self.placeholder_data = Some(data);
        self
    }

    /// Set stale time.
    pub fn stale_time(mut self, duration: std::time::Duration) -> Self {
        self.options.stale_time = duration;
        self
    }

    /// Set GC time.
    pub fn gc_time(mut self, duration: std::time::Duration) -> Self {
        self.options.gc_time = duration;
        self
    }

    /// Set retry options.
    pub fn retry(mut self, retry: crate::options::RetryConfig) -> Self {
        self.options.retry = retry;
        self
    }

    /// Set whether query is enabled.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.options.enabled = enabled;
        self
    }

    /// Set select transformation.
    pub fn select<F>(mut self, f: F) -> Self
    where
        F: Fn(&T) -> T + Send + Sync + 'static,
    {
        self.select = Some(Arc::new(f));
        self
    }

    /// Set refetch interval.
    pub fn refetch_interval(mut self, duration: std::time::Duration) -> Self {
        self.options.refetch_interval = duration;
        self
    }

    /// Set refetch on window focus.
    pub fn refetch_on_window_focus(mut self, enabled: bool) -> Self {
        self.options.refetch_on_window_focus = enabled;
        self
    }

    /// Set structural sharing (requires `T: PartialEq`).
    pub fn structural_sharing(mut self, enabled: bool) -> Self
    where
        T: PartialEq,
    {
        self.options.structural_sharing = enabled;
        self
    }

    /// Set full query options.
    pub fn options(mut self, options: QueryOptions) -> Self {
        self.options = options;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::RetryConfig;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_query_new() {
        let query: Query<String> =
            Query::new(QueryKey::new("test"), || async { Ok("data".to_string()) });
        let result = (query.fetch_fn)().await.unwrap();
        assert_eq!(result, "data");
    }

    #[tokio::test]
    async fn test_query_error() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || async {
            Err(QueryError::network("fail"))
        });
        let result = (query.fetch_fn)().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_query_clone() {
        let query: Query<String> =
            Query::new(QueryKey::new("test"), || async { Ok("data".to_string()) });
        let cloned = query.clone();
        assert_eq!(cloned.key.cache_key(), "test");
        let result = (cloned.fetch_fn)().await.unwrap();
        assert_eq!(result, "data");
    }

    #[tokio::test]
    async fn test_query_builder() {
        let query: Query<String> =
            Query::new(QueryKey::new("test"), || async { Ok("data".to_string()) })
                .stale_time(std::time::Duration::from_secs(60))
                .gc_time(std::time::Duration::from_secs(300))
                .enabled(true)
                .refetch_on_window_focus(false)
                .structural_sharing(false);

        assert_eq!(query.options.stale_time, std::time::Duration::from_secs(60));
        assert_eq!(query.options.gc_time, std::time::Duration::from_secs(300));
        assert!(query.options.enabled);
        assert!(!query.options.refetch_on_window_focus);
        assert!(!query.options.structural_sharing);

        let result = (query.fetch_fn)().await.unwrap();
        assert_eq!(result, "data");
    }

    #[tokio::test]
    async fn test_query_select() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);

        let query: Query<String> =
            Query::new(QueryKey::new("test"), || async { Ok("data".to_string()) }).select(
                move |s: &String| {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                    s.to_uppercase()
                },
            );

        assert!(query.select.is_some());
        let select = query.select.as_ref().unwrap();
        let result = select(&"hello".to_string());
        assert_eq!(result, "HELLO");
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        let fetched = (query.fetch_fn)().await.unwrap();
        assert_eq!(fetched, "data");
    }

    #[tokio::test]
    async fn test_query_options_builder() {
        let opts = QueryOptions {
            stale_time: std::time::Duration::from_secs(120),
            ..Default::default()
        };
        let query: Query<String> =
            Query::new(QueryKey::new("test"), || async { Ok("data".to_string()) }).options(opts);

        assert_eq!(
            query.options.stale_time,
            std::time::Duration::from_secs(120)
        );

        let result = (query.fetch_fn)().await.unwrap();
        assert_eq!(result, "data");
    }

    #[tokio::test]
    async fn test_query_retry() {
        let query: Query<String> =
            Query::new(QueryKey::new("test"), || async { Ok("data".to_string()) })
                .retry(RetryConfig::default());

        assert_eq!(query.options.retry, RetryConfig::default());

        let result = (query.fetch_fn)().await.unwrap();
        assert_eq!(result, "data");
    }

    #[tokio::test]
    async fn test_query_refetch_interval() {
        let query: Query<String> =
            Query::new(QueryKey::new("test"), || async { Ok("data".to_string()) })
                .refetch_interval(std::time::Duration::from_secs(30));

        assert_eq!(
            query.options.refetch_interval,
            std::time::Duration::from_secs(30)
        );

        let result = (query.fetch_fn)().await.unwrap();
        assert_eq!(result, "data");
    }
}
