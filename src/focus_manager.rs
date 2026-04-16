//! Focus manager for detecting window/app focus changes

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Global focus manager that tracks application focus state.
#[derive(Clone)]
pub struct FocusManager {
    focused: Arc<AtomicBool>,
}

impl FocusManager {
    /// Create a new FocusManager, defaulting to focused.
    pub fn new() -> Self {
        Self {
            focused: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Set the current focus state.
    pub fn set_focused(&self, focused: bool) {
        self.focused.store(focused, Ordering::SeqCst);
    }

    /// Check if the application is currently focused.
    pub fn is_focused(&self) -> bool {
        self.focused.load(Ordering::SeqCst)
    }
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_focus_manager_default() {
        let fm = FocusManager::new();
        assert!(fm.is_focused());
    }

    #[test]
    fn test_focus_manager_set_focused() {
        let fm = FocusManager::new();
        fm.set_focused(false);
        assert!(!fm.is_focused());
        fm.set_focused(true);
        assert!(fm.is_focused());
    }

    #[test]
    fn test_focus_manager_clone() {
        let fm1 = FocusManager::new();
        let fm2 = fm1.clone();
        fm1.set_focused(false);
        assert!(!fm2.is_focused());
    }
}
