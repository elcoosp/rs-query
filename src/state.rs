// src/state.rs
//! Query and mutation state types

use crate::QueryError;

/// Aggregate status of a query, mirroring `TanStack` Query's `status`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum QueryStatus {
    /// No data yet (idle, first load, or awaiting a paused fetch).
    #[default]
    Pending,
    /// The last fetch failed and no data is available.
    Error,
    /// Data is available.
    Success,
}

impl QueryStatus {
    /// TanStack-compatible name of the status.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            QueryStatus::Pending => "pending",
            QueryStatus::Error => "error",
            QueryStatus::Success => "success",
        }
    }
}

/// Aggregate fetch status of a query, mirroring `fetchStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FetchStatus {
    /// Not currently fetching.
    #[default]
    Idle,
    /// A fetch is in flight.
    Fetching,
    /// A fetch wants to run but is paused (e.g. offline).
    Paused,
}

impl FetchStatus {
    /// TanStack-compatible name of the fetch status.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            FetchStatus::Idle => "idle",
            FetchStatus::Fetching => "fetching",
            FetchStatus::Paused => "paused",
        }
    }
}

/// Aggregate status of a mutation, mirroring `MutationStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MutationStatus {
    /// The mutation has not been executed (or was reset).
    #[default]
    Idle,
    /// The mutation is executing.
    Pending,
    /// The last execution succeeded.
    Success,
    /// The last execution failed.
    Error,
}

impl MutationStatus {
    /// TanStack-compatible name of the status.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            MutationStatus::Idle => "idle",
            MutationStatus::Pending => "pending",
            MutationStatus::Success => "success",
            MutationStatus::Error => "error",
        }
    }
}

/// Direction of an infinite-query page fetch, mirroring `FetchDirection`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FetchDirection {
    /// Pages are appended (forward pagination).
    #[default]
    Forward,
    /// Pages are prepended (backward pagination).
    Backward,
}

impl FetchDirection {
    /// TanStack-compatible name of the direction.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            FetchDirection::Forward => "forward",
            FetchDirection::Backward => "backward",
        }
    }
}

/// Represents the current state of a query.
///
/// `PartialEq` (with `T: PartialEq`) lets bindings suppress redundant signal
/// writes (`set_if_eq`) so pull-based frameworks only re-render on real
/// changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum QueryState<T> {
    /// Not fetching and no data: the query has never run (or was reset).
    #[default]
    Idle,
    /// Fetching for the first time, without any data to show.
    Loading,
    /// Loading but with placeholder data shown.
    LoadingWithPlaceholder(T),
    /// A background refetch is in flight over existing data.
    Refetching(T),
    /// Fresh data is available.
    Success(T),
    /// Data is available but older than the configured `stale_time`.
    Stale(T),
    /// The last fetch failed; stale data may still be shown.
    Error {
        /// The failure that produced this state.
        error: QueryError,
        /// Previously cached data, if any (kept for display).
        stale_data: Option<T>,
    },
}

impl<T> QueryState<T> {
    /// The data currently held by the state, if any.
    ///
    /// Every variant except `Idle`/`Loading` carries data; the error variant
    /// exposes its stale data, mirroring `TanStack`'s `state.data`.
    pub fn data(&self) -> Option<&T> {
        match self {
            QueryState::Idle | QueryState::Loading => None,
            QueryState::LoadingWithPlaceholder(d)
            | QueryState::Refetching(d)
            | QueryState::Success(d)
            | QueryState::Stale(d) => Some(d),
            QueryState::Error { stale_data, .. } => stale_data.as_ref(),
        }
    }

    /// Mutable access to the data currently held by the state, if any.
    pub fn data_mut(&mut self) -> Option<&mut T> {
        match self {
            QueryState::Idle | QueryState::Loading => None,
            QueryState::LoadingWithPlaceholder(d)
            | QueryState::Refetching(d)
            | QueryState::Success(d)
            | QueryState::Stale(d) => Some(d),
            QueryState::Error { stale_data, .. } => stale_data.as_mut(),
        }
    }

