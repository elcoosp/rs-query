// src/mutation.rs
//! Mutation definition and execution

use crate::mutation_cache::{MutationCache, MutationCacheEntry, MutationScope, now_ms};
use crate::network::NetworkMode;
use crate::state::MutationStatus;
use crate::{QueryClient, QueryError, QueryKey};
use std::sync::Arc;
use std::time::Duration;

/// Rollback function type for optimistic updates.
pub type RollbackFn = Box<dyn FnOnce(&QueryClient) + Send + Sync>;

/// Async mutate function: takes the params and resolves with the result.
///
/// Boxed-future shape so `Mutation` stays object-safe/shareable (`Arc`).
pub type MutateFn<T, P> = Arc<
    dyn Fn(P) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, QueryError>> + Send>>
        + Send
        + Sync,
>;

/// Synchronous `on_mutate` callback producing the optimistic context and a
/// [`RollbackFn`] (`TanStack` `onMutate`).
pub type OnMutateFn<P> =
    Arc<dyn Fn(&QueryClient, &P) -> Result<RollbackFn, QueryError> + Send + Sync>;

/// Success callback receiving the mutation data and params (`TanStack`
/// `onSuccess`).
pub type OnSuccessFn<T, P> = Arc<dyn Fn(&T, &P) + Send + Sync>;

/// Error callback receiving the failure and params (`TanStack` `onError`).
pub type OnErrorFn<P> = Arc<dyn Fn(&QueryError, &P) + Send + Sync>;

/// Settled callback invoked on both outcomes after the per-call callbacks
/// (`TanStack` `onSettled`).
pub type OnSettledFn<T, P> = Arc<dyn Fn(Result<&T, &QueryError>, &P) + Send + Sync>;

/// Definition of a mutation.
pub struct Mutation<T, P> {
    /// The async mutate function (`TanStack` `mutationFn`).
    pub mutate_fn: MutateFn<T, P>,
    /// Keys invalidated after a successful mutation (`TanStack`
    /// `invalidateQueries`).
    pub invalidates_keys: Vec<QueryKey>,
    /// Optimistic-update hook run before `mutate_fn` (`TanStack` `onMutate`).
    pub on_mutate: Option<OnMutateFn<P>>,
    /// Success callback (`TanStack` `onSuccess`).
    pub on_success: Option<OnSuccessFn<T, P>>,
    /// Error callback (`TanStack` `onError`).
    pub on_error: Option<OnErrorFn<P>>,
    /// Optional scope; mutations sharing a scope id run in serial.
    pub scope: Option<MutationScope>,
    /// Number of retries after the first failed attempt (`TanStack` `retry`,
    /// default `0`).
    pub retry: u32,
    /// Delay between retry attempts (default 1 s).
    pub retry_delay: Duration,
    /// Mutation key used by [`crate::MutationFilters`] matching.
    pub mutation_key: Option<QueryKey>,
    /// Mutation-level settled callback, invoked on both outcomes after the
    /// per-call callbacks.
    pub on_settled: Option<OnSettledFn<T, P>>,
    /// How the mutation interacts with network connectivity (`TanStack`
    /// `networkMode`, default [`NetworkMode::Online`]).
    pub network_mode: NetworkMode,
}

impl<T: Send + Sync + 'static, P: Send + Sync + 'static> Clone for Mutation<T, P> {
    fn clone(&self) -> Self {
        Self {
            mutate_fn: Arc::clone(&self.mutate_fn),
            invalidates_keys: self.invalidates_keys.clone(),
            on_mutate: self.on_mutate.clone(),
            on_success: self.on_success.clone(),
            on_error: self.on_error.clone(),
            scope: self.scope.clone(),
            retry: self.retry,
            retry_delay: self.retry_delay,
            mutation_key: self.mutation_key.clone(),
            on_settled: self.on_settled.clone(),
            network_mode: self.network_mode,
        }
    }
}

