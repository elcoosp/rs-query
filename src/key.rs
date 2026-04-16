//! Query key system for cache lookup and hierarchical invalidation

use std::hash::{Hash, Hasher};

/// A hierarchical query key for cache lookup and invalidation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryKey {
    segments: Vec<KeySegment>,
    /// Precomputed cache key string to avoid repeated allocations
    cached_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum KeySegment {
    Static(String),
    Dynamic(String, String), // (name, value)
}

impl QueryKey {
    /// Create a new query key with a root segment
    pub fn new(root: impl Into<String>) -> Self {
        let segments = vec![KeySegment::Static(root.into())];
        let cached_key = Self::compute_key(&segments);
        Self {
            segments,
            cached_key,
        }
    }

    /// Add a static segment
    pub fn segment(mut self, segment: impl Into<String>) -> Self {
        self.segments.push(KeySegment::Static(segment.into()));
        self.cached_key = Self::compute_key(&self.segments);
        self
    }

    /// Add a dynamic segment (parameterized)
    pub fn with(mut self, name: impl Into<String>, value: impl ToString) -> Self {
        self.segments
            .push(KeySegment::Dynamic(name.into(), value.to_string()));
        self.cached_key = Self::compute_key(&self.segments);
        self
    }

    /// Check if this key matches another for invalidation.
    pub fn matches(&self, pattern: &QueryKey) -> bool {
        if pattern.segments.len() > self.segments.len() {
            return false;
        }
        self.segments
            .iter()
            .zip(pattern.segments.iter())
            .all(|(a, b)| a == b)
    }

    /// Get the cached cache key string (zero‑cost after construction).
    pub fn cache_key(&self) -> &str {
        &self.cached_key
    }

    fn compute_key(segments: &[KeySegment]) -> String {
        segments
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
        self.cached_key.hash(state);
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

    #[test]
    fn test_key_caching() {
        let key = QueryKey::new("users").with("id", 42);
        let s1 = key.cache_key();
        let s2 = key.cache_key();
        // Same pointer equality because it's the same &str from the same field.
        assert_eq!(s1 as *const str, s2 as *const str);
    }
}
