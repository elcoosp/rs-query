// src/options.rs
//! Query options configuration, mirroring `TanStack` Query's option surface.

use crate::network::NetworkMode;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// `TanStack` Query's default `gcTime` (5 minutes).
pub const DEFAULT_GC_TIME: Duration = Duration::from_secs(300);

/// `TanStack` Query's maximum retry backoff (30 seconds).
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

/// `TanStack` Query's base retry delay (1 second).
pub const BASE_RETRY_DELAY: Duration = Duration::from_millis(1000);

/// Configuration for automatic retry behaviour, aligned with `TanStack`
/// Query defaults (3 retries, 1 s exponential backoff capped at 30 s,
/// no jitter).
#[derive(Clone, Debug, PartialEq)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (not counting the initial request).
    pub max_retries: u32,
    /// Base delay between retries.
    pub base_delay: Duration,
    /// Whether to use exponential backoff.
    pub exponential_backoff: bool,
    /// Upper bound for a computed delay (`TanStack` caps at 30 s).
    pub max_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: BASE_RETRY_DELAY,
            exponential_backoff: true,
            max_delay: MAX_RETRY_DELAY,
        }
    }
}

impl RetryConfig {
    /// Create a new retry configuration.
    #[must_use]
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries,
            base_delay: BASE_RETRY_DELAY,
            exponential_backoff: true,
            max_delay: MAX_RETRY_DELAY,
        }
    }

    /// Set the base delay between retries.
    #[must_use]
    pub fn base_delay(mut self, delay: Duration) -> Self {
        self.base_delay = delay;
        self
    }

    /// Enable or disable exponential backoff.
    #[must_use]
    pub fn exponential_backoff(mut self, enabled: bool) -> Self {
        self.exponential_backoff = enabled;
        self
    }

    /// Set the maximum delay cap.
    #[must_use]
    pub fn max_delay(mut self, max: Duration) -> Self {
        self.max_delay = max;
        self
    }

    /// Calculate the delay for a given retry attempt (0-indexed), applying
    /// exponential growth and the maximum-delay cap.
    #[must_use]
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        if self.exponential_backoff {
            let multiplier = 2u32.saturating_pow(attempt);
            let delay = self.base_delay.saturating_mul(multiplier);
            delay.min(self.max_delay)
        } else {
            self.base_delay.min(self.max_delay)
        }
    }

    /// Check if a retry should be attempted.
    #[must_use]
    pub fn should_retry(&self, attempt: u32) -> bool {
        attempt < self.max_retries
    }
}

/// Shareable function form of [`PlaceholderData::Function`]: computes the
/// placeholder from the previous data (if any).
pub type PlaceholderDataFn<T> = Arc<dyn Fn(Option<&T>) -> T + Send + Sync>;

/// Placeholder data configuration.
#[derive(Clone)]
pub enum PlaceholderData<T: Clone> {
    /// Static placeholder data.
    Value(T),
    /// Function that computes placeholder data from previous data.
    Function(PlaceholderDataFn<T>),
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
    #[must_use]
    pub fn value(data: T) -> Self {
        Self::Value(data)
    }

    /// Create a function-based placeholder.
    #[must_use]
    pub fn function<F>(f: F) -> Self
    where
        F: Fn(Option<&T>) -> T + Send + Sync + 'static,
    {
        Self::Function(Arc::new(f))
    }

    /// `TanStack` `keepPreviousData`: show the previous data (if any) as the
    /// placeholder; resolves to `None` otherwise.
    #[must_use]
    pub fn keep_previous() -> Self
    where
        T: Default,
    {
        Self::function(|previous| previous.cloned().unwrap_or_default())
    }

    /// Resolve the placeholder data.
    #[must_use]
    pub fn resolve(&self, previous: Option<&T>) -> T {
        match self {
            PlaceholderData::Value(v) => v.clone(),
            PlaceholderData::Function(f) => f(previous),
        }
    }
}