impl<T: Send + Sync + 'static, P: Send + Sync + 'static> Mutation<T, P> {
    /// Create a mutation with the given async mutate function; every other
    /// option starts at its `TanStack` default (no invalidations, no
    /// callbacks, no scope, `retry: 0`, 1 s retry delay, `networkMode:
    /// 'online'`).
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
            scope: None,
            retry: 0,
            retry_delay: Duration::from_secs(1),
            mutation_key: None,
            on_settled: None,
            network_mode: NetworkMode::default(),
        }
    }

    /// Add a key to invalidate after a successful mutation (appends; see
    /// also [`Mutation::invalidates`]).
    #[must_use]
    pub fn invalidates_key(mut self, key: QueryKey) -> Self {
        self.invalidates_keys.push(key);
        self
    }

    /// Replace the list of keys invalidated after a successful mutation.
    #[must_use]
    pub fn invalidates(mut self, keys: Vec<QueryKey>) -> Self {
        self.invalidates_keys = keys;
        self
    }

    /// Set the optimistic-update hook (`TanStack` `onMutate`); runs before
    /// `mutate_fn` and may return a rollback.
    #[must_use]
    pub fn on_mutate<F>(mut self, f: F) -> Self
    where
        F: Fn(&QueryClient, &P) -> Result<RollbackFn, QueryError> + Send + Sync + 'static,
    {
        self.on_mutate = Some(Arc::new(f));
        self
    }

    /// Set the success callback (`TanStack` `onSuccess`).
    #[must_use]
    pub fn on_success<F>(mut self, f: F) -> Self
    where
        F: Fn(&T, &P) + Send + Sync + 'static,
    {
        self.on_success = Some(Arc::new(f));
        self
    }

    /// Set the error callback (`TanStack` `onError`).
    #[must_use]
    pub fn on_error<F>(mut self, f: F) -> Self
    where
        F: Fn(&QueryError, &P) + Send + Sync + 'static,
    {
        self.on_error = Some(Arc::new(f));
        self
    }

    /// Set the mutation scope; mutations sharing a scope run in serial.
    #[must_use]
    pub fn scope(mut self, scope: MutationScope) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Set the mutation key (used by mutation filters).
    #[must_use]
    pub fn mutation_key(mut self, key: QueryKey) -> Self {
        self.mutation_key = Some(key);
        self
    }

    /// Set how many times a failed attempt is retried (default `0`).
    #[must_use]
    pub fn retry(mut self, retry: u32) -> Self {
        self.retry = retry;
        self
    }

    /// Set the delay between retry attempts (default 1 s).
    #[must_use]
    pub fn retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    /// Set the mutation-level settled callback, invoked on both outcomes with
    /// `Ok(&data)` or `Err(&error)` plus the params.
    #[must_use]
    pub fn on_settled<F>(mut self, f: F) -> Self
    where
        F: Fn(Result<&T, &QueryError>, &P) + Send + Sync + 'static,
    {
        self.on_settled = Some(Arc::new(f));
        self
    }

    /// Set the network mode (default [`NetworkMode::Online`]).
    #[must_use]
    pub fn network_mode(mut self, mode: NetworkMode) -> Self {
        self.network_mode = mode;
        self
    }
}

/// Per-call mutation callbacks, mirroring `TanStack`'s `MutateOptions`.
///
/// These fire *last* in the callback order, after the mutation-level and
/// cache-level callbacks. Like every callback in the mutation pipeline they
/// return `()` and therefore cannot fail; each one is invoked independently so
/// one callback can never prevent the others from running.
pub struct MutateOptions<T, P> {
    /// Invoked after a successful mutation.
    pub on_success: Option<OnSuccessFn<T, P>>,
    /// Invoked after a failed mutation.
    pub on_error: Option<OnErrorFn<P>>,
    /// Invoked on both outcomes, after `on_success`/`on_error`.
    pub on_settled: Option<OnSettledFn<T, P>>,
}

impl<T, P> MutateOptions<T, P> {
    /// Options without any callback.
    #[must_use]
    pub fn new() -> Self {
        Self {
            on_success: None,
            on_error: None,
            on_settled: None,
        }
    }

    /// Set the per-call success callback.
    #[must_use]
    pub fn on_success<F>(mut self, f: F) -> Self
    where
        F: Fn(&T, &P) + Send + Sync + 'static,
    {
        self.on_success = Some(Arc::new(f));
        self
    }

    /// Set the per-call error callback.
    #[must_use]
    pub fn on_error<F>(mut self, f: F) -> Self
    where
        F: Fn(&QueryError, &P) + Send + Sync + 'static,
    {
        self.on_error = Some(Arc::new(f));
        self
    }

    /// Set the per-call settled callback.
    #[must_use]
    pub fn on_settled<F>(mut self, f: F) -> Self
    where
        F: Fn(Result<&T, &QueryError>, &P) + Send + Sync + 'static,
    {
        self.on_settled = Some(Arc::new(f));
        self
    }
}

impl<T, P> Clone for MutateOptions<T, P> {
    fn clone(&self) -> Self {
        Self {
            on_success: self.on_success.clone(),
            on_error: self.on_error.clone(),
            on_settled: self.on_settled.clone(),
        }
    }
}