    /// Whether the query is in a first-time loading state (with or without
    /// placeholder data).
    pub fn is_loading(&self) -> bool {
        matches!(
            self,
            QueryState::Loading | QueryState::LoadingWithPlaceholder(_)
        )
    }

    /// Whether a background refetch is in flight over existing data.
    pub fn is_refetching(&self) -> bool {
        matches!(self, QueryState::Refetching(_))
    }

    /// Whether fresh data is available.
    pub fn is_success(&self) -> bool {
        matches!(self, QueryState::Success(_))
    }

    /// Whether the data is present but older than the configured `stale_time`.
    pub fn is_stale(&self) -> bool {
        matches!(self, QueryState::Stale(_))
    }

    /// Whether the last fetch failed.
    pub fn is_error(&self) -> bool {
        matches!(self, QueryState::Error { .. })
    }

    /// The failure of the error state, if any.
    pub fn error(&self) -> Option<&QueryError> {
        match self {
            QueryState::Error { error, .. } => Some(error),
            _ => None,
        }
    }

    /// Whether any data is available (including stale or placeholder data).
    pub fn has_data(&self) -> bool {
        self.data().is_some()
    }

    /// Whether the query has never run (or was reset).
    pub fn is_idle(&self) -> bool {
        matches!(self, QueryState::Idle)
    }

    /// Aggregate status derived from the concrete state, TanStack-style.
    #[must_use]
    pub fn status(&self) -> QueryStatus {
        match self {
            QueryState::Idle | QueryState::Loading | QueryState::LoadingWithPlaceholder(_) => {
                QueryStatus::Pending
            }
            QueryState::Refetching(_) | QueryState::Success(_) | QueryState::Stale(_) => {
                QueryStatus::Success
            }
            QueryState::Error { .. } => QueryStatus::Error,
        }
    }

    /// Aggregate fetch status derived from the concrete state.
    #[must_use]
    pub fn fetch_status(&self) -> FetchStatus {
        match self {
            QueryState::Idle
            | QueryState::Success(_)
            | QueryState::Stale(_)
            | QueryState::Error { .. } => FetchStatus::Idle,
            QueryState::Loading
            | QueryState::LoadingWithPlaceholder(_)
            | QueryState::Refetching(_) => FetchStatus::Fetching,
        }
    }

    /// Map the current data into a new shape, leaving no-data states as
    /// `None`.
    pub fn map<U, F: FnOnce(&T) -> U>(&self, f: F) -> Option<U> {
        self.data().map(f)
    }

    /// Clone the current data out of the state, if any.
    pub fn data_cloned(&self) -> Option<T>
    where
        T: Clone,
    {
        self.data().cloned()
    }

    /// Consume the state and return its data.
    ///
    /// # Panics
    ///
    /// Panics when the state carries no data (`Idle`, `Loading`, or an
    /// `Error` without stale data).
    pub fn unwrap(self) -> T {
        match self {
            QueryState::LoadingWithPlaceholder(d)
            | QueryState::Refetching(d)
            | QueryState::Success(d)
            | QueryState::Stale(d)
            | QueryState::Error {
                stale_data: Some(d),
                ..
            } => d,
            _ => panic!("called unwrap on a QueryState without data"),
        }
    }
}

/// Represents the current state of a mutation.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum MutationState<T> {
    /// The mutation has not been executed (or was reset).
    #[default]
    Idle,
    /// The mutation is executing.
    Loading,
    /// The last execution succeeded with this data.
    Success(T),
    /// The last execution failed with this error.
    Error(QueryError),
}

impl<T> MutationState<T> {
    /// Whether the mutation has not been executed (or was reset).
    pub fn is_idle(&self) -> bool {
        matches!(self, MutationState::Idle)
    }

    /// Whether the mutation is executing.
    pub fn is_loading(&self) -> bool {
        matches!(self, MutationState::Loading)
    }

    /// Whether the last execution succeeded.
    pub fn is_success(&self) -> bool {
        matches!(self, MutationState::Success(_))
    }

    /// Whether the last execution failed.
    pub fn is_error(&self) -> bool {
        matches!(self, MutationState::Error(_))
    }

