//! Query state enum representing all possible states

use crate::QueryError;

/// Represents the state of a query.
///
/// This enum carries data in its variants for ergonomic pattern matching:
///
/// ```rust,ignore
/// match state {
///     QueryState::Success(data) => { /* use data directly */ }
///     QueryState::Error { error, stale_data } => { /* handle error */ }
///     QueryState::Loading => { /* show spinner */ }
///     _ => {}
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub enum QueryState<T: Clone> {
    /// Initial state, no data yet
    #[default]
    Idle,
    /// Currently fetching (first time)
    Loading,
    /// Data available, refetching in background
    Refetching(T),
    /// Fresh data available
    Success(T),
    /// Data is stale but still usable
    Stale(T),
    /// Error occurred (with optional stale data)
    Error {
        error: QueryError,
        stale_data: Option<T>,
    },
}

impl<T: Clone> QueryState<T> {
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }

    pub fn is_fetching(&self) -> bool {
        matches!(self, Self::Loading | Self::Refetching(_))
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success(_))
    }

    pub fn is_stale(&self) -> bool {
        matches!(self, Self::Stale(_))
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. })
    }

    /// Get data if available (success, stale, refetching, or error with stale)
    pub fn data(&self) -> Option<&T> {
        match self {
            Self::Success(d) | Self::Stale(d) | Self::Refetching(d) => Some(d),
            Self::Error { stale_data, .. } => stale_data.as_ref(),
            _ => None,
        }
    }

    /// Get error if in error state
    pub fn error(&self) -> Option<&QueryError> {
        match self {
            Self::Error { error, .. } => Some(error),
            _ => None,
        }
    }

    /// Unwrap data or return default
    pub fn unwrap_or_default(&self) -> T
    where
        T: Default,
    {
        self.data().cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_state_methods() {
        assert!(QueryState::<()>::Idle.is_idle());
        assert!(QueryState::<()>::Loading.is_loading());
        assert!(QueryState::Refetching(()).is_fetching());
        assert!(QueryState::Success(()).is_success());
        assert!(QueryState::Stale(()).is_stale());
        assert!(QueryState::<()>::Error {
            error: QueryError::Custom("e".into()),
            stale_data: None
        }
        .is_error());
    }

    #[test]
    fn test_query_state_data() {
        assert_eq!(QueryState::Success(42).data(), Some(&42));
        assert_eq!(QueryState::Stale(42).data(), Some(&42));
        assert_eq!(QueryState::Refetching(42).data(), Some(&42));
        assert_eq!(QueryState::<i32>::Idle.data(), None);
        assert_eq!(QueryState::<i32>::Loading.data(), None);
        assert_eq!(
            QueryState::<i32>::Error {
                error: QueryError::Custom("e".into()),
                stale_data: Some(42)
            }
            .data(),
            Some(&42)
        );
    }

    #[test]
    fn test_unwrap_or_default() {
        assert_eq!(QueryState::Success(42).unwrap_or_default(), 42);
        assert_eq!(QueryState::<i32>::Idle.unwrap_or_default(), 0);
    }
}
