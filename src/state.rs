// src/state.rs
//! Query and mutation state types

use crate::QueryError;

/// Represents the current state of a query.
#[derive(Clone, Debug)]
pub enum QueryState<T> {
    Idle,
    Loading,
    Refetching(T),
    Success(T),
    Stale(T),
    Error {
        error: QueryError,
        stale_data: Option<T>,
    },
}

impl<T> QueryState<T> {
    pub fn data(&self) -> Option<&T> {
        match self {
            QueryState::Idle => None,
            QueryState::Loading => None,
            QueryState::Refetching(d) => Some(d),
            QueryState::Success(d) => Some(d),
            QueryState::Stale(d) => Some(d),
            QueryState::Error { stale_data, .. } => stale_data.as_ref(),
        }
    }

    pub fn data_mut(&mut self) -> Option<&mut T> {
        match self {
            QueryState::Idle => None,
            QueryState::Loading => None,
            QueryState::Refetching(d) => Some(d),
            QueryState::Success(d) => Some(d),
            QueryState::Stale(d) => Some(d),
            QueryState::Error { stale_data, .. } => stale_data.as_mut(),
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, QueryState::Loading)
    }

    pub fn is_refetching(&self) -> bool {
        matches!(self, QueryState::Refetching(_))
    }

    pub fn is_success(&self) -> bool {
        matches!(self, QueryState::Success(_))
    }

    pub fn is_stale(&self) -> bool {
        matches!(self, QueryState::Stale(_))
    }

    pub fn is_error(&self) -> bool {
        matches!(self, QueryState::Error { .. })
    }

    pub fn error(&self) -> Option<&QueryError> {
        match self {
            QueryState::Error { error, .. } => Some(error),
            _ => None,
        }
    }

    pub fn has_data(&self) -> bool {
        self.data().is_some()
    }

    pub fn is_idle(&self) -> bool {
        matches!(self, QueryState::Idle)
    }

    pub fn map<U, F: FnOnce(&T) -> U>(&self, f: F) -> Option<U> {
        self.data().map(f)
    }

    pub fn data_cloned(&self) -> Option<T>
    where
        T: Clone,
    {
        self.data().cloned()
    }

    pub fn unwrap(self) -> T {
        match self {
            QueryState::Refetching(d) => d,
            QueryState::Success(d) => d,
            QueryState::Stale(d) => d,
            QueryState::Error {
                stale_data: Some(d),
                ..
            } => d,
            _ => panic!("called unwrap on a QueryState without data"),
        }
    }
}

impl<T: Clone> Default for QueryState<T> {
    fn default() -> Self {
        QueryState::Idle
    }
}

/// Represents the current state of a mutation.
#[derive(Clone, Debug)]
pub enum MutationState<T> {
    Idle,
    Loading,
    Success(T),
    Error(QueryError),
}

