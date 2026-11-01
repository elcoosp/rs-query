// src/key.rs
//! `QueryKey` - hierarchical key system for cache lookups and invalidation

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
    #[must_use]
    pub fn with(mut self, label: impl Into<String>, value: impl std::fmt::Display) -> Self {
        let param = format!("{}={}", label.into(), value);
        self.cache_key = format!("{}::{}", self.cache_key, param);
        self.segments.push(param);
        self
    }

    /// Add a plain segment (no key=value format).
    #[must_use]
    pub fn segment(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.cache_key = format!("{}::{}", self.cache_key, name);
        self.segments.push(name);
        self
    }

    /// Get the pre-computed cache key string.
    #[must_use]
    pub fn cache_key(&self) -> &str {
        &self.cache_key
    }

    /// Get the individual segments.
    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// Prefix check used by filters: `self` starts with `prefix` segment-wise.
    ///
    /// Mirrors `TanStack`'s partial key matching: `"users"` matches
    /// `"users::id=1"`, but not the reverse.
    #[must_use]
    pub fn starts_with(&self, prefix: &QueryKey) -> bool {
        let prefix_segments = prefix.segments();
        self.segments.len() >= prefix_segments.len()
            && self.segments[..prefix_segments.len()] == prefix_segments[..]
    }

    /// Build a key from raw segments.
    #[must_use]
    pub fn from_segments<I, S>(segments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut iter = segments.into_iter();
        match iter.next() {
            None => Self {
                segments: Vec::new(),
                cache_key: String::new(),
            },
            Some(first) => iter.fold(Self::new(first), QueryKey::segment),
        }
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
        assert_eq!(format!("{key}"), "test::x=y");
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
        let debug = format!("{key:?}");
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

    #[test]
    fn test_starts_with_prefix_matches() {
        let prefix = QueryKey::new("users");
        let full = QueryKey::new("users").with("id", 1);
        assert!(full.starts_with(&prefix));
        assert!(!prefix.starts_with(&full));
    }

    #[test]
    fn test_starts_with_exact_match() {
        let key = QueryKey::new("users").with("id", 1);
        assert!(key.starts_with(&key));
    }

    #[test]
    fn test_starts_with_different_branch() {
        let a = QueryKey::new("users").segment("a");
        let b = QueryKey::new("users").segment("b");
        assert!(!b.starts_with(&a));
    }

    #[test]
    fn test_starts_with_shorter_candidate() {
        let prefix = QueryKey::new("users").segment("a").segment("b");
        let short = QueryKey::new("users");
        assert!(!short.starts_with(&prefix));
    }

    #[test]
    fn test_from_segments() {
        let key = QueryKey::from_segments(["users", "id=1", "posts"]);
        assert_eq!(key.cache_key(), "users::id=1::posts");
        assert_eq!(key.segments().len(), 3);
    }

    #[test]
    fn test_from_segments_empty() {
        let key = QueryKey::from_segments(Vec::<String>::new());
        assert_eq!(key.cache_key(), "");
        assert_eq!(key.segments(), [] as [String; 0]);
    }
}
