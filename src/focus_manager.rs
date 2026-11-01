// src/focus_manager.rs
//! Focus manager for window focus refetching

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Manages window focus state and triggers refetches on focus gain.
pub struct FocusManager {
    focused: Arc<AtomicBool>,
}

impl FocusManager {
    /// Create a focus manager that starts focused.
    #[must_use]
    pub fn new() -> Self {
        Self {
            focused: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Whether the window is currently focused.
    #[must_use]
    pub fn is_focused(&self) -> bool {
        self.focused.load(Ordering::Relaxed)
    }

    /// Update the focus state.
    ///
    /// Notification of focus changes is the caller's responsibility (the
    /// manager itself only tracks the flag); refetch triggers read this value
    /// when the window regains focus.
    pub fn set_focused(&self, focused: bool) {
        let _was_focused = self.focused.swap(focused, Ordering::Relaxed);
    }
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for FocusManager {
    fn clone(&self) -> Self {
        Self {
            focused: Arc::clone(&self.focused),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_focus_manager_default_focused() {
        let fm = FocusManager::new();
        assert!(fm.is_focused());
    }

    #[test]
    fn test_focus_manager_set_unfocused() {
        let fm = FocusManager::new();
        fm.set_focused(false);
        assert!(!fm.is_focused());
    }

    #[test]
    fn test_focus_manager_set_focused_again() {
        let fm = FocusManager::new();
        fm.set_focused(false);
        fm.set_focused(true);
        assert!(fm.is_focused());
    }

    #[test]
    fn test_focus_manager_default_trait() {
        let fm = FocusManager::default();
        assert!(fm.is_focused());
    }

    #[test]
    fn test_focus_manager_clone() {
        let fm = FocusManager::new();
        let cloned = fm.clone();
        assert!(cloned.is_focused());
        fm.set_focused(false);
        assert!(!cloned.is_focused());
    }
}
