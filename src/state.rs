//! Query and mutation state types.

use crate::QueryError;

/// Represents the current state of a query.
#[derive(Clone, Debug)]
pub enum QueryState<T> {
    Idle,
    Loading,
    /// Loading with placeholder data (used when `placeholderData` is configured)
    LoadingWithPlaceholder(T),
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
            QueryState::LoadingWithPlaceholder(d) => Some(d),
            QueryState::Refetching(d) => Some(d),
            QueryState::Success(d) => Some(d),
            QueryState::Stale(d) => Some(d),
            QueryState::Error { stale_data, .. } => stale_data.as_ref(),
        }
    }

    pub fn data_cloned(&self) -> Option<T>
    where
        T: Clone,
    {
        self.data().cloned()
    }

    pub fn is_loading(&self) -> bool {
        matches!(
            self,
            QueryState::Loading | QueryState::LoadingWithPlaceholder(_) | QueryState::Refetching(_)
        )
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

    pub fn is_idle(&self) -> bool {
        matches!(self, QueryState::Idle)
    }

    pub fn error(&self) -> Option<&QueryError> {
        match self {
            QueryState::Error { error, .. } => Some(error),
            _ => None,
        }
    }
}

impl<T: Clone> Default for QueryState<T> {
    fn default() -> Self {
        QueryState::Idle
    }
}

/// Variant of a query state update, used for notifications.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryStateVariant {
    Idle,
    Loading,
    Refetching,
    Success,
    Stale,
    Error,
}

/// A state update notification sent to subscribers.
#[derive(Clone, Debug)]
pub struct QueryStateUpdate {
    pub key: String,
    pub state_variant: QueryStateVariant,
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
    fn test_placeholder_data_value() {
        let ph = PlaceholderData::value("hello".to_string());
        assert_eq!(ph.resolve(None), "hello");
        assert_eq!(ph.resolve(Some(&"world".to_string())), "hello");
    }

    #[test]
    fn test_placeholder_data_function() {
        let ph: PlaceholderData<String> =
            PlaceholderData::function(|prev| prev.map(|s| s.clone()).unwrap_or_default());
        assert_eq!(ph.resolve(None), "");
        assert_eq!(ph.resolve(Some(&"world".to_string())), "world");
    }

    #[test]
    fn test_query_with_initial_data() {
        use crate::QueryClient;

        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<String> = Query::new(key.clone(), || async { Ok("fetched".to_string()) })
            .initial_data("initial".to_string());

        let observer = crate::QueryObserver::from_query(&client, &query);
        assert_eq!(observer.state().data(), Some(&"initial".to_string()));
    }

    #[tokio::test]
    async fn test_placeholder_data_shown_during_loading() {
        use crate::QueryClient;
        use std::time::Duration;

        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<String> = Query::new(key.clone(), || async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok("fetched".to_string())
        })
        .placeholder_data(PlaceholderData::value("placeholder".to_string()));

