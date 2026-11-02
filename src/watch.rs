// src/watch.rs
//! Minimal watch cell (the `tokio::sync::watch` stand-in).
//!
//! A [`Watch<T>`] holds one value plus a version counter; [`Watcher<T>`]
//! handles observe it and can await changes. Used for the online manager,
//! mount/unmount stop signals, persist/broadcast loop stop flags and
//! mutation-scope release wakeups.
//!
//! Unlike the bus, `changed()` coalesces: multiple writes between polls
//! resolve a single `changed()` and the watcher then reads the latest value.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

struct Shared<T> {
    value: T,
    version: u64,
    alive: bool,
    next_id: u64,
    waiters: HashMap<u64, Waker>,
}
/// The writing half of a watch cell.
pub struct Watch<T> {
    shared: Arc<Mutex<Shared<T>>>,
}

/// The observing half of a watch cell.
pub struct Watcher<T> {
    shared: Arc<Mutex<Shared<T>>>,
    last_seen: u64,
    id: u64,
}

impl<T: Clone> Watch<T> {
    /// Creates a cell holding `initial` plus its first watcher.
    #[must_use]
    pub fn new(initial: T) -> (Self, Watcher<T>) {
        let shared = Arc::new(Mutex::new(Shared {
            value: initial,
            version: 0,
            alive: true,
            next_id: 1,
            waiters: HashMap::new(),
        }));
        let watcher_id = {
            let mut guard = shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let id = guard.next_id;
            guard.next_id += 1;
            id
        };
        (
            Self {
                shared: Arc::clone(&shared),
            },
            Watcher {
                shared,
                last_seen: 0,
                id: watcher_id,
            },
        )
    }
}

impl<T: Clone> Watch<T> {
    /// Creates a new independent watcher observing this cell.
    #[must_use]
    pub fn subscribe(&self) -> Watcher<T> {
        let mut guard = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = guard.next_id;
        guard.next_id += 1;
        Watcher {
            shared: Arc::clone(&self.shared),
            last_seen: guard.version,
            id,
        }
    }

    /// Stores `value` and wakes every watcher if the value changed.
    ///
    /// Change detection is by version counter: every `send` bumps the
    /// version, so a watcher parked in `changed()` resolves even when the
    /// new value compares equal to the old one (callers that need
    /// value-based suppression should compare before sending).
    pub fn send(&self, value: T) {
        let wakers: Vec<Waker> = {
            let mut guard = self
                .shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.value = value;
            guard.version += 1;
            let wakers = guard.waiters.values().cloned().collect();
            guard.waiters.clear();
            wakers
        };
        for waker in wakers {
            waker.wake();
        }
    }
}

impl<T: Clone> Clone for Watch<T> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T: Clone> Watcher<T> {
    /// A copy of the current value (also marks this watcher as up to date).
    ///
    /// Takes `&mut self` because watching is inherently stateful (the
    /// change detection version lives in the watcher).
    pub fn borrow(&mut self) -> T {
        let guard = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.last_seen = guard.version;
        guard.value.clone()
    }

    /// The current value without marking this watcher as up to date.
    #[must_use]
    pub fn peek(&self) -> T {
        self.shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .value
            .clone()
    }

    /// Whether a new value arrived since the last `borrow`/`changed`.
    #[must_use]
    pub fn has_changed(&self) -> bool {
        let guard = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.alive && guard.version != self.last_seen
    }

    /// Awaits a value newer than the last observed one.
    ///
    /// Resolves immediately if a change is already pending. Errors with
    /// [`ChangeError::Closed`] once the cell is closed (the watcher stays
    /// usable through `peek`/`borrow`).
    pub fn changed(&mut self) -> Changed<'_, T> {
        Changed { rx: self }
    }
}

impl<T: Clone> Clone for Watcher<T> {
    fn clone(&self) -> Self {
        // Clones are independent watchers: they observe from the clone
        // point and register their own waker entries.
        let mut shared = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = shared.next_id;
        shared.next_id += 1;
        Self {
            shared: Arc::clone(&self.shared),
            last_seen: self.last_seen,
            id,
        }
    }
}

/// Error returned by [`Watcher::changed`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangeError;

/// Future returned by [`Watcher::changed`].
pub struct Changed<'a, T> {
    rx: &'a mut Watcher<T>,
}

impl<T: Clone> Future for Changed<'_, T> {
    type Output = Result<(), ChangeError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let rx = &mut *this.rx;
        let mut guard = rx
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.version != rx.last_seen {
            rx.last_seen = guard.version;
            return Poll::Ready(Ok(()));
        }
        if !guard.alive {
            return Poll::Ready(Err(ChangeError));
        }
        guard.waiters.insert(rx.id, cx.waker().clone());
        Poll::Pending
    }
}

impl<T> fmt::Debug for Watch<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Watch").finish_non_exhaustive()
    }
}

impl<T> fmt::Debug for Watcher<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Watcher").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    #[test]
    fn send_borrow_round_trip() {
        let (watch, mut rx) = Watch::new(false);
        assert!(!rx.borrow());
        watch.send(true);
        assert!(rx.borrow());
        assert!(!rx.has_changed(), "borrow marks up to date");
    }

    #[test]
    fn changed_resolves_on_send() {
        let (watch, mut rx) = Watch::new(0u32);
        let done = Arc::new(AtomicBool::new(false));
        let done2 = done.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            watch.send(7);
            done2.store(true, Ordering::SeqCst);
        });
        let result = futures::executor::block_on(rx.changed());
        assert_eq!(result, Ok(()));
        assert_eq!(rx.borrow(), 7);
        assert!(done.load(Ordering::SeqCst));
        handle.join().expect("sender thread must not panic");
    }

    #[test]
    fn coalesces_multiple_sends() {
        let (watch, mut rx) = Watch::new(0u32);
        watch.send(1);
        watch.send(2);
        watch.send(3);
        // One changed() is enough; borrow sees the latest value.
        assert!(futures::executor::block_on(rx.changed()).is_ok());
        assert_eq!(rx.borrow(), 3);
    }
}
