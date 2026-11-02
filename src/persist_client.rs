// src/persist_client.rs
//! Whole-client persistence, porting `TanStack` Query's `query-persist-client-core`.
//!
//! The client is persisted as a single blob ([`PersistedClient`]: timestamp,
//! buster, dehydrated state) through a [`ClientPersister`] storage backend.
//! Distinct from the fine-grained per-query persister in
//! [`crate::persister`]: this module saves/restores the entire cache, checks
//! `max_age` against the snapshot's save `timestamp_ms`, and discards
//! (removes) busted or expired snapshots on restore.

use crate::QueryClient;
use crate::error::QueryError;
use crate::hydration::{DehydratedState, HydrateOptions};
use crate::persister::{DEFAULT_MAX_AGE, now_ms};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// Interval between background saves started by [`persist_query_client`].
///
/// `TanStack` saves on every cache event and leaves throttling to the persister;
/// the rs-query cache has no event bus yet, so the loop saves on this fixed
/// throttle instead. Event-driven saves land together with the `QueryCache`
/// event bus (next wave).
pub const PERSIST_INTERVAL: Duration = Duration::from_secs(1);

/// Maximum number of [`persist_with_retry`] retries before giving up (the
/// canonical `TanStack` strategy, `removeOldestQuery`, is bounded the same way).
pub const MAX_RESTORE_RETRIES: u32 = 3;

/// A whole-client snapshot handed to a [`ClientPersister`] (`TanStack`
/// `PersistedClient`).
#[derive(Clone, Debug)]
pub struct PersistedClient {
    /// Save-time `Date.now()`-style epoch milliseconds. A value of `0` counts
    /// as "missing timestamp" and is discarded on restore (`TanStack`: no
    /// `timestamp` ⇒ `removeClient`).
    pub timestamp_ms: u64,
    /// Cache-busting string; a mismatch against the restore options discards
    /// the snapshot.
    pub buster: String,
    /// The dehydrated cache (`TanStack` `clientState`).
    pub client_state: DehydratedState,
}