        let mut observer = crate::QueryObserver::from_query(&client, &query);
        observer.set_loading();
        assert!(matches!(
            observer.state(),
            QueryState::LoadingWithPlaceholder(ref s) if s == "placeholder"
        ));
    }

    #[test]
    fn test_structural_sharing_requires_partial_eq() {
        // This test verifies that structural_sharing(true) compiles only when T: PartialEq.
        let _query: Query<Vec<i32>> =
            Query::new(QueryKey::new("test"), || async { Ok(vec![1, 2, 3]) })
                .structural_sharing(true);
    }

    #[test]
    fn test_query_state_idle() {
        let state = QueryState::<String>::Idle;
        assert!(state.is_idle());
        assert!(!state.is_loading());
        assert!(!state.is_success());
        assert!(!state.is_error());
        assert_eq!(state.data(), None);
    }

    #[test]
    fn test_query_state_loading() {
        let state = QueryState::<String>::Loading;
        assert!(!state.is_idle());
        assert!(state.is_loading());
        assert!(!state.is_success());
        assert!(!state.is_error());
        assert_eq!(state.data(), None);
    }

    #[test]
    fn test_query_state_success() {
        let state = QueryState::Success("hello".to_string());
        assert!(!state.is_idle());
        assert!(!state.is_loading());
        assert!(state.is_success());
        assert!(!state.is_error());
        assert_eq!(state.data(), Some(&"hello".to_string()));
    }

    #[test]
    fn test_query_state_error() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("test error"),
            stale_data: None,
        };
        assert!(!state.is_idle());
        assert!(!state.is_loading());
        assert!(!state.is_success());
        assert!(state.is_error());
        assert_eq!(state.data(), None);
        assert_eq!(state.error(), Some(&QueryError::custom("test error")));
    }

    #[test]
    fn test_query_state_error_with_stale_data() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("test error"),
            stale_data: Some("stale".to_string()),
        };
        assert!(state.is_error());
        assert_eq!(state.data(), Some(&"stale".to_string()));
    }

    #[test]
    fn test_query_state_refetching() {
        let state = QueryState::Refetching("hello".to_string());
        assert!(!state.is_idle());
        assert!(!state.is_loading());
        assert!(!state.is_success());
        assert!(!state.is_error());
        assert!(state.is_refetching());
        assert_eq!(state.data(), Some(&"hello".to_string()));
    }

    #[test]
    fn test_query_state_stale() {
        let state = QueryState::Stale("hello".to_string());
        assert!(!state.is_idle());
        assert!(!state.is_loading());
        assert!(!state.is_success());
        assert!(!state.is_error());
        assert!(state.is_stale());
        assert_eq!(state.data(), Some(&"hello".to_string()));
    }

    #[test]
    fn test_data_mut_idle() {
        let mut state: QueryState<String> = QueryState::Idle;
        assert_eq!(state.data_mut(), None);
    }

    #[test]
    fn test_data_mut_loading() {
        let mut state: QueryState<String> = QueryState::Loading;
        assert_eq!(state.data_mut(), None);
    }

    #[test]
    fn test_data_mut_success() {
        let mut state = QueryState::Success("hello".to_string());
        if let Some(data) = state.data_mut() {
            *data = "world".to_string();
        }
        assert_eq!(state.data(), Some(&"world".to_string()));
    }

    #[test]
    fn test_data_mut_error() {
        let mut state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("test error"),
            stale_data: Some("stale".to_string()),
        };
        if let Some(data) = state.data_mut() {
            *data = "updated".to_string();
        }
        assert_eq!(state.data(), Some(&"updated".to_string()));
    }

    #[test]
    fn test_data_mut_error_no_stale() {
        let mut state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("test error"),
            stale_data: None,
        };
        assert_eq!(state.data_mut(), None);
    }

    #[test]
    fn test_data_mut_refetching() {
        let mut state = QueryState::Refetching("hello".to_string());
        if let Some(data) = state.data_mut() {
            *data = "world".to_string();
        }
        assert_eq!(state.data(), Some(&"world".to_string()));
    }

    #[test]
    fn test_data_mut_stale() {
        let mut state = QueryState::Stale("hello".to_string());
        if let Some(data) = state.data_mut() {
            *data = "world".to_string();
        }
        assert_eq!(state.data(), Some(&"world".to_string()));
    }
    #[test]
    fn test_has_data_true() {
        let state = QueryState::Success("hello".to_string());
        assert!(state.has_data());
    }

    #[test]
    fn test_has_data_false() {
        let state = QueryState::<String>::Idle;
        assert!(!state.has_data());
    }

    #[test]
    fn test_has_data_loading() {
        let state = QueryState::<String>::Loading;
        assert!(!state.has_data());
    }

    #[test]
    fn test_has_data_error_no_stale() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: None,
        };
        assert!(!state.has_data());
    }

    #[test]
    fn test_has_data_error_with_stale() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: Some("stale".to_string()),
        };
        assert!(state.has_data());
    }

    #[test]
    fn test_map_success() {
        let state = QueryState::Success("hello".to_string());
        let result = state.map(|d| d.len());
        assert_eq!(result, Some(5));
    }

    #[test]
    fn test_map_idle() {
        let state = QueryState::<String>::Idle;
        let result = state.map(|d| d.len());
        assert_eq!(result, None);
    }

    #[test]
    fn test_map_loading() {
        let state = QueryState::<String>::Loading;
        let result = state.map(|d| d.len());
        assert_eq!(result, None);
    }

    #[test]
    fn test_map_error() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: None,
        };
        let result = state.map(|d| d.len());
        assert_eq!(result, None);
    }

    #[test]
    fn test_map_error_with_stale() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: Some("stale".to_string()),
        };
        let result = state.map(|d| d.len());
        assert_eq!(result, Some(5));
    }

    #[test]
    fn test_map_refetching() {
        let state = QueryState::Refetching("hello".to_string());
        let result = state.map(|d| d.len());
        assert_eq!(result, Some(5));
    }

    #[test]
    fn test_map_stale() {
        let state = QueryState::Stale("hello".to_string());
        let result = state.map(|d| d.len());
        assert_eq!(result, Some(5));
    }

    #[test]
    fn test_data_cloned_success() {
        let state = QueryState::Success("hello".to_string());
        assert_eq!(state.data_cloned(), Some("hello".to_string()));
    }

    #[test]
    fn test_data_cloned_idle() {
        let state = QueryState::<String>::Idle;
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_data_cloned_loading() {
        let state = QueryState::<String>::Loading;
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_data_cloned_error_no_stale() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: None,
        };
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_data_cloned_error_with_stale() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: Some("stale".to_string()),
        };
        assert_eq!(state.data_cloned(), Some("stale".to_string()));
    }

    #[test]
    fn test_data_cloned_refetching() {
        let state = QueryState::Refetching("hello".to_string());
        assert_eq!(state.data_cloned(), Some("hello".to_string()));
    }

    #[test]
    fn test_data_cloned_stale() {
        let state = QueryState::Stale("hello".to_string());
        assert_eq!(state.data_cloned(), Some("hello".to_string()));
    }

    #[test]
    fn test_unwrap_success() {
        let state = QueryState::Success("hello".to_string());
        assert_eq!(state.unwrap(), "hello".to_string());
    }

    #[test]
    fn test_unwrap_refetching() {
        let state = QueryState::Refetching("hello".to_string());
        assert_eq!(state.unwrap(), "hello".to_string());
    }

    #[test]
    fn test_unwrap_stale() {
        let state = QueryState::Stale("hello".to_string());
        assert_eq!(state.unwrap(), "hello".to_string());
    }

    #[test]
    fn test_unwrap_error_with_stale() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: Some("stale".to_string()),
        };
        assert_eq!(state.unwrap(), "stale".to_string());
    }

    #[test]
    #[should_panic(expected = "called unwrap on a QueryState without data")]
    fn test_unwrap_idle_panics() {
        let state = QueryState::<String>::Idle;
        state.unwrap();
    }

    #[test]
    #[should_panic(expected = "called unwrap on a QueryState without data")]
    fn test_unwrap_loading_panics() {
        let state = QueryState::<String>::Loading;
        state.unwrap();
    }

    #[test]
    #[should_panic(expected = "called unwrap on a QueryState without data")]
    fn test_unwrap_error_no_stale_panics() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: None,
        };
        state.unwrap();
    }

    #[test]
    fn test_mutation_state_data_cloned_success() {
        let state = MutationState::Success("data".to_string());
        assert_eq!(state.data_cloned(), Some("data".to_string()));
    }

    #[test]
    fn test_mutation_state_data_cloned_idle() {
        let state = MutationState::<String>::Idle;
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_mutation_state_data_cloned_loading() {
        let state = MutationState::<String>::Loading;
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_mutation_state_data_cloned_error() {
        let state = MutationState::<String>::Error(QueryError::custom("err"));
        assert_eq!(state.data_cloned(), None);
    }

    #[test]
    fn test_mutation_state_default() {
        let state = MutationState::<String>::default();
        assert!(state.is_idle());
    }

    #[test]
    fn test_query_state_default() {
        let state = QueryState::<String>::default();
        assert!(state.is_idle());
    }
}
