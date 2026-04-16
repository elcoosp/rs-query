//! Query and mutation options

use std::time::Duration;

/// Configuration options for a query
#[derive(Debug, Clone)]
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
}

/// When to refetch on mount
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefetchOnMount {
    /// Always refetch
    Always,
    /// Only if data is stale
    IfStale,
    /// Never refetch on mount
    Never,
}

/// Retry configuration with exponential backoff
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum retry attempts
    pub max_retries: u32,
    /// Base delay for exponential backoff
    pub base_delay: Duration,
    /// Maximum delay cap
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
    /// No retries
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    /// Custom retry count
    pub fn retries(max_retries: u32) -> Self {
        Self {
            max_retries,
            ..Default::default()
        }
    }
}
