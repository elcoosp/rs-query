// src/options.rs
//! Query options configuration

use std::time::Duration;

/// Configuration for automatic retry behaviour.
#[derive(Clone, Debug)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (not counting the initial request).
    pub max_retries: u32,
    /// Base delay between retries.
    pub base_delay: Duration,
    /// Whether to use exponential backoff.
    pub exponential_backoff: bool,
}

impl RetryConfig {
    /// Create a new retry configuration.
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries,
            base_delay: Duration::from_millis(500),
            exponential_backoff: true,
        }
    }

    /// Set the base delay between retries.
    pub fn base_delay(mut self, delay: Duration) -> Self {
        self.base_delay = delay;
        self
    }

    /// Enable or disable exponential backoff.
    pub fn exponential_backoff(mut self, enabled: bool) -> Self {
        self.exponential_backoff = enabled;
        self
    }

    /// Calculate the delay for a given retry attempt (0-indexed).
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        if self.exponential_backoff {
            let multiplier = 2u32.saturating_pow(attempt);
            self.base_delay * multiplier
        } else {
            self.base_delay
        }
    }

    /// Check if a retry should be attempted.
    pub fn should_retry(&self, attempt: u32) -> bool {
        attempt < self.max_retries
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
            exponential_backoff: true,
        }
    }
}

/// Placeholder data configuration.
#[derive(Clone)]
pub enum PlaceholderData<T: Clone> {
    /// Static placeholder data.
    Value(T),
    /// Function that computes placeholder data from previous data.
    Function(std::sync::Arc<dyn Fn(Option<&T>) -> T + Send + Sync>),
}

impl<T: Clone + std::fmt::Debug> std::fmt::Debug for PlaceholderData<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Value(v) => f.debug_tuple("Value").field(v).finish(),
            Self::Function(_) => f.debug_tuple("Function").finish(),
        }
    }
}

impl<T: Clone> PlaceholderData<T> {
    /// Create a static placeholder.
    pub fn value(data: T) -> Self {
        Self::Value(data)
    }

    /// Create a function-based placeholder.
    pub fn function<F>(f: F) -> Self
    where
        F: Fn(Option<&T>) -> T + Send + Sync + 'static,
    {
        Self::Function(std::sync::Arc::new(f))
    }

    /// Resolve the placeholder data.
    pub fn resolve(&self, previous: Option<&T>) -> T {
        match self {
            PlaceholderData::Value(v) => v.clone(),
            PlaceholderData::Function(f) => f(previous),
        }
    }
}

