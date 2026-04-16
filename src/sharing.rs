//! Structural sharing utilities for efficient cache updates

use std::any::Any;
use std::sync::Arc;

/// Replace old data with new data while attempting to preserve references.
/// Currently returns the new data if types differ; otherwise returns new.
/// A more sophisticated deep equality check can be added later.
pub fn replace_equal_deep_any(
    old: Arc<dyn Any + Send + Sync>,
    new: Arc<dyn Any + Send + Sync>,
) -> Arc<dyn Any + Send + Sync> {
    // If types differ, we cannot compare; return the new data.
    if old.type_id() != new.type_id() {
        return new;
    }
    // For now, simply return the new data. Future enhancement: compare and return old if equal.
    new
}

/// A version for concrete types that implement `Clone + PartialEq`.
pub fn replace_equal_deep<T>(old: &T, new: &T) -> T
where
    T: Clone + PartialEq,
{
    if old == new {
        old.clone()
    } else {
        new.clone()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_replace_equal_deep_same() {
        let old = "hello".to_string();
        let new = "hello".to_string();
        let result = replace_equal_deep(&old, &new);
        assert_eq!(result, old);
    }

    #[test]
    fn test_replace_equal_deep_different() {
        let old = "hello".to_string();
        let new = "world".to_string();
        let result = replace_equal_deep(&old, &new);
        assert_eq!(result, new);
    }

    #[test]
    fn test_replace_equal_deep_any() {
        let old = Arc::new("hello".to_string()) as Arc<dyn Any + Send + Sync>;
        let new = Arc::new("hello".to_string()) as Arc<dyn Any + Send + Sync>;
        let result = replace_equal_deep_any(old.clone(), new.clone());
        let result_str = result.downcast_ref::<String>().unwrap();
        assert_eq!(result_str, "hello");
    }
}