impl<T, P> Default for MutateOptions<T, P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, P> std::fmt::Debug for MutateOptions<T, P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("MutateOptions");
        if self.on_success.is_some() {
            debug.field("on_success", &"<fn>");
        }
        if self.on_error.is_some() {
            debug.field("on_error", &"<fn>");
        }
        if self.on_settled.is_some() {
            debug.field("on_settled", &"<fn>");
        }
        debug.finish()
    }
}

/// Execute the full `TanStack` mutation lifecycle against a [`MutationCache`].
///
/// Ordering (`TanStack` parity):
///
/// 1. A cache entry is added with `Pending` status and `submitted_at` stamped
///    (a scoped mutation that cannot start immediately is added as paused).
/// 2. The cache-level `on_mutate` config callback fires with an entry
///    snapshot.
/// 3. If the mutation has a scope, wait until [`MutationCache::can_run`]
///    allows it, then mark the scope running and unpause the entry.
/// 4. The mutation-level `on_mutate` runs, producing the optimistic context
///    and rollback.
/// 5. `mutate_fn` runs with up to `mutation.retry` retries, delaying
///    `mutation.retry_delay` between attempts.
/// 6. Success: the invalidated keys are invalidated, the cache entry is
///    updated, then callbacks run in order: mutation `on_success` → cache
///    config `on_success` → per-call `on_success` → per-call `on_settled` →
///    mutation `on_settled` → cache config `on_settled`.
/// 7. Error: the rollback runs, the cache entry is updated, then callbacks run
///    in order: mutation `on_error` → cache config `on_error` → per-call
///    `on_error` → per-call `on_settled` → mutation `on_settled` → cache
///    config `on_settled`.
/// 8. Finally the scope is released and the next queued mutation of the scope
///    is resumed.
///
/// All callbacks are closures returning `()` and therefore cannot fail; each
/// is invoked unconditionally and independently, so one callback can never
/// prevent the later ones from running. Callback failures are therefore not
/// possible here (a panic inside a callback propagates like any panic).
///
/// # Errors
///
/// Returns the [`QueryError`] from the `on_mutate` hook (after the error-path
/// callbacks and rollback bookkeeping) or, when the mutate function exhausts
/// its retries, the last attempt's error.
pub async fn execute_mutation_full<T, P>(
    client: &QueryClient,
    cache: &MutationCache,
    mutation: &Mutation<T, P>,
    params: P,
    per_call: Option<MutateOptions<T, P>>,
) -> Result<T, QueryError>
where
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + Sync + 'static,
{
    execute_mutation_full_tracked(client, cache, mutation, params, per_call, None).await
}