/// Options for query behaviour.
#[derive(Clone, Debug)]
pub struct QueryOptions {
    /// How long data is considered fresh before becoming stale.
    pub stale_time: Duration,
    /// How long unused cache entries are kept before garbage collection.
    pub gc_time: Duration,
    /// Retry configuration.
    pub retry: RetryConfig,
    /// Whether the query is enabled.
    pub enabled: bool,
    /// Whether to refetch when the window regains focus.
    pub refetch_on_window_focus: bool,
    /// Interval for automatic background refetching.
    pub refetch_interval: Duration,
    /// Whether to apply structural sharing when updating cache.
    pub structural_sharing: bool,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            stale_time: Duration::ZERO,
            gc_time: Duration::from_secs(300),
            retry: RetryConfig::default(),
            enabled: true,
            refetch_on_window_focus: true,
            refetch_interval: Duration::ZERO,
            structural_sharing: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_config_new() {
        let config = RetryConfig::new(5);
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.base_delay, Duration::from_millis(500));
        assert!(config.exponential_backoff);
    }

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.base_delay, Duration::from_millis(500));
        assert!(config.exponential_backoff);
    }

    #[test]
    fn test_retry_config_base_delay_builder() {
        let config = RetryConfig::new(2).base_delay(Duration::from_secs(1));
        assert_eq!(config.base_delay, Duration::from_secs(1));
    }

    #[test]
    fn test_retry_config_exponential_backoff_builder() {
        let config = RetryConfig::new(2).exponential_backoff(false);
        assert!(!config.exponential_backoff);
    }

    #[test]
    fn test_retry_config_delay_for_attempt_exponential() {
        let config = RetryConfig::new(3).base_delay(Duration::from_millis(100));
        assert_eq!(config.delay_for_attempt(0), Duration::from_millis(100));
        assert_eq!(config.delay_for_attempt(1), Duration::from_millis(200));
        assert_eq!(config.delay_for_attempt(2), Duration::from_millis(400));
        assert_eq!(config.delay_for_attempt(3), Duration::from_millis(800));
    }

    #[test]
    fn test_retry_config_delay_for_attempt_linear() {
        let config = RetryConfig::new(3)
            .base_delay(Duration::from_millis(100))
            .exponential_backoff(false);
        assert_eq!(config.delay_for_attempt(0), Duration::from_millis(100));
        assert_eq!(config.delay_for_attempt(1), Duration::from_millis(100));
        assert_eq!(config.delay_for_attempt(2), Duration::from_millis(100));
        assert_eq!(config.delay_for_attempt(3), Duration::from_millis(100));
    }

    #[test]
    fn test_retry_config_should_retry() {
        let config = RetryConfig::new(3);
        assert!(config.should_retry(0));
        assert!(config.should_retry(1));
        assert!(config.should_retry(2));
        assert!(!config.should_retry(3));
        assert!(!config.should_retry(4));
        assert!(!config.should_retry(100));
    }

    #[test]
    fn test_retry_config_should_retry_zero() {
        let config = RetryConfig::new(0);
        assert!(!config.should_retry(0));
        assert!(!config.should_retry(1));
    }

    #[test]
    fn test_retry_config_debug() {
        let config = RetryConfig::new(3);
        let debug = format!("{:?}", config);
        assert!(debug.contains("max_retries"));
        assert!(debug.contains("3"));
    }

    #[test]
    fn test_placeholder_data_value() {
        let ph = PlaceholderData::value("placeholder".to_string());
        assert_eq!(ph.resolve(None), "placeholder".to_string());
        assert_eq!(
            ph.resolve(Some(&"previous".to_string())),
            "placeholder".to_string()
        );
    }

    #[test]
    fn test_placeholder_data_function_with_previous() {
        let ph: PlaceholderData<String> = PlaceholderData::function(|prev| {
            prev.map(|p| format!("{} (loading...)", p))
                .unwrap_or_else(|| "loading...".to_string())
        });

        assert_eq!(ph.resolve(Some(&"old".to_string())), "old (loading...)");
        assert_eq!(ph.resolve(None), "loading...");
    }

    #[test]
    fn test_placeholder_data_value_clone() {
        let ph = PlaceholderData::value(42i32);
        let cloned = ph.clone();
        assert_eq!(ph.resolve(None), cloned.resolve(None));
    }

    #[test]
    fn test_placeholder_data_function_clone() {
        let ph: PlaceholderData<Vec<i32>> =
            PlaceholderData::function(|prev| prev.cloned().unwrap_or_default());
        let cloned = ph.clone();
        assert_eq!(cloned.resolve(None), Vec::<i32>::new());
    }

    #[test]
    fn test_placeholder_data_debug_value() {
        let ph = PlaceholderData::value("test".to_string());
        let debug = format!("{:?}", ph);
        assert!(debug.contains("Value"));
    }

    #[test]
    fn test_placeholder_data_debug_function() {
        let ph: PlaceholderData<String> = PlaceholderData::function(|_| "computed".to_string());
        let debug = format!("{:?}", ph);
        assert!(debug.contains("Function"));
    }

    #[test]
    fn test_query_options_default() {
        let opts = QueryOptions::default();
        assert_eq!(opts.stale_time, Duration::ZERO);
        assert_eq!(opts.gc_time, Duration::from_secs(300));
        assert_eq!(opts.retry.max_retries, 3);
        assert!(opts.enabled);
        assert!(opts.refetch_on_window_focus);
        assert_eq!(opts.refetch_interval, Duration::ZERO);
        assert!(opts.structural_sharing);
    }

    #[test]
    fn test_query_options_clone() {
        let opts = QueryOptions::default();
        let cloned = opts.clone();
        assert_eq!(opts.stale_time, cloned.stale_time);
        assert_eq!(opts.gc_time, cloned.gc_time);
    }

    #[test]
    fn test_query_options_debug() {
        let opts = QueryOptions::default();
        let debug = format!("{:?}", opts);
        assert!(debug.contains("stale_time"));
    }

    #[test]
    fn test_query_options_custom() {
        let opts = QueryOptions {
            stale_time: Duration::from_secs(60),
            gc_time: Duration::from_secs(600),
            retry: RetryConfig::new(5),
            enabled: false,
            refetch_on_window_focus: false,
            refetch_interval: Duration::from_secs(30),
            structural_sharing: false,
        };
        assert_eq!(opts.stale_time, Duration::from_secs(60));
        assert!(!opts.enabled);
        assert!(!opts.refetch_on_window_focus);
        assert!(!opts.structural_sharing);
        assert_eq!(opts.retry.max_retries, 5);
    }
}
