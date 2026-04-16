//! Query error types

use thiserror::Error;

/// Errors that can occur during query/mutation operations
#[derive(Error, Debug, Clone)]
pub enum QueryError {
    #[error("Network error: {0}")]
    Network(String),

    #[error("HTTP error {status}: {message}")]
    Http { status: u16, message: String },

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Timeout after {0:?}")]
    Timeout(std::time::Duration),

    #[error("{0}")]
    Custom(String),
}

impl QueryError {
    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            QueryError::Network(_)
                | QueryError::Timeout(_)
                | QueryError::Http {
                    status: 500..=599,
                    ..
                }
        )
    }

    /// Create a network error
    pub fn network(msg: impl Into<String>) -> Self {
        Self::Network(msg.into())
    }

    /// Create a custom error
    pub fn custom(msg: impl Into<String>) -> Self {
        Self::Custom(msg.into())
    }
}

impl From<String> for QueryError {
    fn from(s: String) -> Self {
        Self::Custom(s)
    }
}

impl From<&str> for QueryError {
    fn from(s: &str) -> Self {
        Self::Custom(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_is_retryable() {
        assert!(QueryError::Network("".into()).is_retryable());
        assert!(QueryError::Timeout(Duration::from_secs(1)).is_retryable());
        assert!(QueryError::Http {
            status: 500,
            message: "".into()
        }
        .is_retryable());
        assert!(QueryError::Http {
            status: 503,
            message: "".into()
        }
        .is_retryable());
        assert!(!QueryError::Http {
            status: 400,
            message: "".into()
        }
        .is_retryable());
        assert!(!QueryError::Custom("".into()).is_retryable());
        assert!(!QueryError::Unauthorized.is_retryable());
        assert!(!QueryError::NotFound("".into()).is_retryable());
        assert!(!QueryError::Validation("".into()).is_retryable());
    }

    #[test]
    fn test_error_display() {
        let err = QueryError::Network("boom".into());
        assert_eq!(err.to_string(), "Network error: boom");

        let err = QueryError::Http {
            status: 404,
            message: "not found".into(),
        };
        assert_eq!(err.to_string(), "HTTP error 404: not found");
    }

    #[test]
    fn test_from_string() {
        let err: QueryError = "oops".to_string().into();
        assert!(matches!(err, QueryError::Custom(s) if s == "oops"));
    }
}
