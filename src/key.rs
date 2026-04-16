// src/key.rs
//! QueryKey - hierarchical key system for cache lookups and invalidation

use std::fmt;

/// A hierarchical key for identifying queries in the cache.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct QueryKey {
    segments: Vec<String>,
    /// Pre-computed cache key string for efficient lookups.
    cache_key: String,
}

impl QueryKey {
    /// Create a new query key with an initial segment.
    pub fn new(segment: impl Into<String>) -> Self {
        let segment = segment.into();
        Self {
            segments: vec![segment.clone()],
            cache_key: segment,
        }
    }

    /// Add a parameter segment (e.g., `"id=42"`).
    pub fn with(mut self, label: impl Into<String>, value: impl std::fmt::Display) -> Self {
        let param = format!("{}={}", label.into(), value);
        self.cache_key = format!("{}::{}", self.cache_key, param);
        self.segments.push(param);
        self
    }

    /// Add a plain segment (no key=value format).
    pub fn segment(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.cache_key = format!("{}::{}", self.cache_key, name);
        self.segments.push(name);
        self
    }

    /// Get the pre-computed cache key string.
    pub fn cache_key(&self) -> &str {
        &self.cache_key
    }

    /// Get the individual segments.
    pub fn segments(&self) -> &[String] {
        &self.segments
    }
}

impl fmt::Display for QueryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.cache_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_key() {
        let key = QueryKey::new("users");
        assert_eq!(key.cache_key(), "users");
        assert_eq!(key.segments().len(), 1);
        assert_eq!(key.segments()[0], "users");
    }

    #[test]
    fn test_key_with_param() {
        let key = QueryKey::new("users").with("id", 42);
        assert_eq!(key.cache_key(), "users::id=42");
        assert_eq!(key.segments().len(), 2);
    }

    #[test]
    fn test_key_with_multiple_params() {
        let key = QueryKey::new("users").with("id", 42).with("page", 1);
        assert_eq!(key.cache_key(), "users::id=42::page=1");
        assert_eq!(key.segments().len(), 3);
    }

    #[test]
    fn test_key_with_segment() {
        let key = QueryKey::new("users").segment("posts").with("id", 100);
        assert_eq!(key.cache_key(), "users::posts::id=100");
        assert_eq!(key.segments().len(), 3);
    }

    #[test]
    fn test_key_display() {
        let key = QueryKey::new("test").with("x", "y");
        assert_eq!(format!("{}", key), "test::x=y");
    }

    #[test]
    fn test_key_clone() {
        let key = QueryKey::new("a").with("b", 1);
        let cloned = key.clone();
        assert_eq!(key, cloned);
    }

    #[test]
    fn test_key_equality() {
        let k1 = QueryKey::new("users").with("id", 1);
        let k2 = QueryKey::new("users").with("id", 1);
        let k3 = QueryKey::new("users").with("id", 2);
        assert_eq!(k1, k2);
        assert_ne!(k1, k3);
    }

    #[test]
    fn test_key_hash() {
        use std::collections::HashMap;
        let mut map = HashMap::new();
        let key = QueryKey::new("test").with("id", 1);
        map.insert(key.clone(), "value");
        assert_eq!(map.get(&key), Some(&"value"));
    }

    #[test]
    fn test_key_debug() {
        let key = QueryKey::new("test");
        let debug = format!("{:?}", key);
        assert!(debug.contains("test"));
    }

    #[test]
    fn test_key_with_string_value() {
        let key = QueryKey::new("search").with("q", "hello world");
        assert_eq!(key.cache_key(), "search::q=hello world");
    }

    #[test]
    fn test_key_empty_segment() {
        let key = QueryKey::new("");
        assert_eq!(key.cache_key(), "");
    }

    #[test]
    fn test_nested_keys() {
        let key = QueryKey::new("org")
            .with("id", "abc")
            .segment("projects")
            .with("id", 123)
            .segment("tasks")
            .with("status", "open");
        assert_eq!(
            key.cache_key(),
            "org::id=abc::projects::id=123::tasks::status=open"
        );
        assert_eq!(key.segments().len(), 6);
    }
}
