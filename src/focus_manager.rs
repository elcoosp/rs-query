// src/focus_manager.rs
//! Focus manager for window focus refetching

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Manages window focus state and triggers refetches on focus gain.
pub struct FocusManager {
    focused: Arc<AtomicBool>,
}

impl FocusManager {
    pub fn new() -> Self {
        Self {
            focused: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn is_focused(&self) -> bool {
        self.focused.load(Ordering::Relaxed)
    }

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
