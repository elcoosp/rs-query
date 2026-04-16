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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MutationState;
    use crate::QueryOptions;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_mutation_new() {
        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) });
        let result = (mutation.mutate_fn)(42).await.unwrap();
        assert_eq!(result, "result_42");
    }

    #[tokio::test]
    async fn test_mutation_error() {
        let mutation: Mutation<String, i32> =
            Mutation::new(|_param: i32| async move { Err(QueryError::network("fail")) });
        let result = (mutation.mutate_fn)(42).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_mutation_invalidates_key() {
        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) })
                .invalidates_key(QueryKey::new("users"));

        assert_eq!(mutation.invalidates_keys.len(), 1);
        assert_eq!(mutation.invalidates_keys[0].cache_key(), "users");
    }

    #[test]
    fn test_mutation_invalidates_multiple_keys() {
        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) })
                .invalidates(vec![QueryKey::new("users"), QueryKey::new("posts")]);

        assert_eq!(mutation.invalidates_keys.len(), 2);
    }

    #[test]
    fn test_mutation_on_mutate_callback() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) }).on_mutate(
                move |client: &QueryClient, _params: &i32| {
                    client.set_query_data(&key, "optimistic".to_string(), QueryOptions::default());
                    let rollback_key = key.clone();
                    Ok(Box::new(move |client: &QueryClient| {
                        client.set_query_data(
                            &rollback_key,
                            "rolled_back".to_string(),
                            QueryOptions::default(),
                        );
                    }) as RollbackFn)
                },
            );

        assert!(mutation.on_mutate.is_some());
        let callback = mutation.on_mutate.as_ref().unwrap();
        let rollback = callback(&client, &42).unwrap();
        let data: Option<String> = client.get_query_data(&key);
        assert_eq!(data, Some("optimistic".to_string()));
        rollback(&client);
        let data: Option<String> = client.get_query_data(&key);
        assert_eq!(data, Some("rolled_back".to_string()));
    }

    #[test]
    fn test_mutation_on_mutate_error() {
        let client = QueryClient::new();
        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) }).on_mutate(
                |_client: &QueryClient, _params: &i32| Err(QueryError::custom("optimistic failed")),
            );

        let callback = mutation.on_mutate.as_ref().unwrap();
        let result = callback(&client, &42);
        assert!(result.is_err());
    }

    #[test]
    fn test_mutation_on_success_callback() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) }).on_success(
                move |_data: &String, _params: &i32| {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                },
            );

        assert!(mutation.on_success.is_some());
        (mutation.on_success.as_ref().unwrap())(&"result".to_string(), &42);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_mutation_on_error_callback() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) }).on_error(
                move |_error: &QueryError, _params: &i32| {
                    counter_clone.fetch_add(1, Ordering::SeqCst);
                },
            );

        assert!(mutation.on_error.is_some());
        (mutation.on_error.as_ref().unwrap())(&QueryError::network("fail"), &42);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_mutation_no_callbacks() {
        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) });
        assert!(mutation.on_mutate.is_none());
        assert!(mutation.on_success.is_none());
        assert!(mutation.on_error.is_none());
        assert!(mutation.invalidates_keys.is_empty());
    }
}
