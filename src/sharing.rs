// src/sharing.rs
//! Structural sharing utilities

use std::any::Any;
use std::sync::Arc;

/// Type-erased structural sharing comparison.
pub(crate) fn replace_equal_deep_any(
    _old: Arc<dyn Any + Send + Sync>,
    new: Arc<dyn Any + Send + Sync>,
) -> Arc<dyn Any + Send + Sync> {
    new
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_equal_deep_any_returns_new() {
        let old: Arc<dyn Any + Send + Sync> = Arc::new("old".to_string());
        let new: Arc<dyn Any + Send + Sync> = Arc::new("new".to_string());
        let result = replace_equal_deep_any(old, new);
        let data = result.downcast_ref::<String>().unwrap();
        assert_eq!(data, "new");
    }

    #[test]
    fn test_replace_equal_deep_any_same_values() {
        let old: Arc<dyn Any + Send + Sync> = Arc::new("same".to_string());
        let new: Arc<dyn Any + Send + Sync> = Arc::new("same".to_string());
        let result = replace_equal_deep_any(old, new);
        let data = result.downcast_ref::<String>().unwrap();
        assert_eq!(data, "same");
    }

    #[test]
    fn test_replace_equal_deep_any_different_types() {
        let old: Arc<dyn Any + Send + Sync> = Arc::new(42i32);
        let new: Arc<dyn Any + Send + Sync> = Arc::new("string".to_string());
        let result = replace_equal_deep_any(old, new);
        let data = result.downcast_ref::<String>().unwrap();
        assert_eq!(data, "string");
    }
}