/// When a refetch trigger (mount, focus, reconnect) may actually refetch,
/// mirroring `TanStack`'s `boolean | 'always'` option shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RefetchTrigger {
    /// Refetch only when the data is stale.
    #[default]
    Enabled,
    /// Never refetch from this trigger.
    Disabled,
    /// Refetch regardless of staleness.
    Always,
}

impl RefetchTrigger {
    /// Whether this trigger would refetch data with the given staleness.
    #[must_use]
    pub fn allows_refetch(&self, is_stale: bool) -> bool {
        match self {
            RefetchTrigger::Enabled => is_stale,
            RefetchTrigger::Disabled => false,
            RefetchTrigger::Always => true,
        }
    }

    /// Whether the trigger is enabled at all (ignoring staleness).
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        !matches!(self, RefetchTrigger::Disabled)
    }
}

/// Options for query behaviour.
#[derive(Clone)]
pub struct QueryOptions {
    /// How long data is considered fresh before becoming stale.
    pub stale_time: Duration,
    /// `TanStack` `staleTime: 'static'`: data never becomes stale by time.
    pub stale_time_static: bool,
    /// How long unused cache entries are kept before garbage collection.
    pub gc_time: Duration,
    /// Retry configuration.
    pub retry: RetryConfig,
    /// Whether the query is enabled (combine with `enabled_fn`).
    pub enabled: bool,
    /// Function form of `enabled`; the query is enabled only while it returns
    /// `true` (`TanStack` `enabled: () => bool`).
    pub enabled_fn: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// Whether to refetch when the window regains focus.
    pub refetch_on_window_focus: RefetchTrigger,
    /// Whether to refetch when connectivity is restored.
    pub refetch_on_reconnect: RefetchTrigger,
    /// Whether to refetch when a new observer subscribes.
    pub refetch_on_mount: RefetchTrigger,
    /// Interval for automatic background refetching.
    pub refetch_interval: Duration,
    /// Allow interval refetching while the window is unfocused.
    pub refetch_interval_in_background: bool,
    /// Whether to apply structural sharing when updating cache.
    pub structural_sharing: bool,
    /// Network interaction mode.
    pub network_mode: NetworkMode,
    /// Allow a failed query to retry when a new observer subscribes.
    pub retry_on_mount: bool,
    /// Surface fetch errors by throwing (observable in `Result`-like flows).
    pub throw_on_error: bool,
    /// Restrict observer notifications to these property names.
    pub notify_on_change_props: Option<Vec<String>>,
    /// Free-form metadata attached to the query (deterministic ordering).
    pub meta: BTreeMap<String, String>,
}

impl std::fmt::Debug for QueryOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("QueryOptions");
        debug
            .field("stale_time", &self.stale_time)
            .field("stale_time_static", &self.stale_time_static)
            .field("gc_time", &self.gc_time)
            .field("retry", &self.retry)
            .field("enabled", &self.enabled)
            .field(
                "enabled_fn",
                if self.enabled_fn.is_some() {
                    &"<fn>"
                } else {
                    &"None"
                },
            )
            .field("refetch_on_window_focus", &self.refetch_on_window_focus)
            .field("refetch_on_reconnect", &self.refetch_on_reconnect)
            .field("refetch_on_mount", &self.refetch_on_mount)
            .field("refetch_interval", &self.refetch_interval)
            .field(
                "refetch_interval_in_background",
                &self.refetch_interval_in_background,
            )
            .field("structural_sharing", &self.structural_sharing)
            .field("network_mode", &self.network_mode)
            .field("retry_on_mount", &self.retry_on_mount)
            .field("throw_on_error", &self.throw_on_error)
            .field("notify_on_change_props", &self.notify_on_change_props)
            .field("meta", &self.meta);
        debug.finish()
    }
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            stale_time: Duration::ZERO,
            stale_time_static: false,
            gc_time: DEFAULT_GC_TIME,
            retry: RetryConfig::default(),
            enabled: true,
            enabled_fn: None,
            refetch_on_window_focus: RefetchTrigger::Enabled,
            refetch_on_reconnect: RefetchTrigger::Enabled,
            refetch_on_mount: RefetchTrigger::Enabled,
            refetch_interval: Duration::ZERO,
            refetch_interval_in_background: false,
            structural_sharing: true,
            network_mode: NetworkMode::Online,
            retry_on_mount: true,
            throw_on_error: false,
            notify_on_change_props: None,
            meta: BTreeMap::new(),
        }
    }
}

