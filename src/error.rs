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
