// src/sharing.rs
//! Structural sharing utilities.
//!
//! Structural sharing ensures that when new data is `PartialEq`-equal to the
//! previously cached data, the same `Arc` is retained. This allows consumers
//! (e.g., GPUI views) to cheaply detect "no change" via `Arc::ptr_eq` and
//! skip unnecessary re-renders.
//!
//! The `PartialEq` bound is enforced at the [`Query`](crate::Query) builder
//! level through the `.structural_sharing(true)` method, so the sharing
//! function stored in the query already captures the comparison logic.

use std::any::Any;
use std::sync::Arc;

/// Compare `old` and `new` by value; return `old` if equal, otherwise `new`.
///
/// This is the core primitive behind structural sharing. When the data has not
/// changed (according to `PartialEq`), we keep the old `Arc` so that downstream
/// pointer comparisons succeed.
///
/// **Note:** The `PartialEq` bound is guaranteed by the caller (the query
/// builder), so this function is safe to call only when `T: PartialEq`.
pub(crate) fn replace_equal_deep<T: PartialEq + 'static>(old: Arc<T>, new: Arc<T>) -> Arc<T> {
    if *old == *new {
        old
    } else {
        new
    }
}

/// Type-erased version used by [`QueryClient::set_query_data_shared`].
///
/// Attempts to downcast both arcs to `T` and compare. Returns the old arc
/// if they are equal, otherwise the new one wrapped as `dyn Any`.
#[allow(dead_code)]
pub(crate) fn replace_equal_deep_erased(
    old: Arc<dyn Any + Send + Sync>,
    new: Arc<dyn Any + Send + Sync>,
) -> Arc<dyn Any + Send + Sync> {
    // We cannot do type-erased PartialEq without knowing the type.
    // The actual sharing logic lives in the closure stored on the Query.
    // This function is kept as a reference/fallback.
    new
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_structural_sharing_keeps_same_arc_when_equal() {
        let old = Arc::new(vec![1, 2, 3]);
        let new = Arc::new(vec![1, 2, 3]);
        let result = replace_equal_deep(old.clone(), new);
        assert!(Arc::ptr_eq(&old, &result));
    }

    #[test]
    fn test_structural_sharing_replaces_when_not_equal() {
        let old = Arc::new(vec![1, 2, 3]);
        let new = Arc::new(vec![4, 5, 6]);
        let result = replace_equal_deep(old.clone(), new.clone());
        assert!(!Arc::ptr_eq(&old, &result));
        assert_eq!(*result, *new);
    }

    #[test]
    fn test_structural_sharing_with_strings() {
        let old = Arc::new("hello".to_string());
        let new = Arc::new("hello".to_string());
        let result = replace_equal_deep(old.clone(), new);
        assert!(Arc::ptr_eq(&old, &result));
    }

    #[test]
    fn test_structural_sharing_with_different_strings() {
        let old = Arc::new("hello".to_string());
        let new = Arc::new("world".to_string());
        let result = replace_equal_deep(old.clone(), new.clone());
        assert!(!Arc::ptr_eq(&old, &result));
        assert_eq!(*result, "world".to_string());
    }

    #[test]
    fn test_structural_sharing_with_nested_structs() {
        #[derive(Clone, PartialEq)]
        struct Data {
            id: u32,
            name: String,
            tags: Vec<String>,
        }

        let old = Arc::new(Data {
            id: 1,
            name: "test".to_string(),
            tags: vec!["a".to_string(), "b".to_string()],
        });
        let new = Arc::new(Data {
            id: 1,
            name: "test".to_string(),
            tags: vec!["a".to_string(), "b".to_string()],
        });
        let result = replace_equal_deep(old.clone(), new);
        assert!(Arc::ptr_eq(&old, &result));
    }

    #[test]
    fn test_structural_sharing_with_nested_structs_different() {
        #[derive(Clone, PartialEq)]
        struct Data {
            id: u32,
            name: String,
        }

        let old = Arc::new(Data {
            id: 1,
            name: "test".to_string(),
        });
        let new = Arc::new(Data {
            id: 2,
            name: "test".to_string(),
        });
        let result = replace_equal_deep(old.clone(), new.clone());
        assert!(!Arc::ptr_eq(&old, &result));
        assert_eq!(result.id, 2);
    }
}
