// src/mutation_observer.rs
//! Mutation observer, mirroring `TanStack` Query's `mutationObserver.ts`.
//!
//! Owns a single [`Mutation`] definition, executes it against the
//! [`MutationCache`] and tracks the resulting [`MutationState`] so UI code can
//! read the current result without re-subscribing by hand. Every `mutate`
//! execution creates its own cache entry; the observer correlates the entry's
//! cache events via [`MutationObserver::subscribe_filtered`].

use crate::QueryClient;
use crate::bus;
use crate::error::QueryError;
use crate::mutation::{MutateOptions, Mutation, execute_mutation_full_tracked};
use crate::mutation_cache::{MutationCache, MutationCacheEvent};
use crate::runtime::SpawnedTask;
use crate::state::MutationState;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Stream of cache events filtered to the entries created by one observer.
///
/// Events for unrelated mutations are skipped; `ScopeResumed` and `Cleared`
/// are global and always delivered.
pub struct MutationEventStream {
    rx: bus::Receiver<MutationCacheEvent>,
    entry_ids: Arc<Mutex<Vec<u64>>>,
}

impl MutationEventStream {
    pub(crate) fn new(
        rx: bus::Receiver<MutationCacheEvent>,
        entry_ids: Arc<Mutex<Vec<u64>>>,
    ) -> Self {
        Self { rx, entry_ids }
    }

    /// Wait for the next event belonging to one of this observer's mutation
    /// entries. Returns `None` once the cache is dropped.
    pub async fn next(&mut self) -> Option<MutationCacheEvent> {
        loop {
            match self.rx.recv().await {
                Ok(event) => {
                    let relevant = match &event {
                        MutationCacheEvent::Added(id)
                        | MutationCacheEvent::Removed(id)
                        | MutationCacheEvent::Updated(id) => self
                            .entry_ids
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .contains(id),
                        MutationCacheEvent::ScopeResumed(_) | MutationCacheEvent::Cleared => true,
                    };
                    if relevant {
                        return Some(event);
                    }
                }
                Err(bus::RecvError::Lagged(_)) => {}
                Err(bus::RecvError::Closed) => return None,
            }
        }
    }
}

/// Tracks one [`Mutation`] definition against a [`MutationCache`], mirroring
/// `TanStack`'s `MutationObserver`.
///
/// The observer stores the mutation state internally, so `current_result` is
/// a snapshot taken under the state lock (interior mutability prevents
/// returning a long-lived reference). `reset` clears the tracked state but
/// never cancels an in-flight execution.
pub struct MutationObserver<T, P> {
    cache: Arc<MutationCache>,
    mutation: Mutation<T, P>,
    state: Arc<Mutex<MutationState<T>>>,
    entry_ids: Arc<Mutex<Vec<u64>>>,
    generation: Arc<AtomicU64>,
}

impl<T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static> MutationObserver<T, P> {
    /// Observe `mutation` against `cache`.
    #[must_use]
    pub fn new(cache: Arc<MutationCache>, mutation: Mutation<T, P>) -> Self {
        Self {
            cache,
            mutation,
            state: Arc::new(Mutex::new(MutationState::Idle)),
            entry_ids: Arc::new(Mutex::new(Vec::new())),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Execute the mutation on the client's runtime.
    ///
    /// The tracked state switches to [`MutationState::Loading`] immediately and
    /// to `Success`/`Error` when the spawned execution settles. The returned
    /// handle resolves with the mutation result (`Err(cancelled)` when the
    /// task is aborted first). Dropping the handle without awaiting or
    /// detaching aborts the execution (structured concurrency).
    pub fn mutate(
        &self,
        client: &QueryClient,
        params: P,
        per_call: Option<MutateOptions<T, P>>,
    ) -> SpawnedTask<Result<T, QueryError>> {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = MutationState::Loading;

        let cache = Arc::clone(&self.cache);
        let mutation = self.mutation.clone();
        let state = Arc::clone(&self.state);
        let entry_ids = Arc::clone(&self.entry_ids);
        let generation = Arc::clone(&self.generation);
        let client = client.clone();
        // Capture the reset-generation at submit time: a later `reset` must
        // suppress this execution's state write.
        let observed_generation = self.generation.load(Ordering::SeqCst);
        let runtime = client.runtime().clone();

        let (tx, rx) = futures::channel::oneshot::channel();
        let task = async move {
            let on_entry_added = |id: u64| {
                entry_ids
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(id);
            };
            let result = execute_mutation_full_tracked(
                &client,
                &cache,
                &mutation,
                params,
                per_call,
                Some(&on_entry_added),
            )
            .await;
            // Do not clobber a state that was reset while we were in flight.
            if generation.load(Ordering::SeqCst) == observed_generation {
                *state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = match &result {
                    Ok(data) => MutationState::Success(data.clone()),
                    Err(error) => MutationState::Error(error.clone()),
                };
            }
            let _ = tx.send(result);
        };
        let handle = runtime.spawn(Box::pin(task));
        SpawnedTask::new(handle, rx, Arc::new(|| Err(QueryError::cancelled())))
    }

    /// Clear the tracked state back to [`MutationState::Idle`].
    ///
    /// An in-flight execution is NOT cancelled; its completion will not
    /// overwrite the reset state, but its handle still yields the result.
    pub fn reset(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = MutationState::Idle;
    }

    /// Snapshot of the tracked mutation state.
    #[must_use]
    pub fn current_result(&self) -> MutationState<T> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Subscribe to every cache event (unfiltered).
    #[must_use]
    pub fn subscribe(&self) -> bus::Receiver<MutationCacheEvent> {
        self.cache.subscribe()
    }

    /// Subscribe to a stream filtered to the cache entries created by this
    /// observer's executions.
    #[must_use]
    pub fn subscribe_filtered(&self) -> MutationEventStream {
        MutationEventStream::new(self.cache.subscribe(), Arc::clone(&self.entry_ids))
    }

    /// Id of the most recent cache entry created by this observer, if any.
    #[must_use]
    pub fn last_entry_id(&self) -> Option<u64> {
        self.entry_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::QueryKey;
    use std::time::Duration;

    fn client() -> QueryClient {
        crate::test_support::tokio_client()
    }

    #[tokio::test]
    async fn test_new_starts_idle() {
        let cache = Arc::new(MutationCache::new());
        let observer = MutationObserver::<String, i32>::new(
            Arc::clone(&cache),
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) }),
        );
        assert!(observer.current_result().is_idle());
        assert_eq!(observer.last_entry_id(), None);
    }