/// [`execute_mutation_full`] with an observer hook: `on_entry_added` is invoked
/// with the assigned cache-entry id *before* the `Added` event is broadcast,
/// letting observers correlate cache events with their own execution race-free.
pub async fn execute_mutation_full_tracked<T, P>(
    client: &QueryClient,
    cache: &MutationCache,
    mutation: &Mutation<T, P>,
    params: P,
    per_call: Option<MutateOptions<T, P>>,
    on_entry_added: Option<&(dyn Fn(u64) + Send + Sync)>,
) -> Result<T, QueryError>
where
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + Sync + 'static,
{
    let params = Arc::new(params);

    // 1. Cache entry: Pending with submitted_at stamped. `add` pauses the
    //    entry when the scope is currently busy (TanStack: dispatch pending
    //    with isPaused before the gate).
    let entry = MutationCacheEntry {
        id: 0,
        mutation_key: mutation.mutation_key.clone(),
        scope: mutation.scope.clone(),
        status: MutationStatus::Pending,
        is_paused: false,
        submitted_at_ms: Some(now_ms()),
        has_result: false,
        error: None,
        has_context: false,
    };
    let entry_id = cache.add_with_hook(entry, on_entry_added);

    // 2. Cache-level on_mutate (runs before the option-level hook).
    cache.fire_on_mutate(entry_id);

    // 3. Scope gate: wait for the previous scoped mutation to finish.
    let scope_id = mutation.scope.as_ref().map(|scope| scope.id.clone());
    let mut scope_acquired = false;
    if let Some(scope) = &scope_id {
        // Park on the scope's release watcher instead of polling: the cache
        // bumps it on `mark_finished`/`run_next`.
        let mut release = cache.subscribe_scope(scope);
        loop {
            if cache.can_run(scope) {
                cache.mark_running(scope);
                scope_acquired = true;
                if cache.get(entry_id).is_some_and(|entry| entry.is_paused) {
                    cache.update(entry_id, |entry| entry.is_paused = false);
                }
                break;
            }
            // A closed watcher only happens if the cache dropped its release
            // cell (cache is alive here, so this cannot happen); re-check
            // either way.
            let _ = release.changed().await;
        }
    }

    // 4. Mutation-level on_mutate producing the optimistic context.
    let rollback: Option<RollbackFn> = match &mutation.on_mutate {
        Some(on_mutate) => match on_mutate(client, &*params) {
            Ok(rollback) => {
                cache.update(entry_id, |entry| entry.has_context = true);
                Some(rollback)
            }
            Err(error) => {
                // The onMutate failure takes the regular error path (no
                // rollback exists yet).
                settle_mutation_error(
                    cache,
                    mutation,
                    &params,
                    entry_id,
                    per_call.as_ref(),
                    &error,
                );
                release_scope(cache, scope_id.as_deref(), scope_acquired);
                return Err(error);
            }
        },
        None => None,
    };

    // 5. mutate_fn with retries (attempts = retry + 1).
    let mut retries_used = 0u32;
    let result = loop {
        match (mutation.mutate_fn)((*params).clone()).await {
            Ok(data) => break Ok(data),
            Err(err) => {
                if retries_used < mutation.retry {
                    retries_used += 1;
                    client.runtime().sleep(mutation.retry_delay).await;
                } else {
                    break Err(err);
                }
            }
        }
    };

    match result {
        Ok(data) => {
            settle_mutation_success(
                client,
                cache,
                mutation,
                &params,
                entry_id,
                per_call.as_ref(),
                &data,
            );
            release_scope(cache, scope_id.as_deref(), scope_acquired);
            Ok(data)
        }
        Err(error) => {
            if let Some(rollback) = rollback {
                rollback(client);
            }
            settle_mutation_error(
                cache,
                mutation,
                &params,
                entry_id,
                per_call.as_ref(),
                &error,
            );
            release_scope(cache, scope_id.as_deref(), scope_acquired);
            Err(error)
        }
    }
}

/// Complete a succeeded mutation (`TanStack` success path): invalidate the
/// configured keys, flip the cache entry to `Success`, then fire the
/// callbacks in order: mutation `on_success` → cache config `on_success` →
/// per-call `on_success` → per-call `on_settled` → mutation `on_settled` →
/// cache config `on_settled`.
fn settle_mutation_success<T, P>(
    client: &QueryClient,
    cache: &MutationCache,
    mutation: &Mutation<T, P>,
    params: &P,
    entry_id: u64,
    per_call: Option<&MutateOptions<T, P>>,
    data: &T,
) where
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + Sync + 'static,
{
    for key in &mutation.invalidates_keys {
        client.invalidate_queries(key);
    }
    cache.update(entry_id, |entry| {
        entry.status = MutationStatus::Success;
        entry.has_result = true;
        entry.error = None;
        entry.is_paused = false;
    });
    if let Some(on_success) = &mutation.on_success {
        on_success(data, params);
    }
    cache.fire_on_success(entry_id);
    if let Some(options) = per_call {
        if let Some(on_success) = &options.on_success {
            on_success(data, params);
        }
        if let Some(on_settled) = &options.on_settled {
            on_settled(Ok(data), params);
        }
    }
    if let Some(on_settled) = &mutation.on_settled {
        on_settled(Ok(data), params);
    }
    cache.fire_on_settled(entry_id);
}

/// Complete a failed mutation (`TanStack` error path): flip the cache entry
/// to `Error`, then fire the callbacks in order: mutation `on_error` → cache
/// config `on_error` → per-call `on_error` → per-call `on_settled` → mutation
/// `on_settled` → cache config `on_settled`.
///
/// The optimistic rollback (if any) is run by the caller *before* this
/// helper, mirroring `TanStack`'s rollback-then-error-callbacks order.
fn settle_mutation_error<T, P>(
    cache: &MutationCache,
    mutation: &Mutation<T, P>,
    params: &P,
    entry_id: u64,
    per_call: Option<&MutateOptions<T, P>>,
    error: &QueryError,
) where
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + Sync + 'static,
{
    cache.update(entry_id, |entry| {
        entry.status = MutationStatus::Error;
        entry.error = Some(error.clone());
        entry.has_result = false;
        entry.is_paused = false;
    });
    if let Some(on_error) = &mutation.on_error {
        on_error(error, params);
    }
    cache.fire_on_error(entry_id);
    if let Some(options) = per_call {
        if let Some(on_error) = &options.on_error {
            on_error(error, params);
        }
        if let Some(on_settled) = &options.on_settled {
            on_settled(Err(error), params);
        }
    }
    if let Some(on_settled) = &mutation.on_settled {
        on_settled(Err(error), params);
    }
    cache.fire_on_settled(entry_id);
}

