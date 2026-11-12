// src/persister.rs
//! Fine-grained, per-query persistence, porting `TanStack` Query's
//! `experimental_createQueryPersister`.
//!
//! This is a distinct mechanism from the whole-client blob persister in
//! [`crate::persist_client`]: every query is its own storage entry keyed by
//! `` `{prefix}-{query_hash}` ``, expiry is checked against the stored
//! `data_updated_at_ms` (`TanStack`'s `state.dataUpdatedAt`), and restoring
//! happens per query with `TanStack`'s "restore only when the query has no data
//! yet" rule.
//!
//! Data in [`QueryClient`](crate::QueryClient) is type-erased, so payloads are
//! stored as JSON strings behind the `serde` feature; without that feature
//! every persisting operation fails gracefully with a serialization error.

use crate::QueryClient;
use crate::error::QueryError;
use crate::key::QueryKey;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// `TanStack`'s `maxAge` default for persisted entries: 24 hours.
pub const DEFAULT_MAX_AGE: Duration = Duration::from_hours(24);

#[cfg(feature = "serde")]
use crate::options::QueryOptions;

/// Default storage-key prefix (`TanStack` uses `tanstack-query`).
pub const PERSISTER_KEY_PREFIX: &str = "rs-query";

/// Current UNIX-epoch milliseconds (`0` only if the clock is before the epoch,
/// which count as "missing timestamp" and are treated as expired).
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Boxed future returned by [`QueryStorage::entries`].
///
/// Same object-safety pattern as the other [`QueryStorage`] methods: the
/// storage may not borrow beyond the returned future, and guards must not be
/// held across await points.
pub type StorageEntriesFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<(String, String)>, QueryError>> + Send + 'a>>;

