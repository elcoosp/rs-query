// src/error.rs
//! Error types for rs-query

use thiserror::Error;

/// Errors that can occur during query operations.
#[derive(Error, Debug, Clone, PartialEq)]
pub enum QueryError {
    /// A network-level failure (offline, connection refused, I/O error).
    #[error("Network error: {0}")]
    Network(String),

    /// A (de)serialization failure while persisting or restoring data.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Cancellation, mirroring `TanStack`'s `CancelledError { revert, silent }`.
    ///
    /// * `revert` — the caller should restore the pre-fetch state.
    /// * `silent` — the error should not be surfaced as a query error state.
    #[error("Query was cancelled")]
    Cancelled {
        /// Whether the operation should revert to its previous state.
        revert: bool,
        /// Whether the error is silent (e.g. silent cancel on unmount).
        silent: bool,
    },

    /// An operation exceeded its allowed time budget.
    #[error("Timeout error: {0}")]
    Timeout(String),

    /// No `queryFn` was registered for the query.
    #[error("Missing queryFn")]
    MissingQueryFn,

    /// The fetch was paused because the application is offline.
    #[error("Fetch paused (offline)")]
    Paused,

    /// The query function returned `None`/`undefined` data.
    #[error("Query data cannot be undefined")]
    UndefinedData,

    /// A redacted error produced by `dehydrate` when `should_redact_errors`
    /// is enabled (default), mirroring `Error('redacted')`.
    #[error("redacted")]
    Redacted,

    /// A free-form error with a caller-provided message.
    #[error("{0}")]
    Custom(String),
}

impl QueryError {
    /// Create a network error
    pub fn network(msg: impl Into<String>) -> Self {
        Self::Network(msg.into())
    }

    /// Create a serialization error
    pub fn serialization(msg: impl Into<String>) -> Self {
        Self::Serialization(msg.into())
    }

    /// Create a plain cancellation error.
    #[must_use]
    pub const fn cancelled() -> Self {
        Self::Cancelled {
            revert: false,
            silent: false,
        }
    }

    /// Create a revert-cancellation (`TanStack` `cancelQueries` default).
    #[must_use]
    pub const fn cancelled_revert() -> Self {
        Self::Cancelled {
            revert: true,
            silent: false,
        }
    }

    /// Create a silent cancellation (`TanStack` `destroy`).
    #[must_use]
    pub const fn cancelled_silent() -> Self {
        Self::Cancelled {
            revert: false,
            silent: true,
        }
    }

    /// Create a timeout error
    pub fn timeout(msg: impl Into<String>) -> Self {
        Self::Timeout(msg.into())
    }

    /// Create a custom error
    pub fn custom(msg: impl Into<String>) -> Self {
        Self::Custom(msg.into())
    }

    /// Returns `true` for cancellation errors of any shape.
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled { .. })
    }
}

impl From<tokio::task::JoinError> for QueryError {
    fn from(err: tokio::task::JoinError) -> Self {
        if err.is_cancelled() {
            QueryError::cancelled()
        } else {
            QueryError::Custom(err.to_string())
        }
    }
}

impl From<std::io::Error> for QueryError {
    fn from(err: std::io::Error) -> Self {
        QueryError::Network(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_error() {
        let err = QueryError::network("connection refused");
        assert_eq!(err.to_string(), "Network error: connection refused");
    }

    #[test]
    fn test_serialization_error() {
        let err = QueryError::serialization("invalid json");
        assert_eq!(err.to_string(), "Serialization error: invalid json");
    }

    #[test]
    fn test_timeout_error() {
        let err = QueryError::timeout("30s elapsed");
        assert_eq!(err.to_string(), "Timeout error: 30s elapsed");
    }

    #[test]
    fn test_custom_error() {
        let err = QueryError::custom("something went wrong");
        assert_eq!(err.to_string(), "something went wrong");
    }

    #[test]
    fn test_cancelled_display() {
        let err = QueryError::cancelled();
        assert_eq!(err.to_string(), "Query was cancelled");
    }

    #[test]
    fn test_cancelled_variants() {
        assert!(QueryError::cancelled().is_cancelled());
        assert!(QueryError::cancelled_revert().is_cancelled());
        assert!(QueryError::cancelled_silent().is_cancelled());
        assert_eq!(
            QueryError::cancelled_revert(),
            QueryError::Cancelled {
                revert: true,
                silent: false
            }
        );
        assert_eq!(
            QueryError::cancelled_silent(),
            QueryError::Cancelled {
                revert: false,
                silent: true
            }
        );
    }

    #[test]
    fn test_new_error_variants_display() {
        assert_eq!(QueryError::MissingQueryFn.to_string(), "Missing queryFn");
        assert_eq!(QueryError::Paused.to_string(), "Fetch paused (offline)");
        assert_eq!(
            QueryError::UndefinedData.to_string(),
            "Query data cannot be undefined"
        );
        assert_eq!(QueryError::Redacted.to_string(), "redacted");
        assert!(!QueryError::Redacted.is_cancelled());
    }

    #[tokio::test]
    async fn test_from_join_error_cancelled() {
        // One spawn site serves both needs: the first task runs the spawned
        // async block to completion, while the second is aborted before its
        // first poll so that joining it yields the cancelled `JoinError`.
        let spawn_sleeper = || {
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            })
        };

        spawn_sleeper().await.expect("sleeper task completes");
        let handle = spawn_sleeper();
        handle.abort();
        let err: QueryError = handle.await.unwrap_err().into();
        assert_eq!(err, QueryError::cancelled());
    }

    #[tokio::test]
    async fn test_from_join_error_panic() {
        let handle = tokio::spawn(async {
            panic!("test panic");
        });
        let err: QueryError = handle.await.unwrap_err().into();
        assert!(matches!(err, QueryError::Custom(_)));
    }

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: QueryError = io_err.into();
        assert!(matches!(err, QueryError::Network(_)));
    }

    #[test]
    fn test_error_clone() {
        let err = QueryError::network("test");
        let cloned = err.clone();
        assert_eq!(err, cloned);
    }

    #[test]
    fn test_error_debug() {
        let err = QueryError::cancelled();
        let debug_str = format!("{err:?}");
        assert!(debug_str.contains("Cancelled"));
    }

    #[test]
    fn test_error_partial_eq() {
        let e1 = QueryError::network("a");
        let e2 = QueryError::network("a");
        let e3 = QueryError::network("b");
        assert_eq!(e1, e2);
        assert_ne!(e1, e3);
    }
}