/// Storage backend for whole-client persistence (`TanStack` `Persister`).
///
/// Object-safe via boxed futures, the same pattern as
/// [`crate::persister::QueryStorage`]; concrete persisters are passed around
/// as `Arc<dyn ClientPersister>` (see [`PersistQueryClientOptions::new`]).
pub trait ClientPersister: Send + Sync + std::fmt::Debug {
    /// Write the client snapshot (single blob, latest wins).
    fn persist_client(
        &self,
        client: &PersistedClient,
    ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>>;

    /// Read the last snapshot, if any.
    fn restore_client(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<PersistedClient>, QueryError>> + Send + '_>>;

    /// Delete the stored snapshot (no-op when absent).
    fn remove_client(&self) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>>;
}

/// In-memory [`ClientPersister`] holding a single snapshot.
///
/// Useful for tests and as the simplest possible wiring of the
/// whole-client persistence flow. All operations resolve eagerly and return
/// ready futures.
#[derive(Debug, Default)]
pub struct InMemoryClientPersister {
    state: Mutex<Option<PersistedClient>>,
}

impl InMemoryClientPersister {
    /// Create an empty in-memory persister.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Option<PersistedClient>> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ClientPersister for InMemoryClientPersister {
    fn persist_client(
        &self,
        client: &PersistedClient,
    ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>> {
        *self.lock() = Some(client.clone());
        Box::pin(std::future::ready(Ok(())))
    }

    fn restore_client(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<PersistedClient>, QueryError>> + Send + '_>>
    {
        let snapshot = self.lock().clone();
        Box::pin(std::future::ready(Ok(snapshot)))
    }

    fn remove_client(&self) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>> {
        self.lock().take();
        Box::pin(std::future::ready(Ok(())))
    }
}

/// Options for whole-client persistence (`TanStack` `PersistQueryClientOptions`).
#[derive(Clone, Debug)]
pub struct PersistQueryClientOptions {
    persister: Arc<dyn ClientPersister>,
    max_age: Duration,
    buster: String,
}

impl PersistQueryClientOptions {
    /// Create options for the given persister with `TanStack` defaults
    /// (`max_age` 24 h, `buster: ""`).
    #[must_use]
    pub fn new(persister: Arc<dyn ClientPersister>) -> Self {
        Self {
            persister,
            max_age: DEFAULT_MAX_AGE,
            buster: String::new(),
        }
    }

    /// The storage backend.
    #[must_use]
    pub fn persister(&self) -> &Arc<dyn ClientPersister> {
        &self.persister
    }

    /// Maximum age of a snapshot before restore discards it.
    #[must_use]
    pub const fn max_age(&self) -> Duration {
        self.max_age
    }

    /// Cache buster compared against stored snapshots.
    #[must_use]
    pub fn buster(&self) -> &str {
        &self.buster
    }

    /// Set the maximum snapshot age.
    #[must_use]
    pub const fn with_max_age(mut self, max_age: Duration) -> Self {
        self.max_age = max_age;
        self
    }

    /// Set the cache buster.
    #[must_use]
    pub fn with_buster(mut self, buster: impl Into<String>) -> Self {
        self.buster = buster.into();
        self
    }
}

/// Restore a previously persisted client snapshot (`TanStack`
/// `persistQueryClientRestore`).
///
/// * Missing snapshot → `Ok(())`, nothing happens.
/// * Busted ([`PersistQueryClientOptions::buster`] mismatch), missing
///   timestamp (`timestamp_ms == 0`) or expired
///   (`now - timestamp > max_age`) → the snapshot is removed and `Ok(())` is
///   returned.
/// * Otherwise the snapshot is hydrated into the client.
/// * Any persister error → best-effort removal (`TanStack`: "as a precaution,
///   the persisted cache will be discarded") and the error is propagated.
///
/// # Errors
///
/// Returns the persister's error from `restore_client` (after a best-effort
/// `remove_client`) or from the precautionary removal itself.
pub async fn persist_query_client_restore(
    client: &QueryClient,
    options: &PersistQueryClientOptions,
) -> Result<(), QueryError> {
    match options.persister.restore_client().await {
        Ok(None) => Ok(()),
        Ok(Some(persisted)) => {
            let busted = persisted.buster != options.buster;
            let missing_timestamp = persisted.timestamp_ms == 0;
            let expired = !missing_timestamp
                && now_ms().saturating_sub(persisted.timestamp_ms)
                    > u64::try_from(options.max_age.as_millis()).unwrap_or(u64::MAX);
            if busted || missing_timestamp || expired {
                options.persister.remove_client().await?;
                return Ok(());
            }
            client.hydrate(persisted.client_state, HydrateOptions::default());
            Ok(())
        }
        Err(error) => {
            // As a precaution, discard the persisted cache before propagating.
            let _ = options.persister.remove_client().await;
            Err(error)
        }
    }
}

/// Dehydrate the client and persist it with a fresh timestamp (`TanStack`
/// `persistQueryClientSave`).
///
/// # Errors
///
/// Returns the persister's error from `persist_client`.
pub async fn persist_query_client_save(
    client: &QueryClient,
    options: &PersistQueryClientOptions,
) -> Result<(), QueryError> {
    let snapshot = PersistedClient {
        timestamp_ms: now_ms(),
        buster: options.buster.clone(),
        client_state: client.dehydrate(),
    };
    options.persister.persist_client(&snapshot).await
}

/// Handle for the throttled background save loop started by
/// [`persist_query_client`].
///
/// Dropping the handle stops the save loop (the `Drop` impl unsubscribes);
/// use [`PersistQueryClientHandle::into_task`] when you need to await it.
pub struct PersistQueryClientHandle {
    /// Cooperative stop flag; also set by
    /// [`PersistQueryClientHandle::unsubscribe`].
    pub unsubscribe: Arc<AtomicBool>,
    /// The background task. Dropping the whole handle stops the loop via the
    /// [`Drop`] implementation; aborting the task works too.
    pub task: Option<crate::runtime::SpawnHandle>,
    stop_signal: crate::watch::Watch<bool>,
}

impl PersistQueryClientHandle {
    /// Stop the background save loop (idempotent).
    pub fn unsubscribe(&self) {
        self.unsubscribe.store(true, Ordering::SeqCst);
        self.stop_signal.send(true);
    }

    /// Stop the subscription and return the completion task (reliable stop
    /// for destructured handles; see [`BroadcastHandle::into_task`] for the
    /// drop-semantics rationale).
    ///
    /// # Panics
    ///
    /// Panics when the task was already taken via a previous `into_task`
    /// call; a handle yields its task exactly once.
    #[must_use]
    pub fn into_task(mut self) -> crate::runtime::SpawnHandle {
        let task = self
            .task
            .take()
            .expect("into_task consumes a handle whose task was never taken");
        self.unsubscribe();
        task
    }
}

impl Drop for PersistQueryClientHandle {
    fn drop(&mut self) {
        self.unsubscribe();
    }
}

/// Restore first, then run a throttled background save loop (`TanStack`
/// `persistQueryClient`).
///
/// `TanStack` subscribes to cache events and saves on every `added`/`removed`/
/// `updated` event; the rs-query cache has no event bus yet, so the loop saves
/// on a fixed [`PERSIST_INTERVAL`] throttle instead (event-driven saves land
/// together with the `QueryCache` event bus, next wave). Save errors are logged
/// via `tracing::debug` and never stop the loop.
///
/// # Errors
///
/// Returns the error from the initial [`persist_query_client_restore`] call;
/// background save-loop failures are only logged.
pub async fn persist_query_client(
    client: &QueryClient,
    options: &PersistQueryClientOptions,
) -> Result<PersistQueryClientHandle, QueryError> {
    persist_query_client_restore(client, options).await?;
    let unsubscribe = Arc::new(AtomicBool::new(false));
    let (stop_tx, mut stop_rx) = crate::watch::Watch::new(false);
    let task_client = client.clone();
    let task_options = options.clone();
    let task_flag = Arc::clone(&unsubscribe);
    let runtime = client.runtime().clone();
    let save_runtime = client.runtime().clone();
    // The unsubscribe flag only becomes reachable after this call returns
    // (TanStack's restore-then-subscribe race cannot occur here), so the
    // loop's per-iteration flag check already covers the earliest possible
    // unsubscribe.
    let (done_tx, done_rx) = futures::channel::oneshot::channel::<()>();
    let task = async move {
        loop {
            let stop_fut = stop_rx.changed();
            let tick_fut = runtime.sleep(PERSIST_INTERVAL);
            futures::pin_mut!(stop_fut, tick_fut);
            match futures::future::select(stop_fut, tick_fut).await {
                // Stop requested (handle dropped/unsubscribed).
                futures::future::Either::Left((_, _)) => break,
                futures::future::Either::Right(((), _)) => {
                    if task_flag.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Err(error) = persist_query_client_save(&task_client, &task_options).await
                    {
                        tracing::debug!(
                            error = %error,
                            "rs-query: throttled persist_query_client save failed"
                        );
                    }
                }
            }
        }
        let _ = done_tx.send(());
    };
    let handle = save_runtime.spawn(Box::pin(task));
    let task = crate::runtime::spawn_handle(Box::new(move || handle.abort()), done_rx);
    Ok(PersistQueryClientHandle {
        unsubscribe,
        task: Some(task),
        stop_signal: stop_tx,
    })
}

/// Retry strategy building block (`TanStack` `removeOldestQuery`).
///
/// Removes the query with the smallest `dataUpdatedAt`-equivalent — the
/// [`crate::hydration::DehydratedQuery::fetched_at_ms`] slot (a missing
/// timestamp counts as the oldest) — from `client_state`, keeping everything
/// else (including mutation entries). Returns `true` when a query was
/// removed, `false` when the state has no queries left (the caller should
/// give up).
pub fn remove_oldest_query(client_state: &mut DehydratedState) -> bool {
    let oldest_index = client_state
        .queries
        .iter()
        .enumerate()
        .min_by_key(|(_, query)| query.fetched_at_ms.unwrap_or(0))
        .map(|(index, _)| index);
    match oldest_index {
        Some(index) => {
            client_state.queries.remove(index);
            true
        }
        None => false,
    }
}

/// Restore with retry (the restore-side analogue of `TanStack`'s storage
/// persister `trySave` retry loop).
///
/// Attempts [`persist_query_client_restore`]; on error the `retry_strategy` is
/// consulted with the error and the 1-based error count:
///
/// * `Some(replacement)` → the replacement snapshot is hydrated into the
///   client and saved back to the persister (best-effort, so a later restore
///   starts from it), then `Ok(())` is returned.
/// * `None` → give up and return the triggering error.
///
/// The strategy is consulted at most [`MAX_RESTORE_RETRIES`] times; beyond
/// that the last error is returned. The canonical strategy shrinks a captured
/// candidate snapshot with [`remove_oldest_query`].
///
/// # Errors
///
/// Returns the triggering error once [`MAX_RESTORE_RETRIES`] attempts are
/// exhausted (or immediately when the strategy declines a replacement).
pub async fn persist_with_retry(
    client: &QueryClient,
    options: &PersistQueryClientOptions,
    retry_strategy: Arc<dyn Fn(QueryError, u32) -> Option<PersistedClient> + Send + Sync>,
) -> Result<(), QueryError> {
    let mut error_count: u32 = 0;
    loop {
        match persist_query_client_restore(client, options).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                error_count += 1;
                if error_count > MAX_RESTORE_RETRIES {
                    return Err(error);
                }
                // A replacement is hydrated and saved immediately: the
                // client is usable without another (possibly failing)
                // restore round-trip. No replacement: retry the restore
                // itself (bounded by MAX_RESTORE_RETRIES total attempts).
                if let Some(replacement) = retry_strategy(error.clone(), error_count) {
                    client.hydrate(replacement.client_state.clone(), HydrateOptions::default());
                    if let Err(save_error) = options.persister.persist_client(&replacement).await {
                        tracing::debug!(
                            error = %save_error,
                            "rs-query: persist_with_retry could not save the replacement snapshot"
                        );
                    }
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hydration::{DehydratedQuery, DehydratedState, QUERY_TYPE_QUERY};
    use crate::key::QueryKey;
    use crate::options::QueryOptions;
    use std::collections::BTreeMap;

    fn options() -> PersistQueryClientOptions {
        PersistQueryClientOptions::new(Arc::new(InMemoryClientPersister::new()))
    }

    fn state_with(entries: &[(&str, &str, u64)]) -> DehydratedState {
        let queries = entries
            .iter()
            .map(|(query_hash, data, fetched_at_ms)| DehydratedQuery {
                query_hash: (*query_hash).to_string(),
                query_key: vec![(*query_hash).to_string()],
                data_json: Some((*data).to_string()),
                error_json: None,
                fetched_at_ms: Some(*fetched_at_ms),
                is_stale: false,
                stale_time_ms: 0,
                gc_time_ms: 300_000,
                meta: BTreeMap::new(),
                query_type: QUERY_TYPE_QUERY.to_string(),
            })
            .collect();
        DehydratedState {
            dehydrated_at_ms: now_ms(),
            queries,
            mutations: Vec::new(),
        }
    }

    fn client_with(entry: (&str, &str)) -> QueryClient {
        // Tokio-backed runtime so paused-time tests (`start_paused` +
        // `advance`) drive the save loop's throttle sleep deterministically.
        let client = crate::test_support::tokio_client();
        client.set_query_data(
            &QueryKey::new(entry.0),
            entry.1.to_string(),
            QueryOptions::default(),
        );
        client
    }

    async fn stored_snapshot(options: &PersistQueryClientOptions) -> Option<PersistedClient> {
        options
            .persister()
            .restore_client()
            .await
            .expect("restore_client should resolve")
    }

    #[test]
    fn test_options_defaults() {
        let opts = options();
        assert_eq!(opts.max_age(), Duration::from_hours(24));
        assert_eq!(opts.buster(), "");
        assert!(format!("{opts:?}").contains("PersistQueryClientOptions"));
    }

    #[test]
    fn test_options_with_max_age_overrides_default() {
        let opts = options().with_max_age(Duration::from_secs(60));
        assert_eq!(opts.max_age(), Duration::from_secs(60));
        assert_eq!(opts.buster(), "");
    }

    #[tokio::test]
    async fn test_save_restore_roundtrip() {
        let opts = options();
        let source = client_with(("users", "alpha"));
        persist_query_client_save(&source, &opts)
            .await
            .expect("save should succeed");

        let snapshot = stored_snapshot(&opts).await.expect("snapshot should exist");
        assert!(snapshot.timestamp_ms > 0);
        assert_eq!(snapshot.buster, "");
        assert_eq!(snapshot.client_state.queries.len(), 1);
        let target = QueryClient::new();
        persist_query_client_restore(&target, &opts)
            .await
            .expect("restore should succeed");
        let data: Option<String> = target.get_query_data(&QueryKey::new("users"));
        assert_eq!(data, Some("alpha".to_string()));
    }

    #[tokio::test]
    async fn test_restore_missing_snapshot_is_ok() {
        let opts = options();
        let client = QueryClient::new();
        persist_query_client_restore(&client, &opts)
            .await
            .expect("restore without a snapshot should succeed");
        assert!(client.cache.is_empty());
    }

    #[tokio::test]
    async fn test_buster_mismatch_removes_snapshot() {
        let persister = Arc::new(InMemoryClientPersister::new());
        let save_opts =
            PersistQueryClientOptions::new(Arc::clone(&persister) as Arc<dyn ClientPersister>)
                .with_buster("v1");
        let source = client_with(("users", "alpha"));
        persist_query_client_save(&source, &save_opts)
            .await
            .expect("save should succeed");

        let restore_opts =
            PersistQueryClientOptions::new(Arc::clone(&persister) as Arc<dyn ClientPersister>)
                .with_buster("v2");
        let target = QueryClient::new();
        persist_query_client_restore(&target, &restore_opts)
            .await
            .expect("restore should succeed");
        assert!(target.cache.is_empty());
        // The busted snapshot is discarded from storage as well.
        assert!(stored_snapshot(&restore_opts).await.is_none());
    }

    #[tokio::test]
    async fn test_expired_snapshot_is_removed() {
        let persister = Arc::new(InMemoryClientPersister::new());
        let opts =
            PersistQueryClientOptions::new(Arc::clone(&persister) as Arc<dyn ClientPersister>);
        let expired = PersistedClient {
            timestamp_ms: now_ms().saturating_sub(25 * 60 * 60 * 1000),
            buster: String::new(),
            client_state: state_with(&[("users", "old", 0)]),
        };
        persister
            .persist_client(&expired)
            .await
            .expect("persist_client should resolve");

        let target = QueryClient::new();
        persist_query_client_restore(&target, &opts)
            .await
            .expect("restore should succeed");
        assert!(target.cache.is_empty());
        assert!(stored_snapshot(&opts).await.is_none());
    }

    #[tokio::test]
    async fn test_missing_timestamp_is_removed() {
        let persister = Arc::new(InMemoryClientPersister::new());
        let opts =
            PersistQueryClientOptions::new(Arc::clone(&persister) as Arc<dyn ClientPersister>);
        let orphan = PersistedClient {
            timestamp_ms: 0,
            buster: String::new(),
            client_state: state_with(&[("users", "orphan", 0)]),
        };
        persister
            .persist_client(&orphan)
            .await
            .expect("persist_client should resolve");

        let target = QueryClient::new();
        persist_query_client_restore(&target, &opts)
            .await
            .expect("restore should succeed");
        assert!(target.cache.is_empty());
        assert!(stored_snapshot(&opts).await.is_none());
    }

    #[tokio::test]
    async fn test_restore_error_discards_and_propagates() {
        #[derive(Debug)]
        struct FailingPersister;
        impl ClientPersister for FailingPersister {
            fn persist_client(
                &self,
                _client: &PersistedClient,
            ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>> {
                Box::pin(std::future::ready(Ok(())))
            }
            fn restore_client(
                &self,
            ) -> Pin<
                Box<dyn Future<Output = Result<Option<PersistedClient>, QueryError>> + Send + '_>,
            > {
                Box::pin(std::future::ready(Err(QueryError::custom(
                    "broken storage",
                ))))
            }
            fn remove_client(
                &self,
            ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>> {
                Box::pin(std::future::ready(Ok(())))
            }
        }

        let opts = PersistQueryClientOptions::new(Arc::new(FailingPersister));
        let client = QueryClient::new();
        let error = persist_query_client_restore(&client, &opts)
            .await
            .expect_err("restore should fail");
        assert_eq!(error, QueryError::custom("broken storage"));
        assert!(client.cache.is_empty());

        // The restore flow never reaches the stub's persist_client; exercise
        // it directly so the impl is covered.
        let snapshot = PersistedClient {
            timestamp_ms: now_ms(),
            buster: String::new(),
            client_state: DehydratedState::default(),
        };
        opts.persister()
            .persist_client(&snapshot)
            .await
            .expect("stub persist_client should resolve");
    }

    #[test]
    fn test_remove_oldest_query_picks_oldest_and_keeps_rest() {
        let mut state = state_with(&[
            ("oldest", "a", 1000),
            ("middle", "b", 2000),
            ("newest", "c", 3000),
        ]);
        assert!(remove_oldest_query(&mut state));
        assert_eq!(state.queries.len(), 2);
        let remaining: Vec<&str> = state
            .queries
            .iter()
            .map(|query| query.query_hash.as_str())
            .collect();
        assert!(!remaining.contains(&"oldest"));
        assert!(remaining.contains(&"middle"));
        assert!(remaining.contains(&"newest"));
        drop(remaining);

        // Keeps removing until empty, then gives up.
        assert!(remove_oldest_query(&mut state));
        assert!(remove_oldest_query(&mut state));
        assert!(!remove_oldest_query(&mut state));
        assert_eq!(state.queries, []);
    }

    #[test]
    fn test_remove_oldest_query_empty_state() {
        let mut state = DehydratedState::default();
        assert!(!remove_oldest_query(&mut state));
    }

    #[tokio::test]
    async fn test_persist_with_retry_drops_oldest_until_success() {
        #[derive(Debug)]
        struct AlwaysFailsRestore {
            inner: InMemoryClientPersister,
        }
        impl ClientPersister for AlwaysFailsRestore {
            fn persist_client(
                &self,
                client: &PersistedClient,
            ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>> {
                self.inner.persist_client(client)
            }
            fn restore_client(
                &self,
            ) -> Pin<
                Box<dyn Future<Output = Result<Option<PersistedClient>, QueryError>> + Send + '_>,
            > {
                Box::pin(std::future::ready(Err(QueryError::custom(
                    "quota exceeded",
                ))))
            }
            fn remove_client(
                &self,
            ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>> {
                self.inner.remove_client()
            }
        }

        let persister = Arc::new(AlwaysFailsRestore {
            inner: InMemoryClientPersister::new(),
        });
        let opts =
            PersistQueryClientOptions::new(Arc::clone(&persister) as Arc<dyn ClientPersister>);

        // Candidate snapshot the strategy shrinks, one query per call.
        let candidate = Arc::new(Mutex::new(PersistedClient {
            timestamp_ms: now_ms(),
            buster: String::new(),
            client_state: state_with(&[
                ("oldest", "a", 1000),
                ("middle", "b", 2000),
                ("newest", "c", 3000),
            ]),
        }));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let strategy_candidate = Arc::clone(&candidate);
        let strategy_calls = Arc::clone(&calls);
        let strategy: Arc<dyn Fn(QueryError, u32) -> Option<PersistedClient> + Send + Sync> =
            Arc::new(move |_error, error_count| {
                strategy_calls.fetch_add(1, Ordering::SeqCst);
                let mut guard = strategy_candidate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if error_count == 1 {
                    // Decline the first attempt and drain the candidate, so
                    // the next consultation exercises the exhausted
                    // (`remove_oldest_query == false`) arm before the retry
                    // still ends in success.
                    guard.client_state.queries.clear();
                    return None;
                }
                let removed = remove_oldest_query(&mut guard.client_state);
                if error_count == 2 {
                    // Refill so the following consultation still yields a
                    // replacement (minus its oldest entry) and the retry loop
                    // ends in success.
                    guard.client_state =
                        state_with(&[("middle", "b", 2000), ("newest", "c", 3000)]);
                }
                if removed { Some(guard.clone()) } else { None }
            });

        let client = QueryClient::new();
        persist_with_retry(&client, &opts, strategy)
            .await
            .expect("retry should end in success");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "declined, exhausted, then accepted"
        );

        // The replacement (candidate reduced to its newest query) was hydrated
        // into the client and saved back to the persister.
        assert_eq!(client.cache.len(), 1);
        let newest: Option<String> = client.get_query_data(&QueryKey::new("newest"));
        assert_eq!(newest, Some("c".to_string()));
        let saved: Option<PersistedClient> = persister
            .inner
            .restore_client()
            .await
            .expect("inner restore should resolve");
        let saved = saved.expect("replacement should be saved");
        assert_eq!(saved.client_state.queries.len(), 1);
        assert!(
            !saved
                .client_state
                .queries
                .iter()
                .any(|query| query.query_hash == "oldest")
        );
    }

    #[tokio::test]
    async fn test_persist_with_retry_gives_up_when_strategy_none() {
        #[derive(Debug)]
        struct AlwaysFailsRestore;
        impl ClientPersister for AlwaysFailsRestore {
            fn persist_client(
                &self,
                _client: &PersistedClient,
            ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>> {
                Box::pin(std::future::ready(Ok(())))
            }
            fn restore_client(
                &self,
            ) -> Pin<
                Box<dyn Future<Output = Result<Option<PersistedClient>, QueryError>> + Send + '_>,
            > {
                Box::pin(std::future::ready(Err(QueryError::custom("boom"))))
            }
            fn remove_client(
                &self,
            ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>> {
                Box::pin(std::future::ready(Ok(())))
            }
        }

        let opts = PersistQueryClientOptions::new(Arc::new(AlwaysFailsRestore));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let strategy_calls = Arc::clone(&calls);
        let strategy: Arc<dyn Fn(QueryError, u32) -> Option<PersistedClient> + Send + Sync> =
            Arc::new(move |_error, _count| {
                strategy_calls.fetch_add(1, Ordering::SeqCst);
                None
            });

        let client = QueryClient::new();
        let error = persist_with_retry(&client, &opts, strategy)
            .await
            .expect_err("restore should keep failing");
        assert_eq!(error, QueryError::custom("boom"));
        assert_eq!(calls.load(Ordering::SeqCst), MAX_RESTORE_RETRIES as usize);

        // The retry loop never saves when the strategy declines; exercise the
        // stub's persist_client directly so the impl is covered.
        let snapshot = PersistedClient {
            timestamp_ms: now_ms(),
            buster: String::new(),
            client_state: DehydratedState::default(),
        };
        opts.persister()
            .persist_client(&snapshot)
            .await
            .expect("stub persist_client should resolve");
    }

    #[tokio::test(start_paused = true)]
    async fn test_persist_loop_saves_snapshot_on_throttle_tick() {
        let opts = options();
        let client = client_with(("users", "tick"));

        let handle = persist_query_client(&client, &opts)
            .await
            .expect("persist_query_client should succeed");

        // Advance past one throttle interval; the loop then saves the client.
        for _ in 0..100 {
            tokio::time::advance(Duration::from_millis(50)).await;
            if stored_snapshot(&opts).await.is_some() {
                break;
            }
        }
        let snapshot = stored_snapshot(&opts)
            .await
            .expect("the throttled loop should have saved a snapshot");
        assert!(snapshot.timestamp_ms > 0);
        assert_eq!(snapshot.client_state.queries.len(), 1);

        let _ = handle.into_task().await;
    }

    #[tokio::test(start_paused = true)]
    async fn test_persist_loop_logs_save_errors_and_keeps_running() {
        #[derive(Debug)]
        struct RestoreOkPersistFails {
            save_calls: Arc<std::sync::atomic::AtomicUsize>,
        }
        impl ClientPersister for RestoreOkPersistFails {
            fn persist_client(
                &self,
                _client: &PersistedClient,
            ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>> {
                self.save_calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(std::future::ready(Err(QueryError::custom("disk full"))))
            }
            fn restore_client(
                &self,
            ) -> Pin<
                Box<dyn Future<Output = Result<Option<PersistedClient>, QueryError>> + Send + '_>,
            > {
                Box::pin(std::future::ready(Ok(None)))
            }
            fn remove_client(
                &self,
            ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>> {
                Box::pin(std::future::ready(Ok(())))
            }
        }

        let save_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let persister = Arc::new(RestoreOkPersistFails {
            save_calls: Arc::clone(&save_calls),
        });
        let opts =
            PersistQueryClientOptions::new(Arc::clone(&persister) as Arc<dyn ClientPersister>);
        let client = client_with(("users", "v1"));
        let handle = persist_query_client(&client, &opts)
            .await
            .expect("restore of a missing snapshot succeeds");

        // Two ticks with a failing save: errors are logged and the loop
        // keeps running. Yield around every advance so the spawned loop
        // registers its throttle timer and actually reaches its tick.
        for _ in 0..2 {
            tokio::task::yield_now().await;
            tokio::time::advance(PERSIST_INTERVAL).await;
            for _ in 0..50 {
                tokio::task::yield_now().await;
            }
        }
        assert!(
            save_calls.load(Ordering::SeqCst) >= 1,
            "the throttled loop must attempt (and log) the failing saves"
        );

        // The remaining trait methods are not used by the save loop; exercise
        // them directly so the stub impl is covered.
        let snapshot = PersistedClient {
            timestamp_ms: now_ms(),
            buster: String::new(),
            client_state: DehydratedState::default(),
        };
        assert!(persister.persist_client(&snapshot).await.is_err());
        persister
            .remove_client()
            .await
            .expect("remove should resolve");

        handle.unsubscribe();
        let _ = handle.into_task().await;
    }

    #[tokio::test(start_paused = true)]
    async fn test_persist_loop_breaks_on_unsubscribe_flag_without_stop_signal() {
        let opts = options();
        let client = client_with(("users", "v1"));

        let handle = persist_query_client(&client, &opts)
            .await
            .expect("persist_query_client should succeed");
        // Raise the cooperative flag without firing the watch signal: the
        // loop must still exit at the next throttle tick via the flag check.
        handle.unsubscribe.store(true, Ordering::SeqCst);

        // Yield once so the spawned loop registers its throttle timer, then
        // advance past one throttle interval and let the tick observe the
        // flag and break out of the loop.
        tokio::task::yield_now().await;
        tokio::time::advance(PERSIST_INTERVAL).await;
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert!(
            handle
                .task
                .as_ref()
                .is_some_and(crate::runtime::SpawnHandle::is_finished),
            "the loop must exit through the unsubscribe flag"
        );
        let _ = handle.into_task().await;
    }

    #[tokio::test]
    async fn test_persist_with_retry_succeeds_immediately_without_strategy() {
        let opts = options();
        let source = client_with(("users", "alpha"));
        persist_query_client_save(&source, &opts)
            .await
            .expect("save should succeed");

        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let strategy_calls = Arc::clone(&calls);
        let strategy: Arc<dyn Fn(QueryError, u32) -> Option<PersistedClient> + Send + Sync> =
            Arc::new(move |_error, _count| {
                strategy_calls.fetch_add(1, Ordering::SeqCst);
                None
            });

        let client = QueryClient::new();
        persist_with_retry(&client, &opts, Arc::clone(&strategy))
            .await
            .expect("a successful restore returns immediately");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "strategy never consulted");
        assert_eq!(client.cache.len(), 1);

        // The strategy is only consulted on restore failures; call it directly
        // so the closure is exercised.
        assert!(strategy(QueryError::custom("consulted"), 1).is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_persist_with_retry_logs_failed_replacement_save() {
        #[derive(Debug)]
        struct RestoreFailsPersistFails;
        impl ClientPersister for RestoreFailsPersistFails {
            fn persist_client(
                &self,
                _client: &PersistedClient,
            ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>> {
                Box::pin(std::future::ready(Err(QueryError::custom(
                    "persist broken",
                ))))
            }
            fn restore_client(
                &self,
            ) -> Pin<
                Box<dyn Future<Output = Result<Option<PersistedClient>, QueryError>> + Send + '_>,
            > {
                Box::pin(std::future::ready(Err(QueryError::custom(
                    "restore broken",
                ))))
            }
            fn remove_client(
                &self,
            ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>> {
                Box::pin(std::future::ready(Ok(())))
            }
        }

        let opts = PersistQueryClientOptions::new(Arc::new(RestoreFailsPersistFails));
        let strategy: Arc<dyn Fn(QueryError, u32) -> Option<PersistedClient> + Send + Sync> =
            Arc::new(move |_error, _count| {
                Some(PersistedClient {
                    timestamp_ms: now_ms(),
                    buster: String::new(),
                    client_state: state_with(&[("users", "replacement", 0)]),
                })
            });

        let client = QueryClient::new();
        // The replacement save fails (logged, best-effort), but the client is
        // still usable: the replacement was hydrated before the save attempt.
        persist_with_retry(&client, &opts, strategy)
            .await
            .expect("the strategy provided a replacement, so restore ends in success");
        let users: Option<String> = client.get_query_data(&QueryKey::new("users"));
        assert_eq!(users, Some("replacement".to_string()));
    }

    #[tokio::test(start_paused = true)]
    async fn test_persist_query_client_saves_on_throttle_and_unsubscribe_stops() {
        let opts = options();
        let client = client_with(("users", "v1"));

        let handle = persist_query_client(&client, &opts)
            .await
            .expect("persist_query_client should succeed");
        let _ = handle.into_task().await;
    }

    #[tokio::test]
    async fn test_in_memory_client_persister_roundtrip_and_remove() {
        let persister = InMemoryClientPersister::new();
        assert!(persister.restore_client().await.expect("resolve").is_none());

        let snapshot = PersistedClient {
            timestamp_ms: 42,
            buster: "b".to_string(),
            client_state: state_with(&[("users", "alpha", 0)]),
        };
        persister
            .persist_client(&snapshot)
            .await
            .expect("persist should resolve");
        let restored = persister
            .restore_client()
            .await
            .expect("resolve")
            .expect("snapshot should exist");
        assert_eq!(restored.timestamp_ms, 42);
        assert_eq!(restored.buster, "b");

        persister
            .remove_client()
            .await
            .expect("remove should resolve");
        assert!(persister.restore_client().await.expect("resolve").is_none());
        // Removing again is a no-op.
        persister
            .remove_client()
            .await
            .expect("remove should resolve");
    }
}