    /// The data of the successful execution, if any.
    pub fn data(&self) -> Option<&T> {
        match self {
            MutationState::Success(d) => Some(d),
            _ => None,
        }
    }

    /// The error of the failed execution, if any.
    pub fn error(&self) -> Option<&QueryError> {
        match self {
            MutationState::Error(e) => Some(e),
            _ => None,
        }
    }

    /// Clone the success data out of the state, if any.
    pub fn data_cloned(&self) -> Option<T>
    where
        T: Clone,
    {
        self.data().cloned()
    }

    /// Aggregate status derived from the concrete mutation state.
    #[must_use]
    pub fn status(&self) -> MutationStatus {
        match self {
            MutationState::Idle => MutationStatus::Idle,
            MutationState::Loading => MutationStatus::Pending,
            MutationState::Success(_) => MutationStatus::Success,
            MutationState::Error(_) => MutationStatus::Error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let result = state.map(String::len);
        assert_eq!(result, Some(5));
    }

    #[test]
    fn test_map_idle() {
        let state = QueryState::<String>::Idle;
        let result = state.map(String::len);
        assert_eq!(result, None);
    }

    #[test]
    fn test_map_loading() {
        let state = QueryState::<String>::Loading;
        let result = state.map(String::len);
        assert_eq!(result, None);
    }

    #[test]
    fn test_map_error() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: None,
        };
        let result = state.map(String::len);
        assert_eq!(result, None);
    }

    #[test]
    fn test_map_error_with_stale() {
        let state: QueryState<String> = QueryState::Error {
            error: QueryError::custom("err"),
            stale_data: Some("stale".to_string()),
        };
        let result = state.map(String::len);
        assert_eq!(result, Some(5));
    }

    #[test]
    fn test_map_refetching() {
        let state = QueryState::Refetching("hello".to_string());
        let result = state.map(String::len);
        assert_eq!(result, Some(5));
    }

    #[test]
    fn test_map_stale() {
        let state = QueryState::Stale("hello".to_string());
        let result = state.map(String::len);
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

    // ----- Coverage: enum `as_str` + `Default` for every status type -----

    #[test]
    fn test_query_status_as_str_and_default() {
        assert_eq!(QueryStatus::default(), QueryStatus::Pending);
        assert_eq!(QueryStatus::Pending.as_str(), "pending");
        assert_eq!(QueryStatus::Error.as_str(), "error");
        assert_eq!(QueryStatus::Success.as_str(), "success");
    }

    #[test]
    fn test_fetch_status_as_str_and_default() {
        assert_eq!(FetchStatus::default(), FetchStatus::Idle);
        assert_eq!(FetchStatus::Idle.as_str(), "idle");
        assert_eq!(FetchStatus::Fetching.as_str(), "fetching");
        assert_eq!(FetchStatus::Paused.as_str(), "paused");
    }

    #[test]
    fn test_mutation_status_as_str_and_default() {
        assert_eq!(MutationStatus::default(), MutationStatus::Idle);
        assert_eq!(MutationStatus::Idle.as_str(), "idle");
        assert_eq!(MutationStatus::Pending.as_str(), "pending");
        assert_eq!(MutationStatus::Success.as_str(), "success");
        assert_eq!(MutationStatus::Error.as_str(), "error");
    }

    #[test]
    fn test_fetch_direction_as_str_and_default() {
        assert_eq!(FetchDirection::default(), FetchDirection::Forward);
        assert_eq!(FetchDirection::Forward.as_str(), "forward");
        assert_eq!(FetchDirection::Backward.as_str(), "backward");
    }

    // ----- Coverage: `status()`/`fetch_status()` on every QueryState variant -----

    #[test]
    fn test_query_state_status_pending_variants() {
        let idle: QueryState<String> = QueryState::Idle;
        assert_eq!(idle.status(), QueryStatus::Pending);
        assert_eq!(idle.fetch_status(), FetchStatus::Idle);

        let loading: QueryState<String> = QueryState::Loading;
        assert_eq!(loading.status(), QueryStatus::Pending);
        assert_eq!(loading.fetch_status(), FetchStatus::Fetching);

        let placeholder = QueryState::LoadingWithPlaceholder("ph".to_string());
        assert_eq!(placeholder.status(), QueryStatus::Pending);
        assert_eq!(placeholder.fetch_status(), FetchStatus::Fetching);
    }

    #[test]
    fn test_query_state_status_success_variants() {
        let refetching = QueryState::Refetching("r".to_string());
        assert_eq!(refetching.status(), QueryStatus::Success);
        assert_eq!(refetching.fetch_status(), FetchStatus::Fetching);

        let success = QueryState::Success("s".to_string());
        assert_eq!(success.status(), QueryStatus::Success);
        assert_eq!(success.fetch_status(), FetchStatus::Idle);

        let stale = QueryState::Stale("st".to_string());
        assert_eq!(stale.status(), QueryStatus::Success);
        assert_eq!(stale.fetch_status(), FetchStatus::Idle);
    }

    #[test]
    fn test_query_state_status_error_variant() {
        let error: QueryState<String> = QueryState::Error {
            error: QueryError::custom("boom"),
            stale_data: None,
        };
        assert_eq!(error.status(), QueryStatus::Error);
        assert_eq!(error.fetch_status(), FetchStatus::Idle);
    }

    // ----- Coverage: LoadingWithPlaceholder accessors -----

    #[test]
    fn test_query_state_loading_with_placeholder_accessors() {
        let state = QueryState::LoadingWithPlaceholder("ph".to_string());
        assert!(state.is_loading());
        assert!(!state.is_success());
        assert_eq!(state.data(), Some(&"ph".to_string()));
        assert_eq!(state.data_cloned(), Some("ph".to_string()));
        assert_eq!(state.map(String::len), Some(2));
        assert_eq!(state.unwrap(), "ph".to_string());
    }

    #[test]
    fn test_query_state_loading_with_placeholder_data_mut() {
        let mut state = QueryState::LoadingWithPlaceholder("ph".to_string());
        if let Some(data) = state.data_mut() {
            *data = "resolved".to_string();
        }
        assert_eq!(state.data(), Some(&"resolved".to_string()));
    }

    // ----- Coverage: error() on non-error states -----

    #[test]
    fn test_query_state_error_none_for_non_error_states() {
        assert_eq!(QueryState::<String>::Idle.error(), None);
        assert_eq!(QueryState::<String>::Loading.error(), None);
        assert_eq!(QueryState::Success("d".to_string()).error(), None);
    }

    // ----- Coverage: MutationState status/accessors on every variant -----

    #[test]
    fn test_mutation_state_accessors_all_variants() {
        let idle = MutationState::<String>::Idle;
        assert!(idle.is_idle());
        assert!(!idle.is_loading());
        assert!(!idle.is_success());
        assert!(!idle.is_error());
        assert_eq!(idle.data(), None);
        assert_eq!(idle.error(), None);

        let loading = MutationState::<String>::Loading;
        assert!(loading.is_loading());
        assert!(!loading.is_idle());
        assert_eq!(loading.data(), None);
        assert_eq!(loading.error(), None);

        let success = MutationState::Success("data".to_string());
        assert!(success.is_success());
        assert!(!success.is_error());
        assert_eq!(success.data(), Some(&"data".to_string()));
        assert_eq!(success.error(), None);

        let error = MutationState::<String>::Error(QueryError::custom("boom"));
        assert!(error.is_error());
        assert!(!error.is_success());
        assert_eq!(error.data(), None);
        assert!(error.error().is_some());
    }

    #[test]
    fn test_mutation_state_status_all_variants() {
        assert_eq!(MutationState::<String>::Idle.status(), MutationStatus::Idle);
        assert_eq!(
            MutationState::<String>::Loading.status(),
            MutationStatus::Pending
        );
        assert_eq!(
            MutationState::Success("d".to_string()).status(),
            MutationStatus::Success
        );
        assert_eq!(
            MutationState::<String>::Error(QueryError::custom("e")).status(),
            MutationStatus::Error
        );
    }
}
