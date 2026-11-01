// src/notify_manager.rs
//! Notification batching, mirroring `TanStack` Query's `notifyManager`.
//!
//! Cache updates frequently fan out to many observers at once. `TanStack`
//! collapses the resulting notifications so downstream consumers see one
//! aggregated wake-up instead of a storm. In rs-query the same contract holds:
//! notifications queued inside [`NotifyManager::batch`] run exactly once, when
//! the outermost batch closes. Batches nest safely.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

type NotifyFn = Box<dyn FnOnce() + Send>;

/// Collapses notification callbacks emitted during a batch into one flush.
#[derive(Clone, Default)]
pub struct NotifyManager {
    queue: Arc<Mutex<VecDeque<NotifyFn>>>,
    batch_depth: Arc<AtomicUsize>,
}

impl std::fmt::Debug for NotifyManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotifyManager")
            .field("pending", &self.pending())
            .finish()
    }
}

impl NotifyManager {
    /// Create a manager with an empty notification queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a callback; it runs immediately outside any batch, and otherwise
    /// when the outermost enclosing batch closes.
    pub fn notify<F>(&self, callback: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(Box::new(callback));
        if self.batch_depth.load(Ordering::SeqCst) == 0 {
            self.flush();
        }
    }

    /// Run `body` inside a batch and flush queued notifications once.
    ///
    /// Returns whatever `body` returns. Nested batches only flush at the
    /// outermost level.
    pub fn batch<T>(&self, body: impl FnOnce() -> T) -> T {
        self.batch_depth.fetch_add(1, Ordering::SeqCst);
        let result = body();
        if self.batch_depth.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.flush();
        }
        result
    }

    /// Drain the queue, executing every pending callback in submission order.
    pub fn flush(&self) {
        let pending: Vec<NotifyFn> = {
            let mut queue = self
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            queue.drain(..).collect()
        };
        for callback in pending {
            callback();
        }
    }

    /// Number of callbacks currently queued (mainly for tests/debugging).
    #[must_use]
    pub fn pending(&self) -> usize {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_notify_without_batch_flushes_immediately() {
        let manager = NotifyManager::new();
        let count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        manager.notify(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(count.load(Ordering::Relaxed), 1);
        assert_eq!(manager.pending(), 0);
    }

    #[test]
    fn test_batch_defers_notifications() {
        let manager = NotifyManager::new();
        let count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        manager.batch(|| {
            manager.notify(move || {
                counter.fetch_add(1, Ordering::Relaxed);
            });
            assert_eq!(manager.pending(), 1);
        });
        assert_eq!(count.load(Ordering::Relaxed), 1);
        assert_eq!(manager.pending(), 0);
    }

    #[test]
    fn test_nested_batch_flushes_once_at_outermost() {
        let manager = NotifyManager::new();
        let count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        manager.batch(|| {
            manager.batch(|| {
                let inner = Arc::clone(&counter);
                manager.notify(move || {
                    inner.fetch_add(1, Ordering::Relaxed);
                });
            });
            // Inner batch closed but the notification must still be queued.
            assert_eq!(manager.pending(), 1);
            assert_eq!(count.load(Ordering::Relaxed), 0);
        });
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_batch_returns_body_result() {
        let manager = NotifyManager::new();
        let value = manager.batch(|| 21 * 2);
        assert_eq!(value, 42);
    }

    #[test]
    fn test_flush_preserves_submission_order() {
        let manager = NotifyManager::new();
        let order = Arc::new(Mutex::new(Vec::new()));
        for label in 0..3 {
            let sink = Arc::clone(&order);
            manager.notify(move || {
                sink.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(label);
            });
        }
        manager.flush();
        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn test_flush_on_empty_queue_is_noop() {
        let manager = NotifyManager::new();
        manager.flush();
        assert_eq!(manager.pending(), 0);
    }

    #[test]
    fn test_clone_shares_queue() {
        let manager = NotifyManager::new();
        let clone = manager.clone();
        let count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        manager.notify(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        clone.flush();
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_debug_reports_pending_count() {
        let manager = NotifyManager::new();
        assert!(format!("{manager:?}").contains("NotifyManager"));
        manager.batch(|| {
            manager.notify(|| {});
            manager.notify(|| {});
        });
        assert!(format!("{manager:?}").contains("pending: 0"));
    }
}
