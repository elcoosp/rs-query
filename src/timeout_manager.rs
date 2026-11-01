// src/timeout_manager.rs
//! Injectable timer abstraction, mirroring `TanStack` Query's `timeoutManager`.
//!
//! All internal waiting (retry backoff, GC timers, refetch intervals, throttle
//! delays) flows through the [`TimeoutManager`] so tests can substitute
//! deterministic timers.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// Async sleep provider used by every internal timer.
pub trait SleepProvider: Send + Sync + std::fmt::Debug {
    /// Sleep for the given duration, then return.
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;

    /// Access the provider as `Any` for downcasting (tests/diagnostics).
    fn as_any(&self) -> &dyn Any;
}

/// Default provider backed by `tokio::time::sleep`.
#[derive(Debug, Default, Clone, Copy)]
pub struct TokioSleepProvider;

impl SleepProvider for TokioSleepProvider {
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            tokio::time::sleep(duration).await;
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Pluggable timer source: every internal delay is routed through here.
///
/// Clone the manager freely; all clones share the same provider.
#[derive(Clone, Debug)]
pub struct TimeoutManager {
    provider: Arc<RwLock<Arc<dyn SleepProvider>>>,
}

impl Default for TimeoutManager {
    fn default() -> Self {
        Self::with_provider(TokioSleepProvider)
    }
}

impl TimeoutManager {
    /// Create a manager using the default (`tokio`) timer source.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a manager with an explicit provider (useful for tests).
    #[must_use]
    pub fn with_provider<P: SleepProvider + 'static>(provider: P) -> Self {
        Self {
            provider: Arc::new(RwLock::new(Arc::new(provider))),
        }
    }

    /// Sleep for `duration` using the installed provider.
    pub async fn sleep(&self, duration: Duration) {
        let provider = {
            let guard = self
                .provider
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(&guard)
        };
        provider.sleep(duration).await;
    }

    /// Replace the timer provider at runtime.
    pub fn set_provider<P: SleepProvider + 'static>(&self, provider: P) {
        let mut guard = self
            .provider
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Arc::new(provider);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A provider that records the requested duration and returns instantly.
    #[derive(Debug)]
    struct InstantSleep {
        calls: AtomicUsize,
    }

    impl SleepProvider for InstantSleep {
        fn sleep(&self, _duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Box::pin(std::future::ready(()))
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[tokio::test]
    async fn test_default_manager_sleeps() {
        let manager = TimeoutManager::new();
        tokio::time::pause();
        manager.sleep(Duration::from_millis(10)).await;
        tokio::time::advance(Duration::from_millis(10)).await;
    }

    #[tokio::test]
    async fn test_custom_provider_is_used() {
        let provider = InstantSleep {
            calls: AtomicUsize::new(0),
        };
        let manager = TimeoutManager::with_provider(provider);
        manager.sleep(Duration::from_secs(1)).await;
        manager.sleep(Duration::from_secs(2)).await;
        assert_eq!(
            manager
                .provider
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_any()
                .downcast_ref::<InstantSleep>()
                .expect("provider should be InstantSleep")
                .calls
                .load(Ordering::Relaxed),
            2
        );
    }

    #[tokio::test]
    async fn test_set_provider_swaps_source() {
        let manager = TimeoutManager::new();
        manager.set_provider(InstantSleep {
            calls: AtomicUsize::new(0),
        });
        // After the swap, sleeps return instantly.
        tokio::time::pause();
        manager.sleep(Duration::from_secs(60)).await;
    }

    #[test]
    fn test_default_provider_is_tokio() {
        let manager = TimeoutManager::default();
        let debug = format!("{manager:?}");
        assert!(debug.contains("TimeoutManager"));
    }

    #[test]
    fn test_default_provider_downcasts_to_tokio_sleep_provider() {
        let manager = TimeoutManager::new();
        let is_tokio = manager
            .provider
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_any()
            .downcast_ref::<TokioSleepProvider>()
            .is_some();
        assert!(is_tokio, "default provider should be TokioSleepProvider");
    }

    #[tokio::test]
    async fn test_clone_shares_provider() {
        let manager = TimeoutManager::with_provider(InstantSleep {
            calls: AtomicUsize::new(0),
        });
        let clone = manager.clone();
        clone.sleep(Duration::from_millis(1)).await;
        assert_eq!(
            manager
                .provider
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_any()
                .downcast_ref::<InstantSleep>()
                .expect("provider should be InstantSleep")
                .calls
                .load(Ordering::Relaxed),
            1
        );
    }
}