impl<T> MutationState<T> {
    pub fn is_idle(&self) -> bool {
        matches!(self, MutationState::Idle)
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, MutationState::Loading)
    }

    pub fn is_success(&self) -> bool {
        matches!(self, MutationState::Success(_))
    }

    pub fn is_error(&self) -> bool {
        matches!(self, MutationState::Error(_))
    }

    pub fn data(&self) -> Option<&T> {
        match self {
            MutationState::Success(d) => Some(d),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&QueryError> {
        match self {
            MutationState::Error(e) => Some(e),
            _ => None,
        }
    }

    pub fn data_cloned(&self) -> Option<T>
    where
        T: Clone,
    {
        self.data().cloned()
    }
}

impl<T> Default for MutationState<T> {
    fn default() -> Self {
        MutationState::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_state_idle() {
        let state: QueryState<String> = QueryState::Idle;
        assert!(state.is_idle());
        assert!(!state.is_loading());
        assert!(!state.is_refetching());
        assert!(!state.is_success());
        assert!(!state.is_stale());
        assert!(!state.is_error());
        assert!(!state.has_data());
        assert_eq!(state.data(), None);
        assert_eq!(state.data_mut(), None);
        assert!(state.error().is_none());
        assert_eq!(state.map(|_| 42), None);
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_query_state_loading() {
        let state: QueryState<String> = QueryState::Loading;
        assert!(!state.is_idle());
        assert!(state.is_loading());
        assert!(!state.is_refetching());
        assert!(!state.is_success());
        assert!(!state.is_stale());
        assert!(!state.is_error());
        assert!(!state.has_data());
        assert_eq!(state.data(), None);
        assert_eq!(state.data_mut(), None);
        assert!(state.error().is_none());
        assert_eq!(state.map(|_| 42), None);
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_query_state_refetching() {
        let state = QueryState::Refetching("data".to_string());
        assert!(!state.is_idle());
        assert!(!state.is_loading());
        assert!(state.is_refetching());
        assert!(!state.is_success());
        assert!(!state.is_stale());
        assert!(!state.is_error());
        assert!(state.has_data());
        assert_eq!(state.data(), Some(&"data".to_string()));
        assert_eq!(state.data_cloned(), Some("data".to_string()));
        assert_eq!(state.map(|d| d.len()), Some(4));
        assert!(state.error().is_none());
        assert_eq!(state.unwrap(), "data".to_string());
    }

    #[test]
    fn test_query_state_refetching_mut() {
        let mut state = QueryState::Refetching("data".to_string());
        assert_eq!(state.data_mut(), Some(&mut "data".to_string()));
    }

    #[test]
    fn test_query_state_success() {
        let state = QueryState::Success(42i32);
        assert!(!state.is_idle());
        assert!(!state.is_loading());
        assert!(!state.is_refetching());
        assert!(state.is_success());
        assert!(!state.is_stale());
        assert!(!state.is_error());
        assert!(state.has_data());
        assert_eq!(state.data(), Some(&42));
        assert_eq!(state.data_cloned(), Some(42));
        assert_eq!(state.map(|d| d * 2), Some(84));
        assert!(state.error().is_none());
        assert_eq!(state.unwrap(), 42);
    }

    #[test]
    fn test_query_state_success_mut() {
        let mut state = QueryState::Success(42i32);
        assert_eq!(state.data_mut(), Some(&mut 42));
    }

    #[test]
    fn test_query_state_stale() {
        let state = QueryState::Stale("old".to_string());
        assert!(!state.is_idle());
        assert!(!state.is_loading());
        assert!(!state.is_refetching());
        assert!(!state.is_success());
        assert!(state.is_stale());
        assert!(!state.is_error());
        assert!(state.has_data());
        assert_eq!(state.data(), Some(&"old".to_string()));
        assert_eq!(state.data_cloned(), Some("old".to_string()));
        assert_eq!(state.map(|d| d.to_uppercase()), Some("OLD".to_string()));
        assert!(state.error().is_none());
        assert_eq!(state.unwrap(), "old".to_string());
    }

    #[test]
    fn test_query_state_stale_mut() {
        let mut state = QueryState::Stale("old".to_string());
        assert_eq!(state.data_mut(), Some(&mut "old".to_string()));
    }

    #[test]
    fn test_query_state_error_with_stale_data() {
        let err = QueryError::network("fail");
        let state: QueryState<String> = QueryState::Error {
            error: err.clone(),
            stale_data: Some("old_data".to_string()),
        };
        assert!(!state.is_idle());
        assert!(!state.is_loading());
        assert!(!state.is_refetching());
        assert!(!state.is_success());
        assert!(!state.is_stale());
        assert!(state.is_error());
        assert!(state.has_data());
        assert_eq!(state.data(), Some(&"old_data".to_string()));
        assert_eq!(state.data_cloned(), Some("old_data".to_string()));
        assert_eq!(state.error(), Some(&err));
        assert_eq!(state.map(|d| d.len()), Some(8));
        assert_eq!(state.unwrap(), "old_data".to_string());
    }

    #[test]
    fn test_query_state_error_with_stale_data_mut() {
        let err = QueryError::network("fail");
        let mut state: QueryState<String> = QueryState::Error {
            error: err,
            stale_data: Some("old_data".to_string()),
        };
        assert_eq!(state.data_mut(), Some(&mut "old_data".to_string()));
    }

    #[test]
    fn test_query_state_error_without_stale_data() {
        let err = QueryError::timeout("30s");
        let state: QueryState<String> = QueryState::Error {
            error: err.clone(),
            stale_data: None,
        };
        assert!(state.is_error());
        assert!(!state.has_data());
        assert_eq!(state.data(), None);
        assert_eq!(state.data_mut(), None);
        assert_eq!(state.data_cloned(), None);
        assert_eq!(state.error(), Some(&err));
        assert_eq!(state.map(|_| 42), None);
    }

    #[test]
    #[should_panic(expected = "called unwrap on a QueryState without data")]
    fn test_query_state_unwrap_idle_panics() {
        let state: QueryState<String> = QueryState::Idle;
        state.unwrap();
    }

    #[test]
    #[should_panic(expected = "called unwrap on a QueryState without data")]
    fn test_query_state_unwrap_loading_panics() {
        let state: QueryState<String> = QueryState::Loading;
        state.unwrap();
    }

    #[test]
    #[should_panic(expected = "called unwrap on a QueryState without data")]
    fn test_query_state_unwrap_error_no_data_panics() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::Cancelled,
            stale_data: None,
        };
        state.unwrap();
    }

    #[test]
    fn test_query_state_default() {
        let state: QueryState<String> = QueryState::default();
        assert!(state.is_idle());
    }

    #[test]
    fn test_query_state_debug() {
        let state = QueryState::Success(42);
        let debug = format!("{:?}", state);
        assert!(debug.contains("Success"));
    }

    #[test]
    fn test_mutation_state_idle() {
        let state: MutationState<String> = MutationState::Idle;
        assert!(state.is_idle());
        assert!(!state.is_loading());
        assert!(!state.is_success());
        assert!(!state.is_error());
        assert_eq!(state.data(), None);
        assert!(state.error().is_none());
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_mutation_state_loading() {
        let state: MutationState<String> = MutationState::Loading;
        assert!(!state.is_idle());
        assert!(state.is_loading());
        assert!(!state.is_success());
        assert!(!state.is_error());
        assert_eq!(state.data(), None);
        assert!(state.error().is_none());
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_mutation_state_success() {
        let state = MutationState::Success("result".to_string());
        assert!(!state.is_idle());
        assert!(!state.is_loading());
        assert!(state.is_success());
        assert!(!state.is_error());
        assert_eq!(state.data(), Some(&"result".to_string()));
        assert_eq!(state.data_cloned(), Some("result".to_string()));
        assert!(state.error().is_none());
    }

    #[test]
    fn test_mutation_state_error() {
        let err = QueryError::custom("mutation failed");
        let state: MutationState<String> = MutationState::Error(err.clone());
        assert!(!state.is_idle());
        assert!(!state.is_loading());
        assert!(!state.is_success());
        assert!(state.is_error());
        assert_eq!(state.data(), None);
        assert_eq!(state.data_cloned(), None);
        assert_eq!(state.error(), Some(&err));
    }

    #[test]
    fn test_mutation_state_default() {
        let state: MutationState<String> = MutationState::default();
        assert!(state.is_idle());
    }

    #[test]
    fn test_mutation_state_debug() {
        let state: MutationState<String> = MutationState::Error(QueryError::Cancelled);
        let debug = format!("{:?}", state);
        assert!(debug.contains("Error"));
    }

    #[test]
    fn test_mutation_state_clone() {
        let state = MutationState::Success(42i32);
        let cloned = state.clone();
        assert_eq!(state.data_cloned(), cloned.data_cloned());
    }
}
