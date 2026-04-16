//! Infinite queries for paginated data

use crate::{QueryError, QueryKey, QueryOptions, RetryConfig};
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Type alias for boxed async infinite query function that receives a page parameter.
pub type InfiniteQueryFn<T, P> =
    Arc<dyn Fn(P) -> Pin<Box<dyn Future<Output = Result<T, QueryError>> + Send>> + Send + Sync>;

/// Data structure for infinite query results.
#[derive(Debug, Clone)]
pub struct InfiniteData<T, P> {
    pub pages: Vec<T>,
    pub page_params: Vec<P>,
}

impl<T, P> Default for InfiniteData<T, P> {
    fn default() -> Self {
        Self {
            pages: Vec::new(),
            page_params: Vec::new(),
        }
    }
}

/// An infinite query definition.
#[derive(Clone)]
pub struct InfiniteQuery<T, P>
where
    T: Clone + Send + Sync + Debug + 'static,
    P: Clone + Send + Sync + Debug + 'static,
{
    pub key: QueryKey,
    pub fetch_fn: InfiniteQueryFn<T, P>,
    pub initial_page_param: P,
    pub get_next_page_param: Option<Arc<dyn Fn(&T, &[T]) -> Option<P> + Send + Sync>>,
    pub get_previous_page_param: Option<Arc<dyn Fn(&T, &[T]) -> Option<P> + Send + Sync>>,
    pub options: QueryOptions,
    pub max_pages: Option<usize>,
}

impl<T, P> InfiniteQuery<T, P>
where
    T: Clone + Send + Sync + Debug + 'static,
    P: Clone + Send + Sync + Debug + 'static,
{
    /// Create a new infinite query with required parameters.
    pub fn new<F, Fut>(key: QueryKey, fetch_fn: F, initial_page_param: P) -> Self
    where
        F: Fn(P) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, QueryError>> + Send + 'static,
    {
        Self {
            key,
            fetch_fn: Arc::new(move |p| Box::pin(fetch_fn(p))),
            initial_page_param,
            get_next_page_param: None,
            get_previous_page_param: None,
            options: QueryOptions::default(),
            max_pages: None,
        }
    }

    /// Set the function to determine the next page parameter.
    pub fn get_next_page_param<F>(mut self, f: F) -> Self
    where
        F: Fn(&T, &[T]) -> Option<P> + Send + Sync + 'static,
    {
        self.get_next_page_param = Some(Arc::new(f));
        self
    }

    /// Set the function to determine the previous page parameter.
    pub fn get_previous_page_param<F>(mut self, f: F) -> Self
    where
        F: Fn(&T, &[T]) -> Option<P> + Send + Sync + 'static,
    {
        self.get_previous_page_param = Some(Arc::new(f));
        self
    }

    /// Set stale time.
    pub fn stale_time(mut self, duration: std::time::Duration) -> Self {
        self.options.stale_time = duration;
        self
    }

    /// Set garbage collection time.
    pub fn gc_time(mut self, duration: std::time::Duration) -> Self {
        self.options.gc_time = duration;
        self
    }

    /// Set retry configuration.
    pub fn retry(mut self, config: RetryConfig) -> Self {
        self.options.retry = config;
        self
    }

    /// Enable or disable the query.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.options.enabled = enabled;
        self
    }

    /// Set maximum number of pages to keep in cache.
    pub fn max_pages(mut self, max: usize) -> Self {
        self.max_pages = Some(max);
        self
    }
}
