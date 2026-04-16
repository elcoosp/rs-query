//! Mutation definition

use crate::{QueryError, QueryKey};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Type alias for boxed async mutation function
pub type MutateFn<T, P> =
    Arc<dyn Fn(P) -> Pin<Box<dyn Future<Output = Result<T, QueryError>> + Send>> + Send + Sync>;

/// Mutation state
#[derive(Debug, Clone, Default)]
pub enum MutationState<T: Clone> {
    #[default]
    Idle,
    Pending,
    Success(T),
    Error(QueryError),
}

impl<T: Clone> MutationState<T> {
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success(_))
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }

    pub fn data(&self) -> Option<&T> {
        match self {
            Self::Success(d) => Some(d),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&QueryError> {
        match self {
            Self::Error(e) => Some(e),
            _ => None,
        }
    }
}

/// A mutation definition.
///
/// # Example
///
/// ```rust,ignore
/// let mutation = Mutation::new(|params: CreateUserParams| async move {
///     api::create_user(params).await
/// })
/// .invalidates_key(QueryKey::new("users"));
/// ```
pub struct Mutation<T, P>
where
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + 'static,
{
    pub mutate_fn: MutateFn<T, P>,
    pub invalidate_keys: Vec<QueryKey>,
}

impl<T, P> Mutation<T, P>
where
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + 'static,
{
    /// Create a new mutation
    pub fn new<F, Fut>(mutate_fn: F) -> Self
    where
        F: Fn(P) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, QueryError>> + Send + 'static,
    {
        Self {
            mutate_fn: Arc::new(move |p| Box::pin(mutate_fn(p))),
            invalidate_keys: Vec::new(),
        }
    }

    /// Add query keys to invalidate on success
    pub fn invalidates(mut self, keys: impl IntoIterator<Item = QueryKey>) -> Self {
        self.invalidate_keys.extend(keys);
        self
    }

    /// Add a single key to invalidate
    pub fn invalidates_key(mut self, key: QueryKey) -> Self {
        self.invalidate_keys.push(key);
        self
    }
}
