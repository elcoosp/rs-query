// src/query.rs
//! Query definition and builder

use crate::{PlaceholderData, QueryError, QueryKey, QueryOptions};
use std::marker::PhantomData;
use std::pin::Pin;
use std::time::Instant;

/// Definition of a query.
pub struct Query<T: Clone> {
    pub key: QueryKey,
    pub fetch_fn: Box<
        dyn Fn() -> Pin<Box<dyn std::future::Future<Output = Result<T, QueryError>> + Send>>
            + Send
            + Sync,
    >,
    pub options: QueryOptions,
    pub initial_data: Option<T>,
    pub initial_data_updated_at: Option<Instant>,
    pub placeholder_data: Option<PlaceholderData<T>>,
    pub select: Option<Box<dyn Fn(&T) -> T + Send + Sync>>,
    _marker: PhantomData<T>,
}

impl<T: Clone + Send + Sync + 'static> Query<T> {
    pub fn new<F, Fut>(key: QueryKey, fetch_fn: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<T, QueryError>> + Send + 'static,
    {
        Self {
            key,
            fetch_fn: Box::new(move || Box::pin(fetch_fn())),
            options: QueryOptions::default(),
            initial_data: None,
            initial_data_updated_at: None,
            placeholder_data: None,
            select: None,
            _marker: PhantomData,
        }
    }

    pub fn stale_time(mut self, duration: std::time::Duration) -> Self {
        self.options.stale_time = duration;
        self
    }

    pub fn gc_time(mut self, duration: std::time::Duration) -> Self {
        self.options.gc_time = duration;
        self
    }

    pub fn retry(mut self, config: crate::RetryConfig) -> Self {
        self.options.retry = config;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.options.enabled = enabled;
        self
    }

    pub fn initial_data(mut self, data: T) -> Self {
        self.initial_data = Some(data);
        self.initial_data_updated_at = Some(Instant::now());
        self
    }

    pub fn initial_data_with_time(mut self, data: T, updated_at: Instant) -> Self {
        self.initial_data = Some(data);
        self.initial_data_updated_at = Some(updated_at);
        self
    }

    pub fn placeholder_data(mut self, data: T) -> Self {
        self.placeholder_data = Some(PlaceholderData::value(data));
        self
    }

    pub fn placeholder_data_fn<F>(mut self, f: F) -> Self
    where
        F: Fn(Option<&T>) -> T + Send + Sync + 'static,
    {
        self.placeholder_data = Some(PlaceholderData::function(f));
        self
    }

    pub fn select<F>(mut self, f: F) -> Self
    where
        F: Fn(&T) -> T + Send + Sync + 'static,
    {
        self.select = Some(Box::new(f));
        self
    }

    pub fn refetch_on_window_focus(mut self, enabled: bool) -> Self {
        self.options.refetch_on_window_focus = enabled;
        self
    }

    pub fn refetch_interval(mut self, duration: std::time::Duration) -> Self {
        self.options.refetch_interval = duration;
        self
    }

    pub fn structural_sharing(mut self, enabled: bool) -> Self {
        self.options.structural_sharing = enabled;
        self
    }

    pub fn apply_select(&self, data: &T) -> T {
        match &self.select {
            Some(f) => f(data),
            None => data.clone(),
        }
    }

    pub fn resolve_placeholder(&self, previous: Option<&T>) -> Option<T> {
        self.placeholder_data
            .as_ref()
            .map(|ph| ph.resolve(previous))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_query_new() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Ok("data".to_string()) })
        });
        assert_eq!(query.key.cache_key(), "test");
        assert!(query.initial_data.is_none());
        assert!(query.placeholder_data.is_none());
        assert!(query.select.is_none());
    }

    #[test]
    fn test_query_stale_time() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Ok("data".to_string()) })
        })
        .stale_time(Duration::from_secs(60));
        assert_eq!(query.options.stale_time, Duration::from_secs(60));
    }

    #[test]
    fn test_query_gc_time() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Ok("data".to_string()) })
        })
        .gc_time(Duration::from_secs(600));
        assert_eq!(query.options.gc_time, Duration::from_secs(600));
    }

    #[test]
    fn test_query_retry() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Ok("data".to_string()) })
        })
        .retry(crate::RetryConfig::new(5));
        assert_eq!(query.options.retry.max_retries, 5);
    }

    #[test]
    fn test_query_enabled() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Ok("data".to_string()) })
        })
        .enabled(false);
        assert!(!query.options.enabled);
    }

    #[test]
    fn test_query_initial_data() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Ok("data".to_string()) })
        })
        .initial_data("initial".to_string());
        assert_eq!(query.initial_data, Some("initial".to_string()));
        assert!(query.initial_data_updated_at.is_some());
    }

    #[test]
    fn test_query_initial_data_with_time() {
        let time = Instant::now() - Duration::from_secs(10);
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Ok("data".to_string()) })
        })
        .initial_data_with_time("initial".to_string(), time);
        assert_eq!(query.initial_data, Some("initial".to_string()));
        assert_eq!(query.initial_data_updated_at, Some(time));
    }

    #[test]
    fn test_query_placeholder_data() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Ok("data".to_string()) })
        })
        .placeholder_data("loading...".to_string());
        assert_eq!(
            query.resolve_placeholder(None),
            Some("loading...".to_string())
        );
    }

    #[test]
    fn test_query_placeholder_data_fn() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Ok("data".to_string()) })
        })
        .placeholder_data_fn(|prev| {
            prev.map(|p| format!("{} (loading)", p))
                .unwrap_or_else(|| "loading...".to_string())
        });
        assert_eq!(
            query.resolve_placeholder(Some(&"old".to_string())),
            Some("old (loading)".to_string())
        );
        assert_eq!(
            query.resolve_placeholder(None),
            Some("loading...".to_string())
        );
    }

    #[test]
    fn test_query_placeholder_data_no_placeholder() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Ok("data".to_string()) })
        });
        assert_eq!(query.resolve_placeholder(None), None);
    }

    #[test]
    fn test_query_select() {
        let query: Query<i32> = Query::new(QueryKey::new("test"), || Box::pin(async { Ok(42) }))
            .select(|data| data * 2);
        assert_eq!(query.apply_select(&42), 84);
    }

    #[test]
    fn test_query_no_select() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Ok("data".to_string()) })
        });
        assert_eq!(
            query.apply_select(&"hello".to_string()),
            "hello".to_string()
        );
    }

    #[test]
    fn test_query_refetch_on_window_focus() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Ok("data".to_string()) })
        })
        .refetch_on_window_focus(false);
        assert!(!query.options.refetch_on_window_focus);
    }

    #[test]
    fn test_query_refetch_interval() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Ok("data".to_string()) })
        })
        .refetch_interval(Duration::from_secs(30));
        assert_eq!(query.options.refetch_interval, Duration::from_secs(30));
    }

    #[test]
    fn test_query_structural_sharing() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Ok("data".to_string()) })
        })
        .structural_sharing(false);
        assert!(!query.options.structural_sharing);
    }

    #[tokio::test]
    async fn test_query_fetch_fn_success() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Ok("fetched".to_string()) })
        });
        let result = (query.fetch_fn)().await.unwrap();
        assert_eq!(result, "fetched".to_string());
    }

    #[tokio::test]
    async fn test_query_fetch_fn_error() {
        let query: Query<String> = Query::new(QueryKey::new("test"), || {
            Box::pin(async { Err(QueryError::network("fail")) })
        });
        let result = (query.fetch_fn)().await;
        assert!(result.is_err());
    }

    #[test]
    fn test_query_chained_builders() {
        let query: Query<Vec<i32>> = Query::new(QueryKey::new("test").with("id", 1), || {
            Box::pin(async { Ok(vec![1, 2, 3]) })
        })
        .stale_time(Duration::from_secs(10))
        .gc_time(Duration::from_secs(100))
        .retry(crate::RetryConfig::new(0))
        .enabled(true)
        .refetch_on_window_focus(true)
        .refetch_interval(Duration::from_secs(5))
        .structural_sharing(false)
        .select(|data| data.iter().copied().filter(|x| *x > 1).collect());

        assert_eq!(query.key.cache_key(), "test::id=1");
        assert_eq!(query.options.stale_time, Duration::from_secs(10));
        assert_eq!(query.options.gc_time, Duration::from_secs(100));
        assert_eq!(query.options.retry.max_retries, 0);
        assert!(query.options.enabled);
        assert!(query.options.refetch_on_window_focus);
        assert_eq!(query.options.refetch_interval, Duration::from_secs(5));
        assert!(!query.options.structural_sharing);

        let transformed = query.apply_select(&vec![1, 2, 3]);
        assert_eq!(transformed, vec![2, 3]);
    }
}
