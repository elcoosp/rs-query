// src/updater.rs
//! Value-or-function updater, mirroring `TanStack` Query's `Updater<T>` type.
//!
//! Many `TanStack` APIs (`setQueryData`, `placeholderData`, retry callbacks…)
//! accept either a plain value or a function receiving the previous value. The
//! [`Updater`] enum models both, and [`functional_update`] implements the
//! `TanStack` rule that an updater resolving to `None` is a no-op.

/// Type-erased, shareable functional updater used by [`Updater::Function`].
///
/// Receives the previous value (if any) and returns the new value, or `None`
/// to abort the update (`TanStack`: an `undefined` result is a no-op).
pub type UpdaterFn<T> = std::sync::Arc<dyn Fn(Option<&T>) -> Option<T> + Send + Sync>;

/// An update supplied either as a value or as a function of the previous value.
#[derive(Clone)]
pub enum Updater<T: Clone> {
    /// A static value.
    Value(T),
    /// A function computing the new value from the previous one.
    Function(UpdaterFn<T>),
}

impl<T: Clone + std::fmt::Debug> std::fmt::Debug for Updater<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Updater::Value(value) => f.debug_tuple("Value").field(value).finish(),
            Updater::Function(_) => f.write_str("Function(<fn>)"),
        }
    }
}

impl<T: Clone> Updater<T> {
    /// Create a static-value updater.
    #[must_use]
    pub fn value(value: T) -> Self {
        Updater::Value(value)
    }

    /// Create a functional updater. Returning `None` inside `f` makes the
    /// update a no-op (`TanStack`: `undefined` result aborts the write).
    #[must_use]
    pub fn function<F>(f: F) -> Self
    where
        F: Fn(Option<&T>) -> Option<T> + Send + Sync + 'static,
    {
        Updater::Function(std::sync::Arc::new(f))
    }

    /// Apply the updater to the previous value.
    #[must_use]
    pub fn apply(&self, previous: Option<&T>) -> Option<T> {
        match self {
            Updater::Value(value) => Some(value.clone()),
            Updater::Function(function) => function(previous),
        }
    }
}

/// Apply an [`Updater`], returning the new value or `None` for a no-op update.
#[must_use]
pub fn functional_update<T: Clone>(updater: &Updater<T>, previous: Option<&T>) -> Option<T> {
    updater.apply(previous)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Opaque;

    struct DebugNoBound<'a>(&'a Updater<Opaque>);
    impl std::fmt::Debug for DebugNoBound<'_> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match &self.0 {
                Updater::Value(_) => f.write_str("Value(<opaque>)"),
                Updater::Function(_) => f.write_str("Function(<fn>)"),
            }
        }
    }

    #[test]
    fn test_updater_value_applies_clone() {
        let updater = Updater::value(42);
        assert_eq!(updater.apply(None), Some(42));
        assert_eq!(updater.apply(Some(&1)), Some(42));
    }

    #[test]
    fn test_updater_function_receives_previous() {
        let updater = Updater::function(|prev: Option<&i32>| prev.map(|v| v + 1));
        assert_eq!(updater.apply(Some(&41)), Some(42));
        assert_eq!(updater.apply(None), None);
    }

    #[test]
    fn test_updater_function_none_is_noop() {
        let updater: Updater<String> = Updater::function(|_| None);
        assert_eq!(updater.apply(Some(&"old".to_string())), None);
    }

    #[test]
    fn test_functional_update_value() {
        let updater = Updater::value("data".to_string());
        assert_eq!(functional_update(&updater, None), Some("data".to_string()));
    }

    #[test]
    fn test_functional_update_function() {
        let updater =
            Updater::function(|prev: Option<&Vec<i32>>| Some(prev.cloned().unwrap_or_default()));
        assert_eq!(
            functional_update(&updater, Some(&vec![1, 2])),
            Some(vec![1, 2])
        );
        assert_eq!(functional_update(&updater, None), Some(vec![]));
    }

    #[test]
    fn test_updater_clone_keeps_behavior() {
        let updater = Updater::function(|prev: Option<&i32>| Some(prev.unwrap_or(&0) * 2));
        let cloned = updater.clone();
        assert_eq!(cloned.apply(Some(&10)), Some(20));
    }

    #[test]
    fn test_updater_debug_value() {
        let updater = Updater::value(7);
        assert!(format!("{updater:?}").contains("Value"));
    }

    #[test]
    fn test_updater_debug_function() {
        let updater: Updater<i32> = Updater::function(|_| None);
        assert!(format!("{updater:?}").contains("Function"));
    }

    #[test]
    fn test_updater_debug_needs_t_debug() {
        // Non-Debug payloads still produce a usable debug string.
        let updater = Updater::value(Opaque);
        assert!(format!("{:?}", DebugNoBound(&updater)).contains("Value"));
    }

    #[test]
    fn test_updater_debug_no_bound_function_arm() {
        let updater: Updater<Opaque> = Updater::function(|_| None);
        assert!(format!("{:?}", DebugNoBound(&updater)).contains("Function"));
    }

    #[test]
    fn test_updater_function_noop_with_previous_value() {
        let updater: Updater<Opaque> = Updater::function(|_| None);
        let cloned = updater.clone();
        assert!(cloned.apply(Some(&Opaque)).is_none());
    }
}
