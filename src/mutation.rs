// src/mutation.rs
//! Mutation definition and execution

use crate::{QueryClient, QueryError, QueryKey};
use std::sync::Arc;

/// Rollback function type for optimistic updates.
pub type RollbackFn = Box<dyn FnOnce(&QueryClient) + Send + Sync>;

/// Definition of a mutation.
pub struct Mutation<T, P> {
    pub mutate_fn: Arc<
        dyn Fn(
                P,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, QueryError>> + Send>>
            + Send
            + Sync,
    >,
    pub invalidates_keys: Vec<QueryKey>,
    pub on_mutate:
        Option<Arc<dyn Fn(&QueryClient, &P) -> Result<RollbackFn, QueryError> + Send + Sync>>,
    pub on_success: Option<Arc<dyn Fn(&T, &P) + Send + Sync>>,
    pub on_error: Option<Arc<dyn Fn(&QueryError, &P) + Send + Sync>>,
}

impl<T: Send + Sync + 'static, P: Send + Sync + 'static> Clone for Mutation<T, P> {
    fn clone(&self) -> Self {
        Self {
            mutate_fn: Arc::clone(&self.mutate_fn),
            invalidates_keys: self.invalidates_keys.clone(),
            on_mutate: self.on_mutate.clone(),
            on_success: self.on_success.clone(),
            on_error: self.on_error.clone(),
        }
    }
}

impl<T: Send + Sync + 'static, P: Send + Sync + 'static> Mutation<T, P> {
    pub fn new<F, Fut>(mutate_fn: F) -> Self
    where
        F: Fn(P) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<T, QueryError>> + Send + 'static,
    {
        Self {
            mutate_fn: Arc::new(move |params| Box::pin(mutate_fn(params))),
            invalidates_keys: Vec::new(),
            on_mutate: None,
            on_success: None,
            on_error: None,
        }
    }

    pub fn invalidates_key(mut self, key: QueryKey) -> Self {
        self.invalidates_keys.push(key);
        self
    }

    pub fn invalidates(mut self, keys: Vec<QueryKey>) -> Self {
        self.invalidates_keys = keys;
        self
    }

    pub fn on_mutate<F>(mut self, f: F) -> Self
    where
        F: Fn(&QueryClient, &P) -> Result<RollbackFn, QueryError> + Send + Sync + 'static,
    {
        self.on_mutate = Some(Arc::new(f));
        self
    }

    pub fn on_success<F>(mut self, f: F) -> Self
    where
        F: Fn(&T, &P) + Send + Sync + 'static,
    {
        self.on_success = Some(Arc::new(f));
        self
    }

    pub fn on_error<F>(mut self, f: F) -> Self
    where
        F: Fn(&QueryError, &P) + Send + Sync + 'static,
    {
        self.on_error = Some(Arc::new(f));
        self
    }
}

// src/mutation.rs (tests section only)
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QueryKey, QueryOptions};

    #[test]
    fn test_mutation_creation() {
        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) });

        assert_eq!(mutation.invalidates_keys.len(), 0);
        assert!(mutation.on_mutate.is_none());
    }

    #[test]
    fn test_mutation_with_invalidation() {
        let key1 = QueryKey::new("users");
        let key2 = QueryKey::new("posts");

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) })
                .invalidates_key(key1)
                .invalidates_key(key2);

        assert_eq!(mutation.invalidates_keys.len(), 2);
    }

    #[test]
    fn test_mutation_with_on_mutate() {
        let key = QueryKey::new("users");
        let key_for_test = key.clone();

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) }).on_mutate(
                move |client: &QueryClient, _params: &i32| {
                    client.set_query_data(&key, "optimistic".to_string(), QueryOptions::default());
                    let key_for_rollback = key.clone();
                    Ok(Box::new(move |client: &QueryClient| {
                        client.set_query_data(
                            &key_for_rollback,
                            "rolled_back".to_string(),
                            QueryOptions::default(),
                        );
                    }))
                },
            );

        assert!(mutation.on_mutate.is_some());

        let client = QueryClient::new();
        if let Some(on_mutate) = &mutation.on_mutate {
            let rollback = on_mutate(&client, &1).unwrap();
            let data: Option<String> = client.get_query_data(&key_for_test);
            assert_eq!(data, Some("optimistic".to_string()));

            rollback(&client);
            let data: Option<String> = client.get_query_data(&key_for_test);
            assert_eq!(data, Some("rolled_back".to_string()));
        }
    }

    #[test]
    fn test_mutation_builder_chain() {
        let key1 = QueryKey::new("users");
        let key2 = QueryKey::new("posts");

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) })
                .invalidates_key(key1)
                .invalidates_key(key2);

        assert_eq!(mutation.invalidates_keys.len(), 2);
    }
}
