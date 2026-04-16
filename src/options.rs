//! Query and mutation options

use std::any::Any;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Initial data for a query – either a concrete value or a lazy function.
pub enum InitialData<T: Clone + Send + Sync + 'static> {
    Value(T),
    Function(Arc<dyn Fn() -> T + Send + Sync>),
}

impl<T: Clone + Send + Sync + 'static> Clone for InitialData<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Value(v) => Self::Value(v.clone()),
            Self::Function(f) => Self::Function(Arc::clone(f)),
        }
    }
}

/// Placeholder data for a query – value or function that receives previous data.
pub enum PlaceholderData<T: Clone + Send + Sync + 'static> {
    Value(T),
    Function(Arc<dyn Fn(Option<T>) -> T + Send + Sync>),
}

impl<T: Clone + Send + Sync + 'static> Clone for PlaceholderData<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Value(v) => Self::Value(v.clone()),
            Self::Function(f) => Self::Function(Arc::clone(f)),
        }
    }
}

/// Configuration options for a query
#[derive(Clone)]
pub struct QueryOptions {
    /// Time after which data is considered stale (default: 0 = immediately)
    pub stale_time: Duration,
    /// Time after which inactive cache is garbage collected (default: 5 min)
    pub gc_time: Duration,
    /// Whether to refetch when query becomes active
    pub refetch_on_mount: RefetchOnMount,
    /// Retry configuration
    pub retry: RetryConfig,
    /// Whether query is enabled
    pub enabled: bool,
    /// Initial data to populate cache if empty
    pub initial_data: Option<Arc<dyn Any + Send + Sync>>,
    /// Lazy initial data function
    pub initial_data_fn: Option<Arc<dyn Fn() -> Arc<dyn Any + Send + Sync> + Send + Sync>>,
    /// Timestamp when initial data was last updated
    pub initial_data_updated_at: Option<Instant>,
    /// Placeholder data shown while fetching
    pub placeholder_data: Option<Arc<dyn Any + Send + Sync>>,
    /// Placeholder data function (previous_data) -> placeholder
    pub placeholder_data_fn: Option<
        Arc<dyn Fn(Option<Arc<dyn Any + Send + Sync>>) -> Arc<dyn Any + Send + Sync> + Send + Sync>,
    >,
    /// Transform function applied to cached data
    pub select: Option<Arc<dyn Fn(&dyn Any) -> Arc<dyn Any + Send + Sync> + Send + Sync>>,
    /// Whether to use structural sharing (reserved for future)
    pub structural_sharing: bool,
    /// Interval for automatic background refetch (None = disabled)
    pub refetch_interval: Option<Duration>,
    /// Whether to continue refetch interval when app is in background
    pub refetch_interval_in_background: bool,
    /// Refetch on window focus when data is stale
    pub refetch_on_window_focus: bool,
}

impl std::fmt::Debug for QueryOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryOptions")
            .field("stale_time", &self.stale_time)
            .field("gc_time", &self.gc_time)
            .field("refetch_on_mount", &self.refetch_on_mount)
            .field("retry", &self.retry)
            .field("enabled", &self.enabled)
            .field("initial_data_updated_at", &self.initial_data_updated_at)
            .field("structural_sharing", &self.structural_sharing)
            .field("refetch_interval", &self.refetch_interval)
            .field(
                "refetch_interval_in_background",
                &self.refetch_interval_in_background,
            )
            .field("refetch_on_window_focus", &self.refetch_on_window_focus)
            .finish_non_exhaustive()
    }
}

/// When to refetch on mount
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefetchOnMount {
    Always,
    IfStale,
    Never,
}

/// Retry configuration with exponential backoff
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            stale_time: Duration::ZERO,
            gc_time: Duration::from_secs(5 * 60),
            refetch_on_mount: RefetchOnMount::IfStale,
            retry: RetryConfig::default(),
            enabled: true,
            initial_data: None,
            initial_data_fn: None,
            initial_data_updated_at: None,
            placeholder_data: None,
            placeholder_data_fn: None,
            select: None,
            structural_sharing: true,
            refetch_interval: None,
            refetch_interval_in_background: false,
            refetch_on_window_focus: true,
        }
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(1000),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryConfig {
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    pub fn retries(max_retries: u32) -> Self {
        Self {
            max_retries,
            ..Default::default()
        }
    }
}
