//! Query definition

use crate::{QueryError, QueryKey, QueryOptions, RetryConfig};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Type alias for boxed async query function
pub type QueryFn<T> =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<T, QueryError>> + Send>> + Send + Sync>;

/// A query definition.
pub struct Query<T: Clone + Send + Sync + 'static> {
    pub key: QueryKey,
    pub fetch_fn: QueryFn<T>,
    pub options: QueryOptions,
}

impl<T: Clone + Send + Sync + 'static> Query<T> {
    /// Create a new query
    pub fn new<F, Fut>(key: QueryKey, fetch_fn: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, QueryError>> + Send + 'static,
    {
        Self {
            key,
            fetch_fn: Arc::new(move || Box::pin(fetch_fn())),
            options: QueryOptions::default(),
        }
    }

    /// Set stale time
    pub fn stale_time(mut self, duration: std::time::Duration) -> Self {
        self.options.stale_time = duration;
        self
    }

    /// Set garbage collection time
    pub fn gc_time(mut self, duration: std::time::Duration) -> Self {
        self.options.gc_time = duration;
        self
    }

    /// Set retry configuration
    pub fn retry(mut self, config: RetryConfig) -> Self {
        self.options.retry = config;
        self
    }

    /// Enable or disable the query
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.options.enabled = enabled;
        self
    }
}
