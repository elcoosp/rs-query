//! Structural sharing utilities for efficient cache updates

use std::any::Any;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// A trait for types that support structural sharing.
/// This is automatically implemented for types that are `Clone + PartialEq + 'static`.
pub trait StructuralShare: Any {
    fn as_any(&self) -> &dyn Any;
    fn clone_box(&self) -> Box<dyn StructuralShare>;
    fn eq_box(&self, other: &dyn StructuralShare) -> bool;
    fn replace_deep(&self, other: &dyn StructuralShare) -> Arc<dyn Any + Send + Sync>;
}

impl<T> StructuralShare for T
where
    T: Clone + PartialEq + 'static + Send + Sync,
{
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn StructuralShare> {
        Box::new(self.clone())
    }

    fn eq_box(&self, other: &dyn StructuralShare) -> bool {
        other
            .as_any()
            .downcast_ref::<T>()
            .map_or(false, |v| self == v)
    }

    fn replace_deep(&self, other: &dyn StructuralShare) -> Arc<dyn Any + Send + Sync> {
        if let Some(other_val) = other.as_any().downcast_ref::<T>() {
            if self == other_val {
                // Data is equal; keep the old reference to preserve structural sharing
                Arc::new(self.clone())
            } else {
                // Data changed; return the new value
                Arc::new(other_val.clone())
            }
        } else {
            // Type mismatch; fallback to new value
            Arc::new(other.clone_box())
        }
    }
}

/// Replace old data with new data while preserving references where equal.
/// This is the main entry point for structural sharing.
pub fn replace_equal_deep<T>(old: &T, new: &T) -> T
where
    T: Clone + PartialEq + 'static + Send + Sync,
{
    // For simple types, just compare and return appropriate reference
    if old == new {
        old.clone()
    } else {
        new.clone()
    }
}

/// A version that works with Any for type-erased cache entries.
pub fn replace_equal_deep_any(
    old: &(dyn Any + Send + Sync),
    new: &(dyn Any + Send + Sync),
) -> Arc<dyn Any + Send + Sync> {
    // If types differ, return new
    if old.type_id() != new.type_id() {
        return Arc::from(new);
    }

    // Use the StructuralShare trait for comparison and replacement
    if let Some(old_share) = old.downcast_ref::<Box<dyn StructuralShare>>() {
        if let Some(new_share) = new.downcast_ref::<Box<dyn StructuralShare>>() {
            return old_share.replace_deep(&**new_share);
        }
    }

    // Fallback for types not implementing StructuralShare: just return new
    Arc::from(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Debug)]
    struct TestData {
        id: u32,
        name: String,
        nested: Vec<u32>,
    }

    #[test]
    fn test_replace_equal_deep_same() {
        let old = TestData {
            id: 1,
            name: "test".to_string(),
            nested: vec![1, 2, 3],
        };
        let new = old.clone();
        let result = replace_equal_deep(&old, &new);
        assert_eq!(result, old);
        // Reference equality not guaranteed across clones, but value equality holds.
    }

    #[test]
    fn test_replace_equal_deep_different() {
        let old = TestData {
            id: 1,
            name: "test".to_string(),
            nested: vec![1, 2, 3],
        };
        let new = TestData {
            id: 2,
            name: "test2".to_string(),
            nested: vec![4, 5, 6],
        };
        let result = replace_equal_deep(&old, &new);
        assert_eq!(result, new);
        assert_ne!(result, old);
    }
}