/// Release a previously acquired scope and wake the next queued mutation.
fn release_scope(cache: &MutationCache, scope_id: Option<&str>, scope_acquired: bool) {
    if scope_acquired && let Some(scope) = scope_id {
        cache.mark_finished(scope);
        cache.run_next(scope);
    }
}

// src/mutation.rs (tests section only)
// src/mutation.rs (tests section only)
#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::MutationFilters;
    use crate::mutation_cache::{
        MutationCache, MutationCacheConfig, MutationCacheEntry, MutationCacheEvent, MutationScope,
    };
    use crate::state::MutationStatus;
    use crate::{QueryKey, QueryOptions};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    /// Append a message to a shared, ordered log.
    fn push_log(log: &Mutex<Vec<String>>, message: &str) {
        log.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message.to_string());
    }

    /// Read a log deterministically.
    fn read_log(log: &Mutex<Vec<String>>) -> Vec<String> {
        log.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Drain already-broadcast cache events without awaiting.
    fn drain_events(rx: &mut crate::bus::Receiver<MutationCacheEvent>) -> Vec<MutationCacheEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn test_mutation_creation() {
        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) });

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
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) })
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
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) }).on_mutate(
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
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) })
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
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) })
                .invalidates(vec![key1, key2]);

        assert_eq!(mutation.invalidates_keys.len(), 2);

        let result = (mutation.mutate_fn)(3).await;
        assert_eq!(result.unwrap(), "result_3");
    }

    #[tokio::test]
    async fn test_mutation_invalidates_vec_empty() {
        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) })
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
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) }).on_success(
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
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) }).on_error(
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
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) })
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
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) })
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
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) }).on_mutate(
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
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) });

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
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) }).on_mutate(
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
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) }).on_error(
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
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) })
                .invalidates_key(key1)
                .invalidates_key(key2);

        let cloned = mutation.clone();
        assert_eq!(cloned.invalidates_keys.len(), 2);

        let result = (cloned.mutate_fn)(6).await;
        assert_eq!(result.unwrap(), "result_6");
    }

    #[tokio::test]
    async fn test_mutation_fn_with_complex_params() {
        let mutation: Mutation<String, (i32, String)> =
            Mutation::new(|(id, name): (i32, String)| async move { Ok(format!("{id}-{name}")) });

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
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) }).on_error(
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
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) })
                .invalidates_key(key1)
                .invalidates_key(key2)
                .invalidates(vec![key3]);

        // invalidates() replaces, so only key3 remains
        assert_eq!(mutation.invalidates_keys.len(), 1);

        let result = (mutation.mutate_fn)(1).await;
        assert_eq!(result.unwrap(), "result_1");
    }

    // ---- Mutation option extensions ----

    #[tokio::test]
    async fn test_mutation_new_options_defaults() {
        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) });

        assert!(mutation.scope.is_none());
        assert_eq!(mutation.retry, 0);
        assert_eq!(mutation.retry_delay, Duration::from_secs(1));
        assert!(mutation.mutation_key.is_none());
        assert!(mutation.on_settled.is_none());
        assert_eq!(mutation.network_mode, NetworkMode::Online);

        let configured = mutation
            .clone()
            .scope(MutationScope::new("checkout"))
            .mutation_key(QueryKey::new("add-item"))
            .retry(3)
            .retry_delay(Duration::from_millis(5))
            .network_mode(NetworkMode::Always);

        assert_eq!(configured.scope, Some(MutationScope::new("checkout")));
        assert_eq!(configured.mutation_key, Some(QueryKey::new("add-item")));
        assert_eq!(configured.retry, 3);
        assert_eq!(configured.retry_delay, Duration::from_millis(5));
        assert_eq!(configured.network_mode, NetworkMode::Always);
    }

    #[tokio::test]
    async fn test_mutate_options_defaults_and_builders() {
        let options = MutateOptions::<String, i32>::new();
        assert!(options.on_success.is_none());
        assert!(options.on_error.is_none());
        assert!(options.on_settled.is_none());

        let built = options
            .on_success(|_data: &String, _params: &i32| {})
            .on_error(|_error: &QueryError, _params: &i32| {})
            .on_settled(|_result: Result<&String, &QueryError>, _params: &i32| {});
        assert!(built.on_success.is_some());
        assert!(built.on_error.is_some());
        assert!(built.on_settled.is_some());

        // Clone keeps every callback without requiring T/P: Clone.
        let cloned = built.clone();
        assert!(cloned.on_success.is_some());
        assert!(cloned.on_error.is_some());
        assert!(cloned.on_settled.is_some());
    }

    // ---- execute_mutation_full ----

    #[tokio::test]
    async fn test_execute_mutation_full_success_callback_order() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        client.set_query_data(&key, "cached".to_string(), QueryOptions::default());
        assert!(!client.is_stale(&key));

        let log = Arc::new(Mutex::new(Vec::new()));
        let config_success_log = Arc::clone(&log);
        let config_settled_log = Arc::clone(&log);
        let cache = MutationCache::with_config(MutationCacheConfig {
            on_success: Some(Arc::new(move |entry: &MutationCacheEntry| {
                push_log(&config_success_log, &format!("config_success:{}", entry.id));
            })),
            on_settled: Some(Arc::new(move |_entry: &MutationCacheEntry| {
                push_log(&config_settled_log, "config_settled");
            })),
            ..MutationCacheConfig::default()
        });

        let options_success_log = Arc::clone(&log);
        let options_settled_log = Arc::clone(&log);
        let mutation: Mutation<String, i32> =
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) })
                .invalidates_key(key.clone())
                .on_success(move |_data: &String, _params: &i32| {
                    push_log(&options_success_log, "options_success");
                })
                .on_settled(
                    move |_result: Result<&String, &QueryError>, _params: &i32| {
                        push_log(&options_settled_log, "options_settled");
                    },
                );

        let per_call_success_log = Arc::clone(&log);
        let per_call_settled_log = Arc::clone(&log);
        let per_call = MutateOptions::<String, i32>::new()
            .on_success(move |_data: &String, _params: &i32| {
                push_log(&per_call_success_log, "per_call_success");
            })
            .on_settled(
                move |_result: Result<&String, &QueryError>, _params: &i32| {
                    push_log(&per_call_settled_log, "per_call_settled");
                },
            );

        let result = execute_mutation_full(&client, &cache, &mutation, 7, Some(per_call)).await;
        assert_eq!(result.unwrap(), "result_7");

        // Task order: options on_success -> config on_success -> per-call
        // on_success -> per-call on_settled -> options on_settled -> config
        // on_settled.
        assert_eq!(
            read_log(&log),
            vec![
                "options_success".to_string(),
                "config_success:1".to_string(),
                "per_call_success".to_string(),
                "per_call_settled".to_string(),
                "options_settled".to_string(),
                "config_settled".to_string(),
            ]
        );

        // Existing behavior preserved: invalidation on success.
        assert!(client.is_stale(&key));

        let entry = cache.get(1).expect("entry recorded");
        assert_eq!(entry.status, MutationStatus::Success);
        assert!(entry.has_result);
        assert!(entry.submitted_at_ms.is_some());
        assert_eq!(entry.error, None);
    }

    #[tokio::test]
    async fn test_execute_mutation_full_error_callback_order_and_rollback() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        let log = Arc::new(Mutex::new(Vec::new()));

        let config_error_log = Arc::clone(&log);
        let config_settled_log = Arc::clone(&log);
        let cache = MutationCache::with_config(MutationCacheConfig {
            on_error: Some(Arc::new(move |entry: &MutationCacheEntry| {
                push_log(&config_error_log, &format!("config_error:{}", entry.id));
            })),
            on_settled: Some(Arc::new(move |_entry: &MutationCacheEntry| {
                push_log(&config_settled_log, "config_settled");
            })),
            ..MutationCacheConfig::default()
        });

        let key_for_optimistic = key.clone();
        let options_error_log = Arc::clone(&log);
        let options_settled_log = Arc::clone(&log);
        let mutation: Mutation<String, i32> =
            Mutation::new(|_param: i32| async { Err(QueryError::custom("boom")) })
                .on_mutate(move |client: &QueryClient, _params: &i32| {
                    client.set_query_data(
                        &key_for_optimistic,
                        "optimistic".to_string(),
                        QueryOptions::default(),
                    );
                    let key_for_rollback = key_for_optimistic.clone();
                    Ok(Box::new(move |client: &QueryClient| {
                        client.set_query_data(
                            &key_for_rollback,
                            "rolled_back".to_string(),
                            QueryOptions::default(),
                        );
                    }))
                })
                .on_error(move |_error: &QueryError, _params: &i32| {
                    push_log(&options_error_log, "options_error");
                })
                .on_settled(move |result: Result<&String, &QueryError>, _params: &i32| {
                    assert!(result.is_err());
                    push_log(&options_settled_log, "options_settled");
                });

        let per_call_error_log = Arc::clone(&log);
        let per_call_settled_log = Arc::clone(&log);
        let per_call = MutateOptions::<String, i32>::new()
            .on_error(move |error: &QueryError, _params: &i32| {
                assert_eq!(error.to_string(), "boom");
                push_log(&per_call_error_log, "per_call_error");
            })
            .on_settled(move |result: Result<&String, &QueryError>, _params: &i32| {
                assert!(result.is_err());
                push_log(&per_call_settled_log, "per_call_settled");
            });

        let result = execute_mutation_full(&client, &cache, &mutation, 1, Some(per_call)).await;
        assert_eq!(result.unwrap_err().to_string(), "boom");

        // Task order: options on_error -> config on_error -> per-call
        // on_error -> per-call on_settled -> options on_settled -> config
        // on_settled.
        assert_eq!(
            read_log(&log),
            vec![
                "options_error".to_string(),
                "config_error:1".to_string(),
                "per_call_error".to_string(),
                "per_call_settled".to_string(),
                "options_settled".to_string(),
                "config_settled".to_string(),
            ]
        );

        // Rollback restored the pre-mutation data.
        let data: Option<String> = client.get_query_data(&key);
        assert_eq!(data, Some("rolled_back".to_string()));

        let entry = cache.get(1).expect("entry recorded");
        assert_eq!(entry.status, MutationStatus::Error);
        assert_eq!(entry.error, Some(QueryError::custom("boom")));
        assert!(!entry.has_result);
        assert!(entry.has_context);
    }

    #[tokio::test]
    async fn test_execute_mutation_full_retry_failing_then_succeeding() {
        let client = QueryClient::new();
        let cache = MutationCache::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let mutation: Mutation<String, i32> = Mutation::new(move |_param: i32| {
            let attempts = Arc::clone(&attempts_clone);
            async move {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    Err(QueryError::custom("transient"))
                } else {
                    Ok("recovered".to_string())
                }
            }
        })
        .retry(1)
        .retry_delay(Duration::from_millis(1));

        let result = execute_mutation_full(&client, &cache, &mutation, 1, None).await;
        assert_eq!(result.unwrap(), "recovered");
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            2,
            "one retry after the failure"
        );
        assert_eq!(
            cache.get(1).map(|entry| entry.status),
            Some(MutationStatus::Success)
        );
    }

    #[tokio::test]
    async fn test_execute_mutation_full_no_retry_by_default() {
        let client = QueryClient::new();
        let cache = MutationCache::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let mutation: Mutation<String, i32> = Mutation::new(move |_param: i32| {
            let attempts = Arc::clone(&attempts_clone);
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err::<String, _>(QueryError::custom("fatal"))
            }
        });

        let result = execute_mutation_full(&client, &cache, &mutation, 1, None).await;
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1, "retry defaults to 0");
        assert_eq!(
            cache.get(1).map(|entry| entry.status),
            Some(MutationStatus::Error)
        );
    }

    #[tokio::test]
    async fn test_execute_mutation_full_on_mutate_error_takes_error_path() {
        let client = QueryClient::new();
        let cache = MutationCache::new();
        let error_calls = Arc::new(AtomicUsize::new(0));
        let error_calls_clone = Arc::clone(&error_calls);

        let mutation: Mutation<String, i32> =
            Mutation::new(|_param: i32| async { Ok("should not run".to_string()) })
                .on_mutate(|_client: &QueryClient, _params: &i32| {
                    Err(QueryError::custom("optimistic failed"))
                })
                .on_error(move |_error: &QueryError, _params: &i32| {
                    error_calls_clone.fetch_add(1, Ordering::SeqCst);
                });

        let result = execute_mutation_full(&client, &cache, &mutation, 1, None).await;
        assert_eq!(result.unwrap_err().to_string(), "optimistic failed");
        assert_eq!(
            error_calls.load(Ordering::SeqCst),
            1,
            "error path runs the callbacks"
        );

        let entry = cache.get(1).expect("entry recorded");
        assert_eq!(entry.status, MutationStatus::Error);
        assert!(
            !entry.has_context,
            "no context without a successful on_mutate"
        );
    }

    #[tokio::test]
    async fn test_execute_mutation_full_scope_waiting_is_serialized() {
        let client = QueryClient::new();
        let cache = Arc::new(MutationCache::new());
        let mut rx = cache.subscribe();

        let live = Arc::new(AtomicUsize::new(0));
        let max_live = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(tokio::sync::Notify::new());

        // First mutation parks inside its mutate_fn until released.
        let live_a = Arc::clone(&live);
        let max_a = Arc::clone(&max_live);
        let release_a = Arc::clone(&release);
        let first: Mutation<String, i32> = Mutation::new(move |_param: i32| {
            let live = Arc::clone(&live_a);
            let max_live = Arc::clone(&max_a);
            let release = Arc::clone(&release_a);
            async move {
                let current = live.fetch_add(1, Ordering::SeqCst) + 1;
                max_live.fetch_max(current, Ordering::SeqCst);
                release.notified().await;
                live.fetch_sub(1, Ordering::SeqCst);
                Ok("first".to_string())
            }
        })
        .scope(MutationScope::new("checkout"))
        .mutation_key(QueryKey::new("checkout"));

        // Second mutation is quick but must wait for the scope.
        let live_b = Arc::clone(&live);
        let max_b = Arc::clone(&max_live);
        let second: Mutation<String, i32> = Mutation::new(move |_param: i32| {
            let live = Arc::clone(&live_b);
            let max_live = Arc::clone(&max_b);
            async move {
                let current = live.fetch_add(1, Ordering::SeqCst) + 1;
                max_live.fetch_max(current, Ordering::SeqCst);
                live.fetch_sub(1, Ordering::SeqCst);
                Ok("second".to_string())
            }
        })
        .scope(MutationScope::new("checkout"))
        .mutation_key(QueryKey::new("checkout"));

        let first_handle = tokio::spawn({
            let cache = Arc::clone(&cache);
            let client = client.clone();
            async move { execute_mutation_full(&client, &cache, &first, 1, None).await }
        });
        // Let the first mutation acquire the scope and park in its mutate_fn.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let second_handle = tokio::spawn({
            let cache = Arc::clone(&cache);
            let client = client.clone();
            async move { execute_mutation_full(&client, &cache, &second, 2, None).await }
        });
        // Let the second mutation submit and pause behind the scope.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let paused = cache.find_all(&MutationFilters::all().paused());
        assert_eq!(paused.len(), 1, "the queued mutation is paused");
        assert_eq!(live.load(Ordering::SeqCst), 1, "only the first runs");

        release.notify_one();

        assert_eq!(first_handle.await.unwrap().unwrap(), "first");
        assert_eq!(second_handle.await.unwrap().unwrap(), "second");
        assert_eq!(
            max_live.load(Ordering::SeqCst),
            1,
            "scope serializes execution"
        );

        let events = drain_events(&mut rx);
        assert!(
            events.contains(&MutationCacheEvent::ScopeResumed("checkout".to_string())),
            "scope resume is broadcast: {events:?}"
        );
        assert!(
            cache
                .find_all(&MutationFilters::with_key(QueryKey::new("checkout")).exact())
                .iter()
                .all(|entry| entry.status == MutationStatus::Success)
        );
    }

    #[test]
    fn test_mutate_options_default_has_no_callbacks() {
        let default = MutateOptions::<String, i32>::default();
        assert!(default.on_success.is_none());
        assert!(default.on_error.is_none());
        assert!(default.on_settled.is_none());

        // Debug of an empty options object names the struct without callback
        // fields.
        let debug = format!("{default:?}");
        assert!(debug.contains("MutateOptions"), "{debug}");
        assert!(!debug.contains("<fn>"), "{debug}");
    }

    #[test]
    fn test_mutate_options_debug_lists_configured_callbacks() {
        let built = MutateOptions::<String, i32>::new()
            .on_success(|_data: &String, _params: &i32| {})
            .on_error(|_error: &QueryError, _params: &i32| {})
            .on_settled(|_result: Result<&String, &QueryError>, _params: &i32| {});

        let debug = format!("{built:?}");
        assert!(debug.contains("MutateOptions"), "{debug}");
        assert!(debug.contains("on_success"), "{debug}");
        assert!(debug.contains("on_error"), "{debug}");
        assert!(debug.contains("on_settled"), "{debug}");
        assert_eq!(debug.matches("<fn>").count(), 3, "{debug}");
    }
}
