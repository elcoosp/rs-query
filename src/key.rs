//! Query key system for cache lookup and hierarchical invalidation

use std::hash::{Hash, Hasher};

/// A hierarchical query key for cache lookup and invalidation.
///
/// Keys can have static and dynamic segments for flexible matching:
///
/// ```rust,ignore
/// // Static key
/// QueryKey::new("users")
///
/// // With parameters
/// QueryKey::new("users").with("id", user_id)
///
/// // Hierarchical
/// QueryKey::new("users").segment("posts").with("id", post_id)
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryKey {
    segments: Vec<KeySegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum KeySegment {
    Static(String),
    Dynamic(String, String), // (name, value)
}

impl QueryKey {
    /// Create a new query key with a root segment
    pub fn new(root: impl Into<String>) -> Self {
        Self {
            segments: vec![KeySegment::Static(root.into())],
        }
    }

    /// Add a static segment
    pub fn segment(mut self, segment: impl Into<String>) -> Self {
        self.segments.push(KeySegment::Static(segment.into()));
        self
    }

    /// Add a dynamic segment (parameterized)
    pub fn with(mut self, name: impl Into<String>, value: impl ToString) -> Self {
        self.segments
            .push(KeySegment::Dynamic(name.into(), value.to_string()));
        self
    }

    /// Check if this key matches another for invalidation.
    /// Returns true if `pattern` is a prefix of or equal to `self`.
    pub fn matches(&self, pattern: &QueryKey) -> bool {
        if pattern.segments.len() > self.segments.len() {
            return false;
        }
        self.segments
            .iter()
            .zip(pattern.segments.iter())
            .all(|(a, b)| a == b)
    }

    /// Get cache key string for HashMap lookup
    pub fn cache_key(&self) -> String {
        self.segments
            .iter()
            .map(|s| match s {
                KeySegment::Static(v) => v.clone(),
                KeySegment::Dynamic(k, v) => format!("{}={}", k, v),
            })
            .collect::<Vec<_>>()
            .join("::")
    }
}

impl Hash for QueryKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.cache_key().hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_key() {
        let key = QueryKey::new("users");
        assert_eq!(key.cache_key(), "users");
    }

    #[test]
    fn test_key_with_params() {
        let key = QueryKey::new("users").with("id", 123);
        assert_eq!(key.cache_key(), "users::id=123");
    }

    #[test]
    fn test_hierarchical_key() {
        let key = QueryKey::new("users").segment("posts").with("id", 456);
        assert_eq!(key.cache_key(), "users::posts::id=456");
    }

    #[test]
    fn test_key_matching() {
        let full = QueryKey::new("users").with("id", 123);
        let pattern = QueryKey::new("users");

        assert!(full.matches(&pattern));
        assert!(!pattern.matches(&full));
    }
}
