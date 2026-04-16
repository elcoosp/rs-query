//! Query definition

use crate::{InitialData, PlaceholderData, QueryError, QueryKey, QueryOptions, RetryConfig};
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

    /// Provide initial data (value)
    pub fn initial_data(mut self, data: T) -> Self {
        self.options.initial_data = Some(Box::new(data));
        self
    }

    /// Provide initial data via a lazy function
    pub fn initial_data_fn<F>(mut self, f: F) -> Self
    where
        F: Fn() -> T + Send + Sync + 'static,
    {
        self.options.initial_data_fn = Some(Arc::new(move || Box::new(f())));
        self
    }

    /// Set the timestamp when initial data was last updated
    pub fn initial_data_updated_at(mut self, timestamp: std::time::Instant) -> Self {
        self.options.initial_data_updated_at = Some(timestamp);
        self
    }

    /// Provide placeholder data (value)
    pub fn placeholder_data(mut self, data: T) -> Self {
        self.options.placeholder_data = Some(Box::new(data));
        self
    }

    /// Provide placeholder data via a function
    pub fn placeholder_data_fn<F>(mut self, f: F) -> Self
    where
        F: Fn(Option<T>) -> T + Send + Sync + 'static,
    {
        self.options.placeholder_data_fn = Some(Arc::new(
            move |prev: Option<Box<dyn std::any::Any + Send + Sync>>| {
                let prev_typed = prev.and_then(|b| b.downcast::<T>().ok()).map(|b| *b);
                Box::new(f(prev_typed))
            },
        ));
        self
    }

    /// Apply a select transformation to the query result
    pub fn select<R, F>(mut self, f: F) -> Self
    where
        R: Clone + Send + Sync + 'static,
        F: Fn(&T) -> R + Send + Sync + 'static,
    {
        self.options.select = Some(Arc::new(move |data: &dyn std::any::Any| {
            let typed = data
                .downcast_ref::<T>()
                .expect("select called on wrong type");
            Box::new(f(typed))
        }));
        self
    }

    /// Set structural sharing (default: true)
    pub fn structural_sharing(mut self, enabled: bool) -> Self {
        self.options.structural_sharing = enabled;
        self
    }
}