impl QueryOptions {
    /// Options with `TanStack` defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the stale time.
    #[must_use]
    pub fn stale_time(mut self, stale_time: Duration) -> Self {
        self.stale_time = stale_time;
        self.stale_time_static = false;
        self
    }

    /// Mark data as never stale by time (`TanStack` `staleTime: 'static'`).
    #[must_use]
    pub fn stale_time_static(mut self) -> Self {
        self.stale_time_static = true;
        self
    }

    /// Set the gc time.
    #[must_use]
    pub fn gc_time(mut self, gc_time: Duration) -> Self {
        self.gc_time = gc_time;
        self
    }

    /// Set the retry configuration.
    #[must_use]
    pub fn retry(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }

    /// Set static enabledness.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set the function form of `enabled`.
    #[must_use]
    pub fn enabled_fn<F>(mut self, enabled_fn: F) -> Self
    where
        F: Fn() -> bool + Send + Sync + 'static,
    {
        self.enabled_fn = Some(Arc::new(enabled_fn));
        self
    }

    /// Resolve effective enabledness (static flag AND function form).
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled && self.enabled_fn.as_ref().is_none_or(|f| f())
    }

    /// Set the focus-refetch trigger.
    #[must_use]
    pub fn refetch_on_window_focus(mut self, trigger: RefetchTrigger) -> Self {
        self.refetch_on_window_focus = trigger;
        self
    }

    /// Set the reconnect-refetch trigger.
    #[must_use]
    pub fn refetch_on_reconnect(mut self, trigger: RefetchTrigger) -> Self {
        self.refetch_on_reconnect = trigger;
        self
    }

    /// Set the mount-refetch trigger.
    #[must_use]
    pub fn refetch_on_mount(mut self, trigger: RefetchTrigger) -> Self {
        self.refetch_on_mount = trigger;
        self
    }

    /// Set the refetch interval.
    #[must_use]
    pub fn refetch_interval(mut self, interval: Duration) -> Self {
        self.refetch_interval = interval;
        self
    }

    /// Allow interval refetches while unfocused.
    #[must_use]
    pub fn refetch_interval_in_background(mut self, allow: bool) -> Self {
        self.refetch_interval_in_background = allow;
        self
    }

    /// Set structural sharing.
    #[must_use]
    pub fn structural_sharing(mut self, enabled: bool) -> Self {
        self.structural_sharing = enabled;
        self
    }

    /// Set the network mode.
    #[must_use]
    pub fn network_mode(mut self, mode: NetworkMode) -> Self {
        self.network_mode = mode;
        self
    }

    /// Set retry-on-mount.
    #[must_use]
    pub fn retry_on_mount(mut self, retry_on_mount: bool) -> Self {
        self.retry_on_mount = retry_on_mount;
        self
    }

    /// Set throw-on-error.
    #[must_use]
    pub fn throw_on_error(mut self, throw_on_error: bool) -> Self {
        self.throw_on_error = throw_on_error;
        self
    }

    /// Restrict notifications to the given property names.
    #[must_use]
    pub fn notify_on_change_props(mut self, props: Vec<String>) -> Self {
        self.notify_on_change_props = Some(props);
        self
    }

    /// Attach a metadata entry.
    #[must_use]
    pub fn meta(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.meta.insert(key.into(), value.into());
        self
    }

    /// Whether data fetched at `fetched_at` is stale by time under these
    /// options (`TanStack` `isStaleByTime`).
    #[must_use]
    pub fn is_stale_by_time(&self, fetched_at: Instant) -> bool {
        if self.stale_time_static {
            return false;
        }
        if self.stale_time.is_zero() {
            return false;
        }
        fetched_at.elapsed() > self.stale_time
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_config_new() {
        let config = RetryConfig::new(5);
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.base_delay, BASE_RETRY_DELAY);
        assert!(config.exponential_backoff);
    }

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.base_delay, BASE_RETRY_DELAY);
        assert_eq!(config.max_delay, MAX_RETRY_DELAY);
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
    fn test_retry_config_max_delay_builder() {
        let config = RetryConfig::new(2).max_delay(Duration::from_secs(5));
        assert_eq!(config.max_delay, Duration::from_secs(5));
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
    }

    #[test]
    fn test_retry_config_delay_caps_at_max() {
        let config = RetryConfig::new(10);
        let delay = config.delay_for_attempt(6);
        assert_eq!(delay, MAX_RETRY_DELAY);
    }

    #[test]
    fn test_retry_config_delay_cap_applies_to_linear() {
        let config = RetryConfig::new(10)
            .base_delay(Duration::from_secs(60))
            .exponential_backoff(false)
            .max_delay(Duration::from_secs(1));
        assert_eq!(config.delay_for_attempt(0), Duration::from_secs(1));
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
        let debug = format!("{config:?}");
        assert!(debug.contains("max_retries"));
        assert!(debug.contains('3'));
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
            prev.map_or_else(|| "loading...".to_string(), |p| format!("{p} (loading...)"))
        });

        assert_eq!(ph.resolve(Some(&"old".to_string())), "old (loading...)");
        assert_eq!(ph.resolve(None), "loading...");
    }

    #[test]
    fn test_placeholder_data_keep_previous() {
        let ph = PlaceholderData::<String>::keep_previous();
        assert_eq!(ph.resolve(Some(&"kept".to_string())), "kept");
        assert_eq!(ph.resolve(None), String::new());
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
        let debug = format!("{ph:?}");
        assert!(debug.contains("Value"));
    }

    #[test]
    fn test_placeholder_data_debug_function() {
        let ph: PlaceholderData<String> = PlaceholderData::function(|_| "computed".to_string());
        let debug = format!("{ph:?}");
        assert!(debug.contains("Function"));
    }

    #[test]
    fn test_refetch_trigger_allows_refetch() {
        assert!(RefetchTrigger::Enabled.allows_refetch(true));
        assert!(!RefetchTrigger::Enabled.allows_refetch(false));
        assert!(!RefetchTrigger::Disabled.allows_refetch(true));
        assert!(RefetchTrigger::Always.allows_refetch(false));
    }

    #[test]
    fn test_refetch_trigger_is_enabled() {
        assert!(RefetchTrigger::Enabled.is_enabled());
        assert!(RefetchTrigger::Always.is_enabled());
        assert!(!RefetchTrigger::Disabled.is_enabled());
        assert_eq!(RefetchTrigger::default(), RefetchTrigger::Enabled);
    }

    #[test]
    fn test_query_options_default() {
        let opts = QueryOptions::default();
        assert_eq!(opts.stale_time, Duration::ZERO);
        assert!(!opts.stale_time_static);
        assert_eq!(opts.gc_time, DEFAULT_GC_TIME);
        assert_eq!(opts.retry.max_retries, 3);
        assert!(opts.enabled);
        assert_eq!(opts.refetch_on_window_focus, RefetchTrigger::Enabled);
        assert_eq!(opts.refetch_on_reconnect, RefetchTrigger::Enabled);
        assert_eq!(opts.refetch_on_mount, RefetchTrigger::Enabled);
        assert_eq!(opts.refetch_interval, Duration::ZERO);
        assert!(!opts.refetch_interval_in_background);
        assert!(opts.structural_sharing);
        assert_eq!(opts.network_mode, NetworkMode::Online);
        assert!(opts.retry_on_mount);
        assert!(!opts.throw_on_error);
        assert!(opts.notify_on_change_props.is_none());
        assert!(opts.meta.is_empty());
    }

    #[test]
    fn test_query_options_new_matches_default() {
        let opts = QueryOptions::new();
        assert_eq!(opts.stale_time, QueryOptions::default().stale_time);
    }

    #[test]
    fn test_query_options_builder_chain() {
        let opts = QueryOptions::new()
            .stale_time(Duration::from_secs(60))
            .gc_time(Duration::from_secs(600))
            .enabled(true)
            .refetch_on_window_focus(RefetchTrigger::Always)
            .refetch_on_reconnect(RefetchTrigger::Disabled)
            .refetch_on_mount(RefetchTrigger::Always)
            .refetch_interval(Duration::from_secs(30))
            .refetch_interval_in_background(true)
            .structural_sharing(false)
            .network_mode(NetworkMode::Always)
            .retry_on_mount(false)
            .throw_on_error(true)
            .notify_on_change_props(vec!["data".to_string()])
            .meta("source", "test");
        assert_eq!(opts.stale_time, Duration::from_secs(60));
        assert_eq!(opts.gc_time, Duration::from_secs(600));
        assert_eq!(opts.refetch_on_window_focus, RefetchTrigger::Always);
        assert_eq!(opts.refetch_on_reconnect, RefetchTrigger::Disabled);
        assert_eq!(opts.refetch_on_mount, RefetchTrigger::Always);
        assert_eq!(opts.refetch_interval, Duration::from_secs(30));
        assert!(opts.refetch_interval_in_background);
        assert!(!opts.structural_sharing);
        assert_eq!(opts.network_mode, NetworkMode::Always);
        assert!(!opts.retry_on_mount);
        assert!(opts.throw_on_error);
        assert_eq!(opts.notify_on_change_props, Some(vec!["data".to_string()]));
        assert_eq!(opts.meta.get("source"), Some(&"test".to_string()));
    }

    #[test]
    fn test_query_options_stale_time_static_flag() {
        let opts = QueryOptions::new().stale_time_static();
        assert!(opts.stale_time_static);
        // The static builder resets any duration set beforehand.
        let opts = QueryOptions::new()
            .stale_time(Duration::from_secs(1))
            .stale_time_static();
        assert!(opts.stale_time_static);
    }

    #[test]
    fn test_query_options_is_stale_by_time() {
        let opts = QueryOptions::new().stale_time(Duration::from_millis(10));
        let now = Instant::now();
        assert!(!opts.is_stale_by_time(now));
        std::thread::sleep(Duration::from_millis(20));
        assert!(opts.is_stale_by_time(now));
    }

    #[test]
    fn test_query_options_is_stale_by_time_zero_means_fresh() {
        let opts = QueryOptions::new();
        let now = Instant::now();
        assert!(!opts.is_stale_by_time(now));
        std::thread::sleep(Duration::from_millis(2));
        assert!(!opts.is_stale_by_time(now));
    }

    #[test]
    fn test_query_options_is_stale_by_time_static() {
        let opts = QueryOptions::new().stale_time_static();
        let past = Instant::now()
            .checked_sub(Duration::from_secs(3600))
            .expect("one hour before now should be representable");
        assert!(!opts.is_stale_by_time(past));
    }

    #[test]
    fn test_query_options_enabled_fn() {
        let flag = std::sync::atomic::AtomicBool::new(true);
        let opts =
            QueryOptions::new().enabled_fn(move || flag.load(std::sync::atomic::Ordering::Relaxed));
        assert!(opts.is_enabled());

        let flag2 = std::sync::atomic::AtomicBool::new(false);
        let disabled = QueryOptions::new()
            .enabled_fn(move || flag2.load(std::sync::atomic::Ordering::Relaxed));
        assert!(!disabled.is_enabled());
    }

    #[test]
    fn test_query_options_enabled_static_false_wins() {
        let opts = QueryOptions::new().enabled(false).enabled_fn(|| true);
        assert!(!opts.is_enabled());
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
        let debug = format!("{opts:?}");
        assert!(debug.contains("stale_time"));
    }

    #[test]
    fn test_query_options_debug_with_fn() {
        let opts = QueryOptions::new().enabled_fn(|| true);
        let debug = format!("{opts:?}");
        assert!(debug.contains("<fn>"));
    }
}
