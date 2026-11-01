// src/network.rs
//! Network-mode semantics and the [`OnlineManager`].
//!
//! Mirrors `TanStack` Query's `networkMode` option and `onlineManager`: queries
//! and mutations can be paused while the application is offline and resumed
//! when connectivity returns.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::watch;

/// How a query or mutation interacts with network connectivity.
///
/// * [`NetworkMode::Online`] (default): the operation only runs while online.
/// * [`NetworkMode::Always`]: the operation runs regardless of connectivity.
/// * [`NetworkMode::OfflineFirst`]: the first attempt runs even while offline,
///   but retries pause until connectivity returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NetworkMode {
    /// Only run while the application is online.
    #[default]
    Online,
    /// Run regardless of network connectivity.
    Always,
    /// Run the first attempt offline, but pause retries while offline.
    OfflineFirst,
}

/// Returns `true` when a fetch may be *started* under the given connectivity.
///
/// `TanStack` semantics: only `networkMode: 'online'` gates the start of a
/// fetch; `always` and `offlineFirst` may start while offline.
#[must_use]
pub fn can_fetch(mode: NetworkMode, online: bool) -> bool {
    match mode {
        NetworkMode::Online => online,
        NetworkMode::Always | NetworkMode::OfflineFirst => true,
    }
}

/// Returns `true` when a retry may *continue* under the given connectivity.
///
/// Retries of `offline_first` operations pause while offline (unlike the
/// initial attempt), and `online` mode is fully gated; `always` continues.
#[must_use]
pub fn can_retry(mode: NetworkMode, online: bool) -> bool {
    match mode {
        NetworkMode::Online | NetworkMode::OfflineFirst => online,
        NetworkMode::Always => true,
    }
}

/// Global connectivity state, mirroring `TanStack` Query's `onlineManager`.
///
/// Starts online by default. Owners may call [`OnlineManager::set_online`] from
/// connectivity callbacks; observers are notified only on changes.
#[derive(Clone, Debug)]
pub struct OnlineManager {
    online: Arc<AtomicBool>,
    tx: watch::Sender<bool>,
}

impl Default for OnlineManager {
    fn default() -> Self {
        Self::new()
    }
}

impl OnlineManager {
    /// Create an online manager that starts in the online state.
    #[must_use]
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(true);
        Self {
            online: Arc::new(AtomicBool::new(true)),
            tx,
        }
    }

    /// Returns the current connectivity state.
    #[must_use]
    pub fn is_online(&self) -> bool {
        self.online.load(Ordering::Relaxed)
    }

    /// Update the connectivity state; notifies subscribers only on change.
    pub fn set_online(&self, online: bool) {
        if self.is_online() != online {
            self.online.store(online, Ordering::Relaxed);
            let _ = self.tx.send(online);
        }
    }

    /// Subscribe to connectivity changes; the current value is delivered
    /// immediately through the returned receiver.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_mode_default_is_online() {
        assert_eq!(NetworkMode::default(), NetworkMode::Online);
    }

    #[test]
    fn test_can_fetch_online_mode() {
        assert!(can_fetch(NetworkMode::Online, true));
        assert!(!can_fetch(NetworkMode::Online, false));
    }

    #[test]
    fn test_can_fetch_always_mode() {
        assert!(can_fetch(NetworkMode::Always, true));
        assert!(can_fetch(NetworkMode::Always, false));
    }

    #[test]
    fn test_can_fetch_offline_first_mode() {
        assert!(can_fetch(NetworkMode::OfflineFirst, true));
        assert!(can_fetch(NetworkMode::OfflineFirst, false));
    }

    #[test]
    fn test_can_retry_online_mode() {
        assert!(can_retry(NetworkMode::Online, true));
        assert!(!can_retry(NetworkMode::Online, false));
    }

    #[test]
    fn test_can_retry_always_mode() {
        assert!(can_retry(NetworkMode::Always, true));
        assert!(can_retry(NetworkMode::Always, false));
    }

    #[test]
    fn test_can_retry_offline_first_mode_pauses_offline() {
        assert!(can_retry(NetworkMode::OfflineFirst, true));
        assert!(!can_retry(NetworkMode::OfflineFirst, false));
    }

    #[test]
    fn test_online_manager_starts_online() {
        let manager = OnlineManager::new();
        assert!(manager.is_online());
    }

    #[test]
    fn test_online_manager_default_impl() {
        let manager = OnlineManager::default();
        assert!(manager.is_online());
    }

    #[tokio::test]
    async fn test_online_manager_set_online_notifies_on_change() {
        let manager = OnlineManager::new();
        let mut rx = manager.subscribe();
        assert!(*rx.borrow_and_update());

        manager.set_online(false);
        assert!(!manager.is_online());
        rx.changed().await.expect("change should be observed");
        assert!(!*rx.borrow());

        manager.set_online(true);
        assert!(manager.is_online());
    }

    #[test]
    fn test_online_manager_set_same_value_is_noop() {
        let manager = OnlineManager::new();
        let mut rx = manager.subscribe();
        assert!(*rx.borrow_and_update());
        manager.set_online(true);
        assert!(!rx.has_changed().expect("has_changed should succeed"));
    }

    #[test]
    fn test_online_manager_clone_shares_state() {
        let manager = OnlineManager::new();
        let clone = manager.clone();
        manager.set_online(false);
        assert!(!clone.is_online());
    }

    #[test]
    fn test_online_manager_debug_contains_state() {
        let manager = OnlineManager::new();
        let debug = format!("{manager:?}");
        assert!(debug.contains("OnlineManager"));
    }
}
