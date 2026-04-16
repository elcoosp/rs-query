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
