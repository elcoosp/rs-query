//! Mutation definition

use crate::{QueryClient, QueryError, QueryKey};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Type alias for boxed async mutation function
pub type MutateFn<T, P> =
    Arc<dyn Fn(P) -> Pin<Box<dyn Future<Output = Result<T, QueryError>> + Send>> + Send + Sync>;

/// Rollback context returned from `on_mutate` to revert optimistic updates on error.
pub type RollbackContext = Box<dyn FnOnce(&QueryClient) + Send + Sync>;

/// Callback type for optimistic updates (no longer parameterized over T).
pub type OnMutateFn<P> =
    Arc<dyn Fn(&QueryClient, &P) -> Result<RollbackContext, QueryError> + Send + Sync>;

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
/// .invalidates_key(QueryKey::new("users"))
/// .on_mutate(|client, params| {
///     // Optimistically add the new user to the cache
///     client.set_query_data(&QueryKey::new("users"), |old: Option<Vec<User>>| {
///         let mut users = old.unwrap_or_default();
///         users.push(User { id: params.id, name: params.name.clone() });
///         users
///     });
///     Ok(Box::new(move |client| {
///         // Rollback: remove the user
///         client.set_query_data(&QueryKey::new("users"), |old: Option<Vec<User>>| {
///             old.map(|mut users| {
///                 users.retain(|u| u.id != params.id);
///                 users
///             })
///         });
///     }))
/// });
/// ```
pub struct Mutation<T, P>
where
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + 'static,
{
    pub mutate_fn: MutateFn<T, P>,
    pub invalidate_keys: Vec<QueryKey>,
    pub on_mutate: Option<OnMutateFn<P>>,
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
            on_mutate: None,
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

    /// Set an `on_mutate` callback for optimistic updates.
    pub fn on_mutate<F>(mut self, f: F) -> Self
    where
        F: Fn(&QueryClient, &P) -> Result<RollbackContext, QueryError> + Send + Sync + 'static,
    {
        self.on_mutate = Some(Arc::new(f));
        self
    }
}
