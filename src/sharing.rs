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

    #[derive(Clone, PartialEq, Debug)]
    struct TestData {
        id: u32,
        name: String,
    }

    #[test]
    fn test_replace_equal_deep_same() {
        let old = TestData {
            id: 1,
            name: "test".to_string(),
        };
        let new = old.clone();
        let result = replace_equal_deep(&old, &new);
        assert_eq!(result, old);
    }

    #[test]
    fn test_replace_equal_deep_different() {
        let old = TestData {
            id: 1,
            name: "test".to_string(),
        };
        let new = TestData {
            id: 2,
            name: "test2".to_string(),
        };
        let result = replace_equal_deep(&old, &new);
        assert_eq!(result, new);
    }
}
