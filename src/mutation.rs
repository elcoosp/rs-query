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
// src/mutation.rs (tests section only)
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QueryKey, QueryOptions};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_mutation_creation() {
        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) });

        assert_eq!(mutation.invalidates_keys.len(), 0);
        assert!(mutation.on_mutate.is_none());
        assert!(mutation.on_success.is_none());
        assert!(mutation.on_error.is_none());

        let result = (mutation.mutate_fn)(42).await;
        assert_eq!(result.unwrap(), "result_42");
    }

    #[tokio::test]
    async fn test_mutation_with_invalidation() {
        let key1 = QueryKey::new("users");
        let key2 = QueryKey::new("posts");

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) })
                .invalidates_key(key1)
                .invalidates_key(key2);

        assert_eq!(mutation.invalidates_keys.len(), 2);

        let result = (mutation.mutate_fn)(1).await;
        assert_eq!(result.unwrap(), "result_1");
    }

    #[tokio::test]
    async fn test_mutation_with_on_mutate() {
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
        let on_mutate = mutation.on_mutate.as_ref().expect("on_mutate is set");
        let rollback = on_mutate(&client, &1).unwrap();
        let data: Option<String> = client.get_query_data(&key_for_test);
        assert_eq!(data, Some("optimistic".to_string()));

        rollback(&client);
        let data: Option<String> = client.get_query_data(&key_for_test);
        assert_eq!(data, Some("rolled_back".to_string()));

        // Execute the mutation fn
        let result = (mutation.mutate_fn)(5).await;
        assert_eq!(result.unwrap(), "result_5");
    }

    #[tokio::test]
    async fn test_mutation_builder_chain() {
        let key1 = QueryKey::new("users");
        let key2 = QueryKey::new("posts");

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) })
                .invalidates_key(key1)
                .invalidates_key(key2);

        assert_eq!(mutation.invalidates_keys.len(), 2);

        let result = (mutation.mutate_fn)(7).await;
        assert_eq!(result.unwrap(), "result_7");
    }

    #[tokio::test]
    async fn test_mutation_invalidates_vec() {
        let key1 = QueryKey::new("users");
        let key2 = QueryKey::new("posts");

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) })
                .invalidates(vec![key1, key2]);

        assert_eq!(mutation.invalidates_keys.len(), 2);

        let result = (mutation.mutate_fn)(3).await;
        assert_eq!(result.unwrap(), "result_3");
    }

    #[tokio::test]
    async fn test_mutation_invalidates_vec_empty() {
        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) })
                .invalidates(vec![]);

        assert_eq!(mutation.invalidates_keys.len(), 0);

        let result = (mutation.mutate_fn)(9).await;
        assert_eq!(result.unwrap(), "result_9");
    }

    #[tokio::test]
    async fn test_mutation_on_success() {
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) }).on_success(
                move |_data: &String, _params: &i32| {
                    called_clone.store(true, Ordering::SeqCst);
                },
            );

        assert!(mutation.on_success.is_some());
        let on_success = mutation.on_success.as_ref().expect("on_success is set");
        on_success(&"result_1".to_string(), &1);
        assert!(called.load(Ordering::SeqCst));

        let result = (mutation.mutate_fn)(1).await;
        assert_eq!(result.unwrap(), "result_1");
    }

    #[tokio::test]
    async fn test_mutation_on_error() {
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) }).on_error(
                move |_error: &QueryError, _params: &i32| {
                    called_clone.store(true, Ordering::SeqCst);
                },
            );

        assert!(mutation.on_error.is_some());
        let on_error = mutation.on_error.as_ref().expect("on_error is set");
        on_error(&QueryError::custom("fail"), &1);
        assert!(called.load(Ordering::SeqCst));

        let result = (mutation.mutate_fn)(2).await;
        assert_eq!(result.unwrap(), "result_2");
    }

    #[tokio::test]
    async fn test_mutation_clone() {
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) })
                .invalidates_key(QueryKey::new("users"))
                .on_success(move |_data: &String, _params: &i32| {
                    called_clone.store(true, Ordering::SeqCst);
                });

        let cloned = mutation.clone();
        assert_eq!(cloned.invalidates_keys.len(), 1);
        assert!(cloned.on_success.is_some());

        let on_success = cloned.on_success.as_ref().expect("on_success is set");
        on_success(&"data".to_string(), &1);
        assert!(called.load(Ordering::SeqCst));

        // Execute the cloned mutation fn
        let result = (cloned.mutate_fn)(11).await;
        assert_eq!(result.unwrap(), "result_11");
    }

    #[tokio::test]
    async fn test_mutation_full_builder_chain() {
        let key = QueryKey::new("users");

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) })
                .invalidates_key(key)
                .invalidates_key(QueryKey::new("posts"));

        assert_eq!(mutation.invalidates_keys.len(), 2);
        assert!(mutation.on_mutate.is_none());
        assert!(mutation.on_success.is_none());
        assert!(mutation.on_error.is_none());

        let result = (mutation.mutate_fn)(99).await;
        assert_eq!(result.unwrap(), "result_99");
    }

    #[tokio::test]
    async fn test_mutation_on_mutate_returns_error() {
        let client = QueryClient::new();

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) }).on_mutate(
                |_client: &QueryClient, _params: &i32| Err(QueryError::custom("optimistic failed")),
            );

        assert!(mutation.on_mutate.is_some());
        let on_mutate = mutation.on_mutate.as_ref().expect("on_mutate is set");
        let result = on_mutate(&client, &1);
        assert!(result.is_err());

        let result = (mutation.mutate_fn)(1).await;
        assert_eq!(result.unwrap(), "result_1");
    }

    #[tokio::test]
    async fn test_mutation_fn_executes_success() {
        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) });

        let result = (mutation.mutate_fn)(42).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "result_42");
    }

    #[tokio::test]
    async fn test_mutation_fn_executes_error() {
        let mutation: Mutation<String, i32> =
            Mutation::new(|_param: i32| async move { Err(QueryError::custom("fail")) });

        let result = (mutation.mutate_fn)(1).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "fail");
    }

    #[tokio::test]
    async fn test_mutation_fn_cloned_executes() {
        let mutation: Mutation<i32, i32> = Mutation::new(|param: i32| async move { Ok(param * 2) });

        let cloned = mutation.clone();

        let result = (cloned.mutate_fn)(21).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_mutation_clone_on_mutate() {
        let key = QueryKey::new("test");
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

        let cloned = mutation.clone();
        assert!(cloned.on_mutate.is_some());

        let client = QueryClient::new();
        let on_mutate = cloned.on_mutate.as_ref().expect("on_mutate is set");
        let rollback = on_mutate(&client, &1).unwrap();
        let data: Option<String> = client.get_query_data(&key_for_test);
        assert_eq!(data, Some("optimistic".to_string()));

        rollback(&client);
        let data: Option<String> = client.get_query_data(&key_for_test);
        assert_eq!(data, Some("rolled_back".to_string()));

        let result = (cloned.mutate_fn)(8).await;
        assert_eq!(result.unwrap(), "result_8");
    }

    #[tokio::test]
    async fn test_mutation_clone_on_error() {
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) }).on_error(
                move |_error: &QueryError, _params: &i32| {
                    called_clone.store(true, Ordering::SeqCst);
                },
            );

        let cloned = mutation.clone();
        assert!(cloned.on_error.is_some());

        let on_error = cloned.on_error.as_ref().expect("on_error is set");
        on_error(&QueryError::custom("err"), &1);
        assert!(called.load(Ordering::SeqCst));

        let result = (cloned.mutate_fn)(4).await;
        assert_eq!(result.unwrap(), "result_4");
    }

    #[tokio::test]
    async fn test_mutation_clone_preserves_invalidates_keys() {
        let key1 = QueryKey::new("users");
        let key2 = QueryKey::new("posts");

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) })
                .invalidates_key(key1)
                .invalidates_key(key2);

        let cloned = mutation.clone();
        assert_eq!(cloned.invalidates_keys.len(), 2);

        let result = (cloned.mutate_fn)(6).await;
        assert_eq!(result.unwrap(), "result_6");
    }

    #[tokio::test]
    async fn test_mutation_fn_with_complex_params() {
        let mutation: Mutation<String, (i32, String)> = Mutation::new(
            |(id, name): (i32, String)| async move { Ok(format!("{}-{}", id, name)) },
        );

        let result = (mutation.mutate_fn)((1, "test".to_string())).await;
        assert_eq!(result.unwrap(), "1-test");
    }

    #[tokio::test]
    async fn test_mutation_on_success_receives_correct_params() {
        let received_param = Arc::new(AtomicUsize::new(0));
        let received_data = Arc::new(AtomicUsize::new(0));
        let param_clone = received_param.clone();
        let data_clone = received_data.clone();

        let mutation: Mutation<usize, usize> =
            Mutation::new(|param: usize| async move { Ok(param * 10) }).on_success(
                move |data: &usize, params: &usize| {
                    data_clone.store(*data, Ordering::SeqCst);
                    param_clone.store(*params, Ordering::SeqCst);
                },
            );

        let on_success = mutation.on_success.as_ref().expect("on_success is set");
        on_success(&100, &5);
        assert_eq!(received_data.load(Ordering::SeqCst), 100);
        assert_eq!(received_param.load(Ordering::SeqCst), 5);

        let result = (mutation.mutate_fn)(5).await;
        assert_eq!(result.unwrap(), 50);
    }

    #[tokio::test]
    async fn test_mutation_on_error_receives_correct_params() {
        let received_error = Arc::new(AtomicBool::new(false));
        let received_param = Arc::new(AtomicBool::new(false));
        let err_clone = received_error.clone();
        let param_clone = received_param.clone();

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) }).on_error(
                move |error: &QueryError, _params: &i32| {
                    err_clone.store(error.to_string() == "test_err", Ordering::SeqCst);
                    param_clone.store(true, Ordering::SeqCst);
                },
            );

        let on_error = mutation.on_error.as_ref().expect("on_error is set");
        on_error(&QueryError::custom("test_err"), &1);
        assert!(received_error.load(Ordering::SeqCst));
        assert!(received_param.load(Ordering::SeqCst));

        let result = (mutation.mutate_fn)(1).await;
        assert_eq!(result.unwrap(), "result_1");
    }

    #[tokio::test]
    async fn test_mutation_clone_fn_independent_execution() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let count_clone = call_count.clone();

        let mutation: Mutation<i32, i32> = Mutation::new(move |param: i32| {
            let c = count_clone.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(param)
            }
        });

        let cloned = mutation.clone();

        // Call original
        let _ = (mutation.mutate_fn)(1).await;
        // Call clone
        let _ = (cloned.mutate_fn)(2).await;

        // Both should have executed (shared Arc counter)
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_mutation_invalidates_replaces_keys() {
        let key1 = QueryKey::new("users");
        let key2 = QueryKey::new("posts");
        let key3 = QueryKey::new("comments");

        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{}", param)) })
                .invalidates_key(key1)
                .invalidates_key(key2)
                .invalidates(vec![key3]);

        // invalidates() replaces, so only key3 remains
        assert_eq!(mutation.invalidates_keys.len(), 1);

        let result = (mutation.mutate_fn)(1).await;
        assert_eq!(result.unwrap(), "result_1");
    }
}