    #[tokio::test]
    async fn test_mutate_updates_current_result() {
        let cache = Arc::new(MutationCache::new());
        let observer = MutationObserver::<String, i32>::new(
            Arc::clone(&cache),
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) }),
        );
        let handle = observer.mutate(&client(), 42, None);

        // Loading is recorded synchronously, before the handle resolves.
        assert!(observer.current_result().is_loading());

        let data = handle.await.expect("succeeds");
        assert_eq!(data, "result_42");

        let result = observer.current_result();
        assert!(result.is_success());
        assert_eq!(result.data(), Some(&"result_42".to_string()));
        assert_eq!(observer.last_entry_id(), Some(1));
        assert_eq!(
            cache.get(1).map(|entry| entry.status),
            Some(crate::state::MutationStatus::Success)
        );
    }

    #[tokio::test]
    async fn test_mutate_error_updates_current_result() {
        let cache = Arc::new(MutationCache::new());
        let observer = MutationObserver::<String, i32>::new(
            Arc::clone(&cache),
            Mutation::new(|_param: i32| async move { Err(QueryError::custom("boom")) }),
        );
        let handle = observer.mutate(&client(), 1, None);
        assert!(observer.current_result().is_loading());

        let error = handle.await.unwrap_err();
        assert_eq!(error.to_string(), "boom");

        let result = observer.current_result();
        assert!(result.is_error());
        assert_eq!(
            result.error().map(ToString::to_string),
            Some("boom".to_string())
        );
    }

    #[tokio::test]
    async fn test_reset_clears_state_but_not_in_flight() {
        let cache = Arc::new(MutationCache::new());
        let mut observer = MutationObserver::<String, i32>::new(
            Arc::clone(&cache),
            Mutation::new(|param: i32| async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(format!("slow_{param}"))
            }),
        );

        let handle = observer.mutate(&client(), 7, None);
        observer.reset();
        assert!(observer.current_result().is_idle());

        // The in-flight execution is not cancelled and still resolves.
        let data = handle.await.expect("succeeds");
        assert_eq!(data, "slow_7");
        // The late completion must not clobber the reset state.
        assert!(observer.current_result().is_idle());
    }

    #[tokio::test]
    async fn test_reset_then_mutate_tracks_again() {
        let cache = Arc::new(MutationCache::new());
        let mut observer = MutationObserver::<String, i32>::new(
            Arc::clone(&cache),
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) }),
        );
        let first = observer.mutate(&client(), 1, None).await.unwrap();
        assert_eq!(first, "result_1");

        observer.reset();
        assert!(observer.current_result().is_idle());

        let second = observer.mutate(&client(), 2, None).await.unwrap();
        assert_eq!(second, "result_2");
        assert_eq!(
            observer.current_result().data(),
            Some(&"result_2".to_string())
        );
        assert_eq!(observer.last_entry_id(), Some(2));
    }

    #[tokio::test]
    async fn test_subscribe_delivers_cache_events() {
        let cache = Arc::new(MutationCache::new());
        let observer = MutationObserver::<String, i32>::new(
            Arc::clone(&cache),
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) }),
        );
        let mut rx = observer.subscribe();

        let handle = observer.mutate(&client(), 3, None);
        handle.await.expect("succeeds");

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(events.contains(&MutationCacheEvent::Added(1)));
        assert!(events.contains(&MutationCacheEvent::Updated(1)));
    }

    #[tokio::test]
    async fn test_subscribe_filtered_tracks_own_entries_only() {
        let cache = Arc::new(MutationCache::new());

        // A foreign mutation pollutes the cache with its own entry.
        let foreign: Mutation<String, i32> =
            Mutation::new(|_param: i32| async { Ok("foreign".to_string()) });
        let foreign_client = client();
        let foreign_cache = Arc::clone(&cache);
        let _foreign_handle = tokio::spawn(async move {
            execute_mutation_full_tracked(&foreign_client, &foreign_cache, &foreign, 0, None, None)
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;

        let observer = MutationObserver::<String, i32>::new(
            Arc::clone(&cache),
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) })
                .mutation_key(QueryKey::new("own"))
                .on_mutate(|_client: &QueryClient, _params: &i32| {
                    Ok(Box::new(|_client: &QueryClient| {}))
                }),
        );
        let mut stream = observer.subscribe_filtered();

        let handle = observer.mutate(&client(), 1, None);
        let first = stream.next().await.expect("Added event");
        assert!(
            matches!(first, MutationCacheEvent::Added(id) if Some(id) == observer.last_entry_id()),
            "first filtered event is the observer's own Added: {first:?}"
        );

        handle.await.expect("succeeds");
        let mut seen_updated = 0;
        while let Some(event) = stream.next().await {
            match event {
                MutationCacheEvent::Updated(id) => {
                    assert_eq!(Some(id), observer.last_entry_id());
                    seen_updated += 1;
                    if seen_updated == 2 {
                        break;
                    }
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert_eq!(seen_updated, 2, "context + success updates");
    }

    #[tokio::test]
    async fn test_filtered_stream_skips_foreign_events_and_delivers_global_ones() {
        let cache = Arc::new(MutationCache::new());
        let observer = MutationObserver::<String, i32>::new(
            Arc::clone(&cache),
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) }),
        );
        let mut stream = observer.subscribe_filtered();

        // A foreign entry is submitted before the observer's own execution:
        // its events are buffered first and must be skipped.
        let foreign = cache.add(crate::mutation_cache::MutationCacheEntry::new(
            Some(QueryKey::new("foreign")),
            None,
        ));
        let handle = observer.mutate(&client(), 1, None);
        handle.await.expect("succeeds");
        let own = observer.last_entry_id().expect("entry assigned");

        let first = stream.next().await.expect("own Added event");
        assert!(
            matches!(first, MutationCacheEvent::Added(id) if id == own),
            "foreign Added({foreign}) is skipped, own Added({own}) delivered: {first:?}"
        );

        // The settle update is the next relevant event for this observer.
        let settled = stream.next().await.expect("own Updated event");
        assert!(
            matches!(settled, MutationCacheEvent::Updated(id) if id == own),
            "own settle update delivered: {settled:?}"
        );

        // Removed events follow the same filter: the foreign removal is
        // skipped, the own removal is delivered.
        assert!(cache.remove(foreign));
        assert!(cache.remove(own));
        let second = stream.next().await.expect("own Removed event");
        assert!(
            matches!(second, MutationCacheEvent::Removed(id) if id == own),
            "foreign removal skipped, own removal delivered: {second:?}"
        );

        // Global events are always delivered.
        cache.mark_running("solo");
        let queued = cache.add(crate::mutation_cache::MutationCacheEntry::new(
            None,
            Some(crate::mutation_cache::MutationScope::new("solo")),
        ));
        assert!(cache.run_next("solo").is_some());
        let third = stream.next().await.expect("global ScopeResumed event");
        assert!(
            matches!(&third, MutationCacheEvent::ScopeResumed(scope) if scope.as_str() == "solo"),
            "queued Added({queued}) skipped, ScopeResumed delivered: {third:?}"
        );

        // Dropping every sender closes the stream.
        drop(cache);
        drop(observer);
        assert!(stream.next().await.is_none(), "closed stream yields None");
    }

    #[tokio::test]
    async fn test_filtered_stream_survives_lagged_events() {
        let cache = Arc::new(MutationCache::new());
        let observer = MutationObserver::<String, i32>::new(
            Arc::clone(&cache),
            Mutation::new(|param: i32| async move { Ok(format!("result_{param}")) }),
        );
        let mut stream = observer.subscribe_filtered();

        // Flood the channel far beyond its capacity without reading: the
        // stream lags, drops the missed events and keeps following.
        for _ in 0..300 {
            let _ = cache.add(crate::mutation_cache::MutationCacheEntry::new(None, None));
        }
        let handle = observer.mutate(&client(), 9, None);
        handle.await.expect("succeeds");
        let own = observer.last_entry_id().expect("entry assigned");

        let first = stream.next().await.expect("event after lag");
        assert!(
            matches!(first, MutationCacheEvent::Added(id) if id == own),
            "lag is absorbed and the observer's own Added event surfaces: {first:?}"
        );
    }
}
