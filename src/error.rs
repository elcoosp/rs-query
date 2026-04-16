// src/error.rs
//! Error types for rs-query

use thiserror::Error;

/// Errors that can occur during query operations.
#[derive(Error, Debug, Clone, PartialEq)]
pub enum QueryError {
    #[error("Network error: {0}")]
    Network(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Query was cancelled")]
    Cancelled,

    #[error("Timeout error: {0}")]
    Timeout(String),

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

    /// Create a timeout error
    pub fn timeout(msg: impl Into<String>) -> Self {
        Self::Timeout(msg.into())
    }

    /// Create a custom error
    pub fn custom(msg: impl Into<String>) -> Self {
        Self::Custom(msg.into())
    }
}

impl From<tokio::task::JoinError> for QueryError {
    fn from(err: tokio::task::JoinError) -> Self {
        if err.is_cancelled() {
            QueryError::Cancelled
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
        let err = QueryError::Cancelled;
        assert_eq!(err.to_string(), "Query was cancelled");
    }

    #[tokio::test]
    async fn test_from_join_error_cancelled() {
        let handle = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(100)).await;
        });
        handle.abort();
        let err: QueryError = handle.await.unwrap_err().into();
        assert_eq!(err, QueryError::Cancelled);
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
        let err = QueryError::Cancelled;
        let debug_str = format!("{:?}", err);
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