/// Async key-value storage abstraction for per-query persistence.
///
/// Object-safe by construction: every method returns a boxed future, the same
/// pattern as [`crate::timeout_manager::SleepProvider`], so concrete storages
/// are passed around as `Arc<dyn QueryStorage>` (see
/// [`QueryPersisterOptions::new`]). `Send`, `Sync` and `Debug` are supertraits,
/// which makes the trait object itself `Send + Sync + Debug`.
pub trait QueryStorage: Send + Sync + std::fmt::Debug {
    /// Read the raw value stored under `key`, if any.
    fn get(
        &self,
        key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, QueryError>> + Send + '_>>;

    /// Write `value` under `key` (upsert).
    fn set(
        &self,
        key: String,
        value: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>>;

    /// Delete the entry stored under `key` (no-op when absent).
    fn remove(
        &self,
        key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>>;

    /// Snapshot of every `(key, value)` pair in the storage.
    ///
    /// `TanStack` declares this as the optional `entries()` on `AsyncStorage`;
    /// it is required here because [`QueryPersister::persister_gc`] needs to
    /// enumerate entries.
    fn entries(&self) -> StorageEntriesFuture<'_>;

    /// Access the storage as `Any` for downcasting (tests/diagnostics).
    fn as_any(&self) -> &dyn std::any::Any;
}

/// In-memory [`QueryStorage`] backed by a concurrent [`dashmap::DashMap`].
///
/// Intended for tests and for wiring the persister before a real storage
/// backend exists. All operations resolve eagerly and return ready futures
/// (`DashMap` guards are not `Send`, so none may live inside the futures).
#[derive(Debug, Default, Clone)]
pub struct InMemoryQueryStorage {
    map: dashmap::DashMap<String, String>,
}

impl InMemoryQueryStorage {
    /// Create an empty in-memory storage.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl QueryStorage for InMemoryQueryStorage {
    fn get(
        &self,
        key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, QueryError>> + Send + '_>> {
        let value = self.map.get(key).map(|entry| entry.value().clone());
        Box::pin(std::future::ready(Ok(value)))
    }

    fn set(
        &self,
        key: String,
        value: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>> {
        self.map.insert(key, value);
        Box::pin(std::future::ready(Ok(())))
    }

    fn remove(
        &self,
        key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), QueryError>> + Send + '_>> {
        self.map.remove(key);
        Box::pin(std::future::ready(Ok(())))
    }

    fn entries(&self) -> StorageEntriesFuture<'_> {
        let mut pairs: Vec<(String, String)> = self
            .map
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        pairs.sort_by(|left, right| left.0.cmp(&right.0));
        Box::pin(std::future::ready(Ok(pairs)))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Data payload of a single persisted query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PersistedQueryState {
    /// JSON-serialized query data (see the module docs for the `serde` gating).
    pub data_json: Option<String>,
    /// `Date.now()`-style epoch milliseconds of the last data update; the
    /// expiry check compares this against [`QueryPersisterOptions::max_age`].
    pub data_updated_at_ms: Option<u64>,
    /// Serialized error message, when the query errored.
    pub error: Option<String>,
}

/// A single persisted query entry (`TanStack` `PersistedQuery`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PersistedQuery {
    /// Cache buster recorded at save time; restore discards on mismatch.
    pub buster: String,
    /// Hash of the query key ([`QueryKey::cache_key`]).
    pub query_hash: String,
    /// Raw query-key segments.
    pub query_key: Vec<String>,
    /// The persisted state payload.
    pub state: PersistedQueryState,
}

/// Serialize a [`PersistedQuery`] to a JSON string.
///
/// Requires the `serde` feature.
///
/// # Errors
///
/// Returns [`QueryError::Serialization`] when the payload cannot be
/// serialized.
#[cfg(feature = "serde")]
pub fn serialize_persisted_query(query: &PersistedQuery) -> Result<String, QueryError> {
    serde_json::to_string(query).map_err(|err| QueryError::serialization(err.to_string()))
}

/// Deserialize a [`PersistedQuery`] from a JSON string.
///
/// Requires the `serde` feature.
///
/// # Errors
///
/// Returns [`QueryError::Serialization`] when the payload cannot be parsed
/// into a [`PersistedQuery`].
#[cfg(feature = "serde")]
pub fn deserialize_persisted_query(json: &str) -> Result<PersistedQuery, QueryError> {
    serde_json::from_str(json).map_err(|err| QueryError::serialization(err.to_string()))
}

/// Options for the per-query [`QueryPersister`] (`TanStack`
/// `StoragePersisterOptions`).
#[derive(Clone, Debug)]
pub struct QueryPersisterOptions {
    storage: Arc<dyn QueryStorage>,
    buster: String,
    max_age: Duration,
    prefix: String,
    refetch_on_restore: bool,
    stale_time: Option<Duration>,
}

impl QueryPersisterOptions {
    /// Create options for the given storage, with `TanStack` defaults
    /// (`buster: ""`, `max_age` 24 h, `prefix` [`PERSISTER_KEY_PREFIX`],
    /// `refetch_on_restore: true`, no explicit `stale_time`).
    #[must_use]
    pub fn new(storage: Arc<dyn QueryStorage>) -> Self {
        Self {
            storage,
            buster: String::new(),
            max_age: DEFAULT_MAX_AGE,
            prefix: PERSISTER_KEY_PREFIX.to_string(),
            refetch_on_restore: true,
            stale_time: None,
        }
    }

    /// The storage backend.
    #[must_use]
    pub fn storage(&self) -> &Arc<dyn QueryStorage> {
        &self.storage
    }

    /// Cache buster compared against stored entries.
    #[must_use]
    pub fn buster(&self) -> &str {
        &self.buster
    }

    /// Maximum age of an entry before it counts as expired.
    #[must_use]
    pub const fn max_age(&self) -> Duration {
        self.max_age
    }

    /// Storage-key prefix; keys are `` `{prefix}-{query_hash}` ``.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Whether restored entries may be marked stale (which triggers refetches).
    #[must_use]
    pub const fn refetch_on_restore(&self) -> bool {
        self.refetch_on_restore
    }

    /// Optional staleness budget applied to restored data.
    #[must_use]
    pub const fn stale_time(&self) -> Option<Duration> {
        self.stale_time
    }

    /// Set the cache buster.
    #[must_use]
    pub fn with_buster(mut self, buster: impl Into<String>) -> Self {
        self.buster = buster.into();
        self
    }

    /// Set the maximum entry age.
    #[must_use]
    pub const fn with_max_age(mut self, max_age: Duration) -> Self {
        self.max_age = max_age;
        self
    }

    /// Set the storage-key prefix.
    #[must_use]
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Set `refetch_on_restore`.
    #[must_use]
    pub const fn with_refetch_on_restore(mut self, enabled: bool) -> Self {
        self.refetch_on_restore = enabled;
        self
    }

    /// Set the staleness budget applied to restored data.
    #[must_use]
    pub const fn with_stale_time(mut self, stale_time: Option<Duration>) -> Self {
        self.stale_time = stale_time;
        self
    }
}

/// Fine-grained per-query persister (port of `TanStack`'s
/// `experimental_createQueryPersister`).
///
/// Clone freely: the storage behind the options is shared.
#[derive(Clone, Debug)]
pub struct QueryPersister {
    options: QueryPersisterOptions,
    storage: Arc<dyn QueryStorage>,
}

impl QueryPersister {
    /// Build a persister from the given options; the storage is shared between
    /// the options and the persister.
    #[must_use]
    pub fn new(options: QueryPersisterOptions) -> Self {
        Self {
            storage: Arc::clone(&options.storage),
            options,
        }
    }

    /// The persister options.
    #[must_use]
    pub fn options(&self) -> &QueryPersisterOptions {
        &self.options
    }

    /// The storage backend (cloned out of the options).
    #[must_use]
    pub fn storage(&self) -> &Arc<dyn QueryStorage> {
        &self.storage
    }

    /// Storage key for a query hash: `` `{prefix}-{query_hash}` ``
    /// (`TanStack`: `` `${prefix}-${queryHash}` ``).
    #[must_use]
    pub fn storage_key(&self, query_hash: &str) -> String {
        format!("{}-{}", self.options.prefix, query_hash)
    }

    /// Expiry/buster check for a parsed entry (`TanStack` `isExpiredOrBusted`):
    /// a buster mismatch, a missing `data_updated_at_ms`, or an entry older
    /// than `max_age` counts as unusable.
    #[cfg(feature = "serde")]
    fn is_expired_or_busted(&self, query: &PersistedQuery, now: u64) -> bool {
        if query.buster != self.options.buster {
            return true;
        }
        match query.state.data_updated_at_ms {
            None => true,
            Some(updated_at) => {
                now.saturating_sub(updated_at)
                    > u64::try_from(self.options.max_age.as_millis()).unwrap_or(u64::MAX)
            }
        }
    }

    /// Persist a single query entry (`TanStack` `persistQuery`).
    ///
    /// Reads the query's `String`-typed data from the client cache (the
    /// crate's dehydratable type, see `QueryClient::dehydrate`) and writes a
    /// [`PersistedQuery`] under `` `{prefix}-{query_hash}` ``. Queries without
    /// data — or with non-`String` data — are silently skipped (`TanStack` skips
    /// `undefined` data the same way).
    ///
    /// Without the `serde` feature this returns a serialization error.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::Serialization`] without `serde`; otherwise the
    /// storage backend's error when writing the payload fails.
    pub async fn persist_query(
        &self,
        client: &QueryClient,
        key: &QueryKey,
    ) -> Result<(), QueryError> {
        #[cfg(not(feature = "serde"))]
        {
            let _ = (client, key);
            Err(QueryError::serialization("serde feature required"))
        }
        #[cfg(feature = "serde")]
        {
            let Some(data) = client.get_query_data::<String>(key) else {
                return Ok(());
            };
            let data_json = serde_json::to_string(&data)
                .map_err(|err| QueryError::serialization(err.to_string()))?;
            let cache_key = key.cache_key();
            let persisted = PersistedQuery {
                buster: self.options.buster.clone(),
                query_hash: cache_key.to_string(),
                query_key: key.segments().to_vec(),
                state: PersistedQueryState {
                    data_json: Some(data_json),
                    data_updated_at_ms: Some(now_ms()),
                    error: None,
                },
            };
            let payload = serialize_persisted_query(&persisted)?;
            self.storage.set(self.storage_key(cache_key), payload).await
        }
    }

    /// Persist one query by key — alias of [`QueryPersister::persist_query`],
    /// kept for `TanStack` API parity (`persistQueryByKey`).
    ///
    /// # Errors
    ///
    /// Same conditions as [`QueryPersister::persist_query`].
    pub async fn persist_query_by_key(
        &self,
        client: &QueryClient,
        key: &QueryKey,
    ) -> Result<(), QueryError> {
        self.persist_query(client, key).await
    }

    /// Read, validate and hydrate one entry (`TanStack` `retrieveQuery`).
    ///
    /// Returns the deserialized data, or `None` when the entry is missing,
    /// expired or busted. Corrupted entries (unparseable JSON or a payload
    /// that does not deserialize into `T`) are removed from storage, mirroring
    /// `TanStack`'s `removeItem` on corruption. Unlike
    /// [`QueryPersister::restore_query`] this hydrates unconditionally
    /// (overwrites existing cache data), matching `TanStack`'s `restoreQueries`
    /// `setQueryData` semantics.
    ///
    /// # Errors
    ///
    /// Returns the storage backend's read/remove errors and deserialization
    /// errors for the outer [`PersistedQuery`] payload. A payload whose inner
    /// data does not fit `T` is *not* an error: the entry is removed and
    /// `Ok(None)` is returned, mirroring `TanStack`'s corrupt-entry handling.
    #[cfg(feature = "serde")]
    pub async fn retrieve_query<T>(
        &self,
        client: &QueryClient,
        key: &QueryKey,
    ) -> Result<Option<T>, QueryError>
    where
        T: Clone + Send + Sync + PartialEq + 'static + serde::de::DeserializeOwned,
    {
        let cache_key = key.cache_key();
        let storage_key = self.storage_key(cache_key);
        let Some(raw) = self.storage.get(&storage_key).await? else {
            return Ok(None);
        };
        let persisted = match deserialize_persisted_query(&raw) {
            Ok(parsed) => parsed,
            Err(error) => {
                tracing::debug!(
                    cache_key = %cache_key,
                    error = %error,
                    "rs-query persister: dropping corrupted entry"
                );
                self.storage.remove(&storage_key).await?;
                return Ok(None);
            }
        };
        if self.is_expired_or_busted(&persisted, now_ms()) {
            return Ok(None);
        }
        let Some(data_json) = persisted.state.data_json else {
            return Ok(None);
        };
        match serde_json::from_str::<T>(&data_json) {
            Ok(data) => {
                self.apply_restored_data(client, key, &data, persisted.state.data_updated_at_ms);
                Ok(Some(data))
            }
            Err(error) => {
                tracing::debug!(
                    cache_key = %cache_key,
                    error = %error,
                    "rs-query persister: dropping entry with unusable payload"
                );
                self.storage.remove(&storage_key).await?;
                Ok(None)
            }
        }
    }

    /// Hydrate restored data into the client, preserving `dataUpdatedAt`
    /// semantics as far as the cache allows.
    ///
    /// `QueryClient::set_query_data` stamps `fetched_at = now`, so the
    /// original age is preserved by deciding staleness once, here:
    ///
    /// * `refetch_on_restore == false` (`TanStack` "never refetch") → the entry
    ///   stays fresh and nothing triggers a refetch.
    /// * `refetch_on_restore == true` (`TanStack` "refetch if stale") → the
    ///   entry is marked stale when the persisted data is at least the
    ///   effective stale time old; with the default `stale_time` of zero every
    ///   restored entry is immediately stale (`TanStack`'s default `staleTime`).
    #[cfg(feature = "serde")]
    fn apply_restored_data<T>(
        &self,
        client: &QueryClient,
        key: &QueryKey,
        data: &T,
        data_updated_at_ms: Option<u64>,
    ) where
        T: Clone + Send + Sync + PartialEq + 'static,
    {
        let effective_stale_time = self.options.stale_time.unwrap_or(Duration::ZERO);
        client.set_query_data(
            key,
            data.clone(),
            QueryOptions {
                stale_time: effective_stale_time,
                ..QueryOptions::default()
            },
        );
        // `>=` matches TanStack's staleness comparison (`dataUpdatedAt +
        // staleTime <= now`), so a zero stale time makes even
        // same-millisecond data stale; a missing timestamp counts as stale.
        let is_stale = self.options.refetch_on_restore
            && data_updated_at_ms.is_none_or(|updated_at| {
                now_ms().saturating_sub(updated_at)
                    >= u64::try_from(effective_stale_time.as_millis()).unwrap_or(u64::MAX)
            });
        if is_stale && let Some(mut entry) = client.cache.get_mut(key.cache_key()) {
            entry.is_stale = true;
        }
    }

    /// Restore a query from storage into the client (the `persisterFn` model).
    ///
    /// * When the client already holds `T`-typed data for the key it is
    ///   returned unchanged and storage is not consulted (`TanStack`: restore
    ///   only when `query.state.data === undefined` — the caller proceeds
    ///   without fetching).
    /// * Otherwise the entry is read, validated (buster + max age) and
    ///   hydrated into the client; `Ok(None)` means "no usable persisted data
    ///   — run the real fetch".
    ///
    /// # Errors
    ///
    /// Propagates [`QueryPersister::retrieve_query`]'s errors.
    #[cfg(feature = "serde")]
    pub async fn restore_query<T>(
        &self,
        client: &QueryClient,
        key: &QueryKey,
    ) -> Result<Option<T>, QueryError>
    where
        T: Clone
            + Send
            + Sync
            + PartialEq
            + 'static
            + serde::de::DeserializeOwned
            + serde::Serialize,
    {
        if let Some(existing) = client.get_query_data::<T>(key) {
            return Ok(Some(existing));
        }
        self.retrieve_query::<T>(client, key).await
    }

    /// Remove expired, busted and corrupted entries owned by this persister's
    /// prefix (`TanStack` `persisterGc`).
    ///
    /// Returns the number of removed storage entries. As a side effect the
    /// in-memory copies of removed keys are invalidated on the client: their
    /// persisted snapshot is gone, so the next observer should refetch.
    ///
    /// Without the `serde` feature this returns a serialization error.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::Serialization`] without `serde`; otherwise the
    /// storage backend's errors while enumerating or removing entries.
    pub async fn persister_gc(&self, client: &QueryClient) -> Result<usize, QueryError> {
        #[cfg(not(feature = "serde"))]
        {
            let _ = client;
            Err(QueryError::serialization("serde feature required"))
        }
        #[cfg(feature = "serde")]
        {
            let prefix = format!("{}-", self.options.prefix);
            let entries = self.storage.entries().await?;
            let now = now_ms();
            let mut removed = 0usize;
            for (storage_key, raw) in entries {
                if !storage_key.starts_with(&prefix) {
                    continue;
                }
                let drop_entry = match deserialize_persisted_query(&raw) {
                    Err(_) => true,
                    Ok(persisted) => self.is_expired_or_busted(&persisted, now),
                };
                if drop_entry {
                    self.storage.remove(&storage_key).await?;
                    removed += 1;
                    if let Some(query_hash) = storage_key.strip_prefix(&prefix) {
                        client.invalidate_queries(&QueryKey::from_segments([query_hash]));
                    }
                }
            }
            Ok(removed)
        }
    }

    /// Bulk-restore the given keys into the client (`TanStack` `restoreQueries`).
    ///
    /// Returns how many keys were found, valid and restored. Valid entries
    /// overwrite existing cache data; corrupted entries are removed; expired
    /// or busted entries are skipped and left for
    /// [`QueryPersister::persister_gc`]. Entries restore as `String` data (the
    /// crate's dehydratable type).
    ///
    /// # Errors
    ///
    /// Propagates the storage backend's errors from
    /// [`QueryPersister::retrieve_query`] for the first failing key.
    #[cfg(feature = "serde")]
    pub async fn restore_queries(
        &self,
        client: &QueryClient,
        keys: &[QueryKey],
    ) -> Result<usize, QueryError> {
        let mut restored = 0usize;
        for key in keys {
            if self.retrieve_query::<String>(client, key).await?.is_some() {
                restored += 1;
            }
        }
        Ok(restored)
    }

    /// Remove the given keys from storage and from the client cache (`TanStack`
    /// `removeQueries`).
    ///
    /// Returns how many keys had at least one artifact removed (a storage
    /// entry or a client cache entry).
    ///
    /// # Errors
    ///
    /// Propagates the storage backend's read/remove errors for the first
    /// failing key.
    pub async fn remove_queries(
        &self,
        client: &QueryClient,
        keys: &[QueryKey],
    ) -> Result<usize, QueryError> {
        let mut removed = 0usize;
        for key in keys {
            let cache_key = key.cache_key();
            let storage_key = self.storage_key(cache_key);
            let in_storage = self.storage.get(&storage_key).await?.is_some();
            self.storage.remove(&storage_key).await?;
            let in_client = client.cache.remove(cache_key).is_some();
            if in_storage || in_client {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::QueryOptions;
    use std::sync::Arc;

    #[allow(dead_code)]
    const HOUR_MS: u64 = 60 * 60 * 1000;

    fn test_persister() -> QueryPersister {
        QueryPersister::new(QueryPersisterOptions::new(Arc::new(
            InMemoryQueryStorage::new(),
        )))
    }

    fn key(id: u32) -> QueryKey {
        QueryKey::new("users").with("id", id)
    }

    #[allow(dead_code)]
    async fn stored_entry_count(persister: &QueryPersister) -> usize {
        let prefix = format!("{}-", persister.options().prefix());
        persister
            .storage()
            .entries()
            .await
            .expect("entries should resolve")
            .into_iter()
            .filter(|(storage_key, _)| storage_key.starts_with(&prefix))
            .count()
    }

    /// Write a crafted entry with an explicit `data_updated_at_ms`.
    ///
    /// Requires the `serde` feature: persisted payloads are JSON-serialized.
    #[cfg(feature = "serde")]
    async fn write_entry(persister: &QueryPersister, key: &QueryKey, updated_at_ms: u64) {
        let entry = PersistedQuery {
            buster: persister.options().buster().to_string(),
            query_hash: key.cache_key().to_string(),
            query_key: key.segments().to_vec(),
            state: PersistedQueryState {
                data_json: Some("\"crafted\"".to_string()),
                data_updated_at_ms: Some(updated_at_ms),
                error: None,
            },
        };
        persister
            .storage()
            .set(
                persister.storage_key(key.cache_key()),
                serialize_persisted_query(&entry).expect("entry should serialize"),
            )
            .await
            .expect("set should resolve");
    }

    #[test]
    fn test_options_defaults() {
        let options = QueryPersisterOptions::new(Arc::new(InMemoryQueryStorage::new()));
        assert_eq!(options.buster(), "");
        assert_eq!(options.max_age(), Duration::from_hours(24));
        assert_eq!(options.prefix(), "rs-query");
        assert!(options.refetch_on_restore());
        assert_eq!(options.stale_time(), None);
    }

    #[test]
    fn test_options_storage_accessor_shares_backend() {
        let storage: Arc<dyn QueryStorage> = Arc::new(InMemoryQueryStorage::new());
        let options = QueryPersisterOptions::new(Arc::clone(&storage));
        assert!(Arc::ptr_eq(options.storage(), &storage));
    }

    #[test]
    fn test_storage_key_format() {
        let persister = test_persister();
        let key = key(1);
        assert_eq!(
            persister.storage_key(key.cache_key()),
            "rs-query-users::id=1"
        );

        let custom = QueryPersister::new(
            QueryPersisterOptions::new(Arc::new(InMemoryQueryStorage::new()))
                .with_prefix("my-app")
                .with_buster("v2")
                .with_max_age(Duration::from_secs(60))
                .with_refetch_on_restore(false)
                .with_stale_time(Some(Duration::from_secs(30))),
        );
        assert_eq!(custom.storage_key("abc"), "my-app-abc");
        assert_eq!(custom.options().buster(), "v2");
        assert_eq!(custom.options().max_age(), Duration::from_secs(60));
        assert!(!custom.options().refetch_on_restore());
        assert_eq!(custom.options().stale_time(), Some(Duration::from_secs(30)));
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_persist_and_restore_roundtrip() {
        let persister = test_persister();
        let k = key(1);

        let source = QueryClient::new();
        source.set_query_data(&k, "hello".to_string(), QueryOptions::default());
        persister
            .persist_query(&source, &k)
            .await
            .expect("persist should succeed");
        assert_eq!(stored_entry_count(&persister).await, 1);

        let target = QueryClient::new();
        let restored: Option<String> = persister
            .restore_query(&target, &k)
            .await
            .expect("restore should succeed");
        assert_eq!(restored, Some("hello".to_string()));
        let cached: Option<String> = target.get_query_data(&k);
        assert_eq!(cached, Some("hello".to_string()));
        // Default stale_time of zero + refetch_on_restore: entry is stale.
        assert!(target.is_stale(&k));
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_persist_query_by_key_alias() {
        let persister = test_persister();
        let k = key(2);
        let source = QueryClient::new();
        source.set_query_data(&k, "alias".to_string(), QueryOptions::default());
        persister
            .persist_query_by_key(&source, &k)
            .await
            .expect("persist_query_by_key should succeed");
        assert_eq!(stored_entry_count(&persister).await, 1);

        let target = QueryClient::new();
        let restored: Option<String> = persister
            .restore_query(&target, &k)
            .await
            .expect("restore should succeed");
        assert_eq!(restored, Some("alias".to_string()));
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_restore_into_existing_data_is_noop() {
        let persister = test_persister();
        let k = key(3);

        let source = QueryClient::new();
        source.set_query_data(&k, "stored".to_string(), QueryOptions::default());
        persister
            .persist_query(&source, &k)
            .await
            .expect("persist should succeed");

        let target = QueryClient::new();
        target.set_query_data(&k, "mine".to_string(), QueryOptions::default());
        let restored: Option<String> = persister
            .restore_query(&target, &k)
            .await
            .expect("restore should succeed");
        // Existing client data wins; storage is not consulted.
        assert_eq!(restored, Some("mine".to_string()));
        let cached: Option<String> = target.get_query_data(&k);
        assert_eq!(cached, Some("mine".to_string()));
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_restore_skips_expired_entry() {
        let persister = test_persister();
        let k = key(4);
        write_entry(&persister, &k, now_ms().saturating_sub(25 * HOUR_MS)).await;

        let client = QueryClient::new();
        let restored: Option<String> = persister
            .restore_query(&client, &k)
            .await
            .expect("restore should succeed");
        assert_eq!(restored, None);
        assert!(client.get_query_data::<String>(&k).is_none());
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_restore_skips_entry_without_timestamp() {
        let persister = test_persister();
        let k = key(5);
        let entry = PersistedQuery {
            buster: String::new(),
            query_hash: k.cache_key().to_string(),
            query_key: k.segments().to_vec(),
            state: PersistedQueryState {
                data_json: Some("\"orphan\"".to_string()),
                data_updated_at_ms: None,
                error: None,
            },
        };
        persister
            .storage()
            .set(
                persister.storage_key(k.cache_key()),
                serialize_persisted_query(&entry).expect("entry should serialize"),
            )
            .await
            .expect("set should resolve");

        let client = QueryClient::new();
        let restored: Option<String> = persister
            .restore_query(&client, &k)
            .await
            .expect("restore should succeed");
        assert_eq!(restored, None);
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_restore_skips_buster_mismatch() {
        let persister = QueryPersister::new(
            QueryPersisterOptions::new(Arc::new(InMemoryQueryStorage::new())).with_buster("v2"),
        );
        let k = key(6);
        // The entry is fresh but carries an older buster.
        let entry = PersistedQuery {
            buster: "v1".to_string(),
            query_hash: k.cache_key().to_string(),
            query_key: k.segments().to_vec(),
            state: PersistedQueryState {
                data_json: Some("\"stale-buster\"".to_string()),
                data_updated_at_ms: Some(now_ms()),
                error: None,
            },
        };
        persister
            .storage()
            .set(
                persister.storage_key(k.cache_key()),
                serialize_persisted_query(&entry).expect("entry should serialize"),
            )
            .await
            .expect("set should resolve");

        let client = QueryClient::new();
        let restored: Option<String> = persister
            .restore_query(&client, &k)
            .await
            .expect("restore should succeed");
        assert_eq!(restored, None);
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_restored_fresh_entry_with_stale_time_is_not_stale() {
        let persister = QueryPersister::new(
            QueryPersisterOptions::new(Arc::new(InMemoryQueryStorage::new()))
                .with_stale_time(Some(Duration::from_secs(3600))),
        );
        let k = key(7);
        let source = QueryClient::new();
        source.set_query_data(&k, "fresh".to_string(), QueryOptions::default());
        persister
            .persist_query(&source, &k)
            .await
            .expect("persist should succeed");

        let target = QueryClient::new();
        let restored: Option<String> = persister
            .restore_query(&target, &k)
            .await
            .expect("restore should succeed");
        assert_eq!(restored, Some("fresh".to_string()));
        assert!(!target.is_stale(&k));
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_refetch_on_restore_false_keeps_entry_fresh() {
        let persister = QueryPersister::new(
            QueryPersisterOptions::new(Arc::new(InMemoryQueryStorage::new()))
                .with_refetch_on_restore(false),
        );
        let k = key(8);
        // Deliberately aged data: refetch_on_restore=false must keep it fresh.
        write_entry(&persister, &k, now_ms().saturating_sub(2 * HOUR_MS)).await;

        let client = QueryClient::new();
        let restored: Option<String> = persister
            .restore_query(&client, &k)
            .await
            .expect("restore should succeed");
        assert_eq!(restored, Some("crafted".to_string()));
        assert!(!client.is_stale(&k));
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_corrupted_entry_is_removed() {
        let persister = test_persister();
        let k = key(9);
        persister
            .storage()
            .set(
                persister.storage_key(k.cache_key()),
                "not json at all".to_string(),
            )
            .await
            .expect("set should resolve");

        let client = QueryClient::new();
        let restored: Option<String> = persister
            .restore_query(&client, &k)
            .await
            .expect("restore should succeed");
        assert_eq!(restored, None);
        // Corruption triggers removal, mirroring TanStack's removeItem.
        let remaining = persister
            .storage()
            .get(&persister.storage_key(k.cache_key()))
            .await
            .expect("get should resolve");
        assert_eq!(remaining, None);
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_persist_query_without_data_is_noop() {
        let persister = test_persister();
        let k = key(10);
        let client = QueryClient::new();
        persister
            .persist_query(&client, &k)
            .await
            .expect("persist without data should still succeed");
        assert_eq!(stored_entry_count(&persister).await, 0);
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_restore_skips_entry_without_payload() {
        let persister = test_persister();
        let k = key(19);
        // A fresh, unexpired entry that carries no data payload at all.
        let entry = PersistedQuery {
            buster: String::new(),
            query_hash: k.cache_key().to_string(),
            query_key: k.segments().to_vec(),
            state: PersistedQueryState {
                data_json: None,
                data_updated_at_ms: Some(now_ms()),
                error: None,
            },
        };
        persister
            .storage()
            .set(
                persister.storage_key(k.cache_key()),
                serialize_persisted_query(&entry).expect("entry should serialize"),
            )
            .await
            .expect("set should resolve");

        let client = QueryClient::new();
        let restored: Option<String> = persister
            .restore_query(&client, &k)
            .await
            .expect("restore should succeed");
        assert_eq!(restored, None, "no payload => no data");
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_restore_removes_entry_with_unusable_payload_type() {
        let persister = test_persister();
        let k = key(20);
        // The payload is valid JSON but not a `String`, so it cannot hydrate
        // into `T = String` and the entry is dropped from storage.
        let entry = PersistedQuery {
            buster: String::new(),
            query_hash: k.cache_key().to_string(),
            query_key: k.segments().to_vec(),
            state: PersistedQueryState {
                data_json: Some("123".to_string()),
                data_updated_at_ms: Some(now_ms()),
                error: None,
            },
        };
        persister
            .storage()
            .set(
                persister.storage_key(k.cache_key()),
                serialize_persisted_query(&entry).expect("entry should serialize"),
            )
            .await
            .expect("set should resolve");

        let client = QueryClient::new();
        let restored: Option<String> = persister
            .restore_query(&client, &k)
            .await
            .expect("restore should succeed");
        assert_eq!(restored, None, "payload does not fit T");
        let remaining = persister
            .storage()
            .get(&persister.storage_key(k.cache_key()))
            .await
            .expect("get should resolve");
        assert_eq!(remaining, None, "unusable entry is removed");
        assert!(client.get_query_data::<String>(&k).is_none());
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_persister_gc_removes_corrupted_entries() {
        let persister = test_persister();
        let fresh_key = key(21);
        let garbage_key = key(22);

        let client = QueryClient::new();
        client.set_query_data(&fresh_key, "fresh".to_string(), QueryOptions::default());
        persister
            .persist_query(&client, &fresh_key)
            .await
            .expect("persist should succeed");
        // A corrupted entry under the persister's prefix.
        persister
            .storage()
            .set(
                persister.storage_key(garbage_key.cache_key()),
                "@@not json@@".to_string(),
            )
            .await
            .expect("set should resolve");

        let removed = persister
            .persister_gc(&client)
            .await
            .expect("gc should succeed");
        assert_eq!(removed, 1, "only the corrupted entry is removed");
        assert_eq!(stored_entry_count(&persister).await, 1);
        let remaining = persister
            .storage()
            .get(&persister.storage_key(garbage_key.cache_key()))
            .await
            .expect("get should resolve");
        assert_eq!(remaining, None);
        // The fresh entry survives untouched.
        let fresh: Option<String> = client.get_query_data(&fresh_key);
        assert_eq!(fresh, Some("fresh".to_string()));
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_persister_gc_removes_expired_and_keeps_fresh() {
        let persister = test_persister();
        let fresh_key = key(11);
        let old_key = key(12);

        let client = QueryClient::new();
        client.set_query_data(&fresh_key, "fresh".to_string(), QueryOptions::default());
        persister
            .persist_query(&client, &fresh_key)
            .await
            .expect("persist should succeed");
        // Expired entry, plus a same-key client copy that should be
        // invalidated by the gc side effect.
        write_entry(&persister, &old_key, now_ms().saturating_sub(30 * HOUR_MS)).await;
        client.set_query_data(&old_key, "aged".to_string(), QueryOptions::default());

        // A foreign entry (different prefix) must be left alone.
        persister
            .storage()
            .set("other-prefix-entry".to_string(), "untouched".to_string())
            .await
            .expect("set should resolve");

        let removed = persister
            .persister_gc(&client)
            .await
            .expect("gc should succeed");
        assert_eq!(removed, 1);
        assert_eq!(stored_entry_count(&persister).await, 1);
        let foreign = persister
            .storage()
            .get("other-prefix-entry")
            .await
            .expect("get should resolve");
        assert_eq!(foreign.as_deref(), Some("untouched"));
        // Fresh data survives and stays untouched; the aged in-memory copy of
        // the removed entry is invalidated.
        let fresh: Option<String> = client.get_query_data(&fresh_key);
        assert_eq!(fresh, Some("fresh".to_string()));
        assert!(!client.is_stale(&fresh_key));
        assert!(client.is_stale(&old_key));
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_restore_queries_bulk() {
        let persister = test_persister();
        let k1 = key(13);
        let k2 = key(14);
        let missing = key(15);

        let source = QueryClient::new();
        source.set_query_data(&k1, "one".to_string(), QueryOptions::default());
        source.set_query_data(&k2, "two".to_string(), QueryOptions::default());
        persister
            .persist_query(&source, &k1)
            .await
            .expect("persist should succeed");
        persister
            .persist_query(&source, &k2)
            .await
            .expect("persist should succeed");

        let target = QueryClient::new();
        let restored = persister
            .restore_queries(&target, &[k1.clone(), k2.clone(), missing])
            .await
            .expect("restore_queries should succeed");
        assert_eq!(restored, 2);
        let one: Option<String> = target.get_query_data(&k1);
        let two: Option<String> = target.get_query_data(&k2);
        assert_eq!(one, Some("one".to_string()));
        assert_eq!(two, Some("two".to_string()));
    }

    #[cfg(feature = "serde")]
    #[tokio::test]
    async fn test_remove_queries_clears_storage_and_client() {
        let persister = test_persister();
        let k1 = key(16);
        let k2 = key(17);
        let absent = key(18);

        let client = QueryClient::new();
        client.set_query_data(&k1, "one".to_string(), QueryOptions::default());
        client.set_query_data(&k2, "two".to_string(), QueryOptions::default());
        persister
            .persist_query(&client, &k1)
            .await
            .expect("persist should succeed");
        persister
            .persist_query(&client, &k2)
            .await
            .expect("persist should succeed");

        let removed = persister
            .remove_queries(&client, &[k1.clone(), k2.clone(), absent])
            .await
            .expect("remove_queries should succeed");
        assert_eq!(removed, 2);
        assert_eq!(stored_entry_count(&persister).await, 0);
        assert!(client.get_query_data::<String>(&k1).is_none());
        assert!(client.get_query_data::<String>(&k2).is_none());
    }

    #[cfg(not(feature = "serde"))]
    #[tokio::test]
    async fn test_persist_without_serde_feature_errors() {
        let persister = test_persister();
        let k = key(1);
        let client = QueryClient::new();
        client.set_query_data(&k, "hello".to_string(), QueryOptions::default());
        let error = persister
            .persist_query(&client, &k)
            .await
            .expect_err("persist should fail without serde");
        assert_eq!(error, QueryError::serialization("serde feature required"));

        let gc_error = persister
            .persister_gc(&client)
            .await
            .expect_err("gc should fail without serde");
        assert_eq!(
            gc_error,
            QueryError::serialization("serde feature required")
        );
    }

    #[test]
    fn test_persister_is_cloneable_and_debuggable() {
        let persister = test_persister();
        let clone = persister.clone();
        assert!(format!("{persister:?}").contains("QueryPersister"));
        // Clones share the same storage.
        assert!(Arc::ptr_eq(persister.storage(), clone.storage()));
    }

    #[tokio::test]
    async fn test_in_memory_storage_roundtrip() {
        let storage = InMemoryQueryStorage::new();
        storage
            .set("a".to_string(), "1".to_string())
            .await
            .expect("set should resolve");
        storage
            .set("b".to_string(), "2".to_string())
            .await
            .expect("set should resolve");

        let a = storage.get("a").await.expect("get should resolve");
        assert_eq!(a.as_deref(), Some("1"));
        let missing = storage.get("zz").await.expect("get should resolve");
        assert_eq!(missing, None);

        let entries = storage.entries().await.expect("entries should resolve");
        assert_eq!(
            entries,
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string())
            ]
        );

        storage.remove("a").await.expect("remove should resolve");
        let removed = storage.get("a").await.expect("get should resolve");
        assert_eq!(removed, None);
    }

    #[test]
    fn test_in_memory_storage_as_any_downcast() {
        let storage = InMemoryQueryStorage::new();
        let downcast = storage
            .as_any()
            .downcast_ref::<InMemoryQueryStorage>()
            .expect("should downcast to InMemoryQueryStorage");
        assert!(downcast.map.is_empty());
    }
}
