// src/client.rs
//! `QueryClient` - central cache and query manager

use crate::executor::execute_query;
use crate::filters::{
    InvalidateQueryFilters, QueryFilters, QuerySnapshot, RefetchQueryFilters, RefetchType,
    match_query,
};
use crate::focus_manager::FocusManager;
use crate::infinite::InfiniteData;
use crate::network::OnlineManager;
use crate::observer::{QueryStateUpdate, QueryStateVariant};
use crate::options::RetryConfig;
use crate::query::Query;
use crate::sharing::PartialEqAny;
use crate::state::{FetchStatus, QueryState, QueryStatus};
use crate::{QueryKey, QueryOptions};
use dashmap::DashMap;
use std::any::TypeId;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

/// Cache entry for type-erased storage
#[derive(Clone)]
pub struct CacheEntry {
    pub data: Arc<dyn PartialEqAny + Send + Sync>,
    pub type_id: TypeId,
    pub fetched_at: Instant,
    pub last_accessed: Instant,
    pub options: QueryOptions,
    pub is_stale: bool,
}

/// An in-flight task with cancellation support.
pub(crate) struct InFlightTask {
    pub abort_handle: AbortHandle,
    pub cancel_token: CancellationToken,
}

/// Activity events for global fetching/mutating state.
#[derive(Clone, Debug)]
pub enum ActivityEvent {
    /// The global fetching ref-count changed; carries the new count.
    FetchingCountChanged(usize),
    /// The global mutating ref-count changed; carries the new count.
    MutatingCountChanged(usize),
}

/// Snapshot of a cached query's aggregate state, mirroring `TanStack` Query's
/// `queryClient.getQueryState()` return value.
///
/// The type-erased cache does not persist error states, so `status` is
/// `QueryStatus::Success` whenever data exists and `QueryStatus::Pending`
/// otherwise.
#[derive(Clone, Debug)]
pub struct QueryStateSnapshot {
    /// Aggregate status (`success` when data exists, `pending` otherwise).
    pub status: QueryStatus,
    /// `fetching` while a fetch is in flight, `idle` otherwise.
    pub fetch_status: FetchStatus,
    /// Whether the cached data is stale (by flag or by `stale_time`).
    pub is_stale: bool,
    /// Whether cached data exists (`true` for every returned snapshot).
    pub has_data: bool,
    /// When the cached data was written.
    pub data_updated_at: Option<Instant>,
}

/// Central query client managing cache and query execution.
pub struct QueryClient {
    /// Type-erased cache keyed by cache key; entries hold data, options and
    /// staleness. Exposed for devtools and advanced tooling.
    pub cache: Arc<DashMap<String, CacheEntry>>,
    /// In-flight tasks keyed by cache key.
    in_flight: Arc<DashMap<String, InFlightTask>>,
    // Broadcast channels for state updates, one per key.
    subscribers: Arc<DashMap<String, broadcast::Sender<QueryStateUpdate>>>,
    /// Focus manager for window focus refetching
    pub focus_manager: FocusManager,
    /// Global connectivity state (mirrors `TanStack`'s `onlineManager`).
    online: OnlineManager,
    /// Client-wide default options applied to newly created cache entries.
    default_options: Arc<RwLock<Option<QueryOptions>>>,
    /// Whether a persisted-cache restore is in progress (fetches are skipped).
    is_restoring: Arc<AtomicBool>,
    /// Ref-count of `mount()` calls driving the reconnect watcher.
    mount_count: Arc<AtomicUsize>,
    /// Stop signal of the spawned reconnect watcher (active while mounted).
    reconnect_task: Arc<Mutex<Option<watch::Sender<bool>>>>,
    // Activity counters
    fetching_count: Arc<AtomicUsize>,
    mutating_count: Arc<AtomicUsize>,
    activity_tx: broadcast::Sender<ActivityEvent>,
}

impl QueryClient {
    /// Create a new, empty query client.
    #[must_use]
    pub fn new() -> Self {
        let (activity_tx, _) = broadcast::channel(16);
        let client = Self {
            cache: Arc::new(DashMap::new()),
            in_flight: Arc::new(DashMap::new()),
            subscribers: Arc::new(DashMap::new()),
            focus_manager: FocusManager::new(),
            online: OnlineManager::new(),
            default_options: Arc::new(RwLock::new(None)),
            is_restoring: Arc::new(AtomicBool::new(false)),
            mount_count: Arc::new(AtomicUsize::new(0)),
            reconnect_task: Arc::new(Mutex::new(None)),
            fetching_count: Arc::new(AtomicUsize::new(0)),
            mutating_count: Arc::new(AtomicUsize::new(0)),
            activity_tx,
        };
        // Spawn background garbage collection thread.
        let cache = Arc::clone(&client.cache);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(60));
                Self::gc_internal(&cache);
            }
        });
        client
    }

    /// Subscribe to state changes for a given cache key.
    #[must_use]
    pub fn subscribe(&self, cache_key: &str) -> broadcast::Receiver<QueryStateUpdate> {
        let entry = self.subscribers.entry(cache_key.to_string());
        let sender = entry.or_insert_with(|| broadcast::channel(16).0);
        sender.subscribe()
    }

    /// Internal method to notify subscribers of a state change.
    pub fn notify_subscribers(&self, cache_key: &str, variant: QueryStateVariant) {
        if let Some(sender) = self.subscribers.get(cache_key) {
            let update = QueryStateUpdate {
                key: cache_key.to_string(),
                state_variant: variant,
            };
            let _ = sender.send(update);
        }
    }

    /// Get cached data if available and not stale.
    #[must_use]
    pub fn get_query_data<T: Clone + Send + Sync + 'static>(&self, key: &QueryKey) -> Option<T> {
        let cache_key = key.cache_key();
        let entry = self.cache.get(cache_key)?;

        if entry.type_id != TypeId::of::<T>() {
            return None;
        }

        entry.data.as_any().downcast_ref::<T>().cloned()
    }

    /// Get the options for a query key (if cached).
    #[must_use]
    pub fn get_query_options(&self, key: &QueryKey) -> Option<QueryOptions> {
        let cache_key = key.cache_key();
        self.cache.get(cache_key).map(|entry| entry.options.clone())
    }

    /// Get cached infinite data if available.
    #[must_use]
    pub fn get_infinite_data<T, P>(&self, key: &QueryKey) -> Option<InfiniteData<T, P>>
    where
        T: Clone + Send + Sync + 'static,
        P: Clone + Send + Sync + 'static,
    {
        self.get_query_data::<InfiniteData<T, P>>(key)
    }

    /// Check if the data for a key is stale.
    #[must_use]
    pub fn is_stale(&self, key: &QueryKey) -> bool {
        let cache_key = key.cache_key();
        if let Some(entry) = self.cache.get(cache_key) {
            let time_stale = !entry.options.stale_time.is_zero()
                && entry.fetched_at.elapsed() > entry.options.stale_time;
            time_stale || entry.is_stale
        } else {
            false
        }
    }

    /// Set cached data with optional structural sharing.
    ///
    /// When this call **creates** a new cache entry, client-wide default
    /// options (see [`QueryClient::set_default_options`]) are merged in:
    /// every field the caller left at its crate default is taken from the
    /// defaults. Updates to existing entries keep the caller-provided options.
    pub fn set_query_data<T: Clone + Send + Sync + 'static + PartialEq>(
        &self,
        key: &QueryKey,
        data: T,
        options: QueryOptions,
    ) {
        let cache_key = key.cache_key().to_string();
        let is_new = !self.cache.contains_key(&cache_key);
        let options = if is_new {
            match self
                .default_options
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
            {
                Some(defaults) => Self::merge_options(defaults, &options),
                None => options,
            }
        } else {
            options
        };
        let mut final_data: Arc<dyn PartialEqAny + Send + Sync> = Arc::new(data);

        if options.structural_sharing
            && let Some(old_entry) = self.cache.get(&cache_key)
            && old_entry.type_id == TypeId::of::<T>()
            && final_data.eq_any(old_entry.data.as_any())
        {
            final_data = old_entry.data.clone();
        }

        self.cache.insert(
            cache_key.clone(),
            CacheEntry {
                data: final_data,
                type_id: TypeId::of::<T>(),
                fetched_at: Instant::now(),
                last_accessed: Instant::now(),
                options,
                is_stale: false,
            },
        );
        self.notify_subscribers(&cache_key, QueryStateVariant::Success);
    }

    /// Set infinite query data.
    pub fn set_infinite_data<T, P>(
        &self,
        key: &QueryKey,
        data: InfiniteData<T, P>,
        options: QueryOptions,
    ) where
        T: Clone + Send + Sync + 'static + PartialEq,
        P: Clone + Send + Sync + 'static + PartialEq,
    {
        self.set_query_data(key, data, options);
    }

    /// Append a new page to existing infinite data.
    pub fn append_infinite_page<T, P>(
        &self,
        key: &QueryKey,
        page: T,
        page_param: P,
        max_pages: Option<usize>,
    ) -> Option<InfiniteData<T, P>>
    where
        T: Clone + Send + Sync + 'static + PartialEq,
        P: Clone + Send + Sync + 'static + PartialEq,
    {
        let cache_key = key.cache_key().to_string();
        let mut entry = self.cache.get_mut(&cache_key)?;

        if let Some(data) = entry.data.as_any().downcast_ref::<InfiniteData<T, P>>() {
            let mut new_data = data.clone();
            new_data.push_page(page, page_param);
            if let Some(max) = max_pages {
                while new_data.pages.len() > max {
                    new_data.pages.remove(0);
                    new_data.page_params.remove(0);
                }
            }
            entry.data = Arc::new(new_data.clone());
            entry.fetched_at = Instant::now();
            entry.last_accessed = Instant::now();
            self.notify_subscribers(&cache_key, QueryStateVariant::Success);
            return Some(new_data);
        }
        None
    }

    /// Prepend a new page to existing infinite data (for bi-directional).
    pub fn prepend_infinite_page<T, P>(
        &self,
        key: &QueryKey,
        page: T,
        page_param: P,
        max_pages: Option<usize>,
    ) -> Option<InfiniteData<T, P>>
    where
        T: Clone + Send + Sync + 'static + PartialEq,
        P: Clone + Send + Sync + 'static + PartialEq,
    {
        let cache_key = key.cache_key().to_string();
        let mut entry = self.cache.get_mut(&cache_key)?;

        if let Some(data) = entry.data.as_any().downcast_ref::<InfiniteData<T, P>>() {
            let mut new_data = data.clone();
            new_data.pages.insert(0, page);
            new_data.page_params.insert(0, page_param);
            if let Some(max) = max_pages {
                while new_data.pages.len() > max {
                    new_data.pages.pop();
                    new_data.page_params.pop();
                }
            }
            entry.data = Arc::new(new_data.clone());
            entry.fetched_at = Instant::now();
            entry.last_accessed = Instant::now();
            self.notify_subscribers(&cache_key, QueryStateVariant::Success);
            return Some(new_data);
        }
        None
    }

    /// Invalidate queries matching the key pattern.
    pub fn invalidate_queries(&self, pattern: &QueryKey) {
        let pattern_str = pattern.cache_key();
        let keys_to_invalidate: Vec<String> = self
            .cache
            .iter()
            .filter_map(|entry| {
                let key = entry.key();
                if key.starts_with(pattern_str) || key == pattern_str {
                    Some(key.clone())
                } else {
                    None
                }
            })
            .collect();

        for key in keys_to_invalidate {
            if let Some((_, task)) = self.in_flight.remove(&key) {
                task.cancel_token.cancel();
                task.abort_handle.abort();
            }
            if let Some(mut entry_mut) = self.cache.get_mut(&key) {
                entry_mut.is_stale = true;
                self.notify_subscribers(&key, QueryStateVariant::Stale);
            }
        }
    }

    /// Cancel a specific query if it's in flight.
    pub fn cancel_query(&self, key: &QueryKey) {
        let cache_key = key.cache_key();
        if let Some((_, task)) = self.in_flight.remove(cache_key) {
            task.cancel_token.cancel();
            task.abort_handle.abort();
            // Wake up any waiting subscribers so they don't hang.
            self.notify_subscribers(cache_key, QueryStateVariant::Error);
        }
    }
    /// Check if a query is in flight (for deduplication)
    #[must_use]
    pub fn is_in_flight(&self, key: &QueryKey) -> bool {
        self.in_flight.contains_key(key.cache_key())
    }

    /// Store an in-flight task for a query key.
    pub(crate) fn set_in_flight(&self, key: &QueryKey, task: InFlightTask) {
        let cache_key = key.cache_key().to_string();
        self.in_flight.insert(cache_key, task);
    }

    /// Remove the in-flight task for a query key.
    pub(crate) fn clear_in_flight(&self, key: &QueryKey) {
        let cache_key = key.cache_key();
        self.in_flight.remove(cache_key);
    }

    /// Clear all cached data
    pub fn clear(&self) {
        for entry in self.in_flight.iter() {
            entry.value().cancel_token.cancel();
            entry.value().abort_handle.abort();
        }
        self.in_flight.clear();
        self.cache.clear();
    }

    /// Run garbage collection on stale entries.
    pub fn gc(&self) {
        Self::gc_internal(&self.cache);
    }

    fn gc_internal(cache: &DashMap<String, CacheEntry>) {
        cache.retain(|_, entry| {
            let age = entry.last_accessed.elapsed();
            age < entry.options.gc_time
        });
    }

    /// Trigger refetch of all active stale queries (e.g., on window focus).
    pub fn refetch_all_stale(&self) {
        let keys: Vec<String> = self.cache.iter().map(|entry| entry.key().clone()).collect();
        for key in keys {
            if let Some(entry) = self.cache.get(&key) {
                let time_stale = !entry.options.stale_time.is_zero()
                    && entry.fetched_at.elapsed() > entry.options.stale_time;
                let is_stale = time_stale || entry.is_stale;

                if is_stale
                    && entry
                        .options
                        .refetch_on_window_focus
                        .allows_refetch(is_stale)
                {
                    drop(entry);
                    if let Some(mut entry_mut) = self.cache.get_mut(&key) {
                        entry_mut.is_stale = true;
                    }
                    self.notify_subscribers(&key, QueryStateVariant::Stale);
                }
            }
        }
    }

    // ----- TanStack-parity bulk operations -----

    /// Build a filter snapshot for one cache entry.
    ///
    /// "Active" is approximated as *in flight AND enabled*: the type-erased
    /// cache does not track observers, so a running fetch on an enabled entry
    /// is the closest available proxy for `TanStack`'s "has active observers".
    fn entry_snapshot(cache_key: &str, entry: &CacheEntry, in_flight: bool) -> QuerySnapshot {
        // Rebuild the key from the cache key (segments are joined with "::").
        // Split + re-join with the same separator is an identity roundtrip,
        // so the rebuilt key always reproduces the cache key string exactly,
        // including keys built from empty segments.
        let key = QueryKey::from_segments(cache_key.split("::"));
        QuerySnapshot {
            key,
            is_active: in_flight && entry.options.is_enabled(),
            is_stale: Self::entry_is_stale(entry),
            // Cache entries always hold resolved data, so a snapshot always
            // reports a successful status with data.
            status: QueryStatus::Success,
            fetch_status: if in_flight {
                FetchStatus::Fetching
            } else {
                FetchStatus::Idle
            },
            has_data: true,
        }
    }

    /// Staleness of a cache entry: stale flag OR `stale_time` elapsed.
    fn entry_is_stale(entry: &CacheEntry) -> bool {
        let time_stale = !entry.options.stale_time.is_zero()
            && entry.fetched_at.elapsed() > entry.options.stale_time;
        time_stale || entry.is_stale
    }

    /// Collect `(cache_key, snapshot)` pairs for entries matching `filters`.
    fn matching_entries(&self, filters: &QueryFilters) -> Vec<(String, QuerySnapshot)> {
        self.cache
            .iter()
            .filter_map(|entry| {
                let cache_key = entry.key().clone();
                let in_flight = self.in_flight.contains_key(cache_key.as_str());
                let snapshot = Self::entry_snapshot(&cache_key, &entry, in_flight);
                if match_query(&snapshot, filters) {
                    Some((cache_key, snapshot))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Snapshots for every cached query matching `filters` (the input to
    /// `TanStack`'s `matchQuery`-based bulk operations).
    #[must_use]
    pub fn query_snapshots(&self, filters: &QueryFilters) -> Vec<QuerySnapshot> {
        self.matching_entries(filters)
            .into_iter()
            .map(|(_, snapshot)| snapshot)
            .collect()
    }

    /// Snapshot the aggregate state for a query key (`TanStack`
    /// `getQueryState`). Returns `None` when the key is not cached.
    #[must_use]
    pub fn get_query_state(&self, key: &QueryKey) -> Option<QueryStateSnapshot> {
        let cache_key = key.cache_key();
        let entry = self.cache.get(cache_key)?;
        let in_flight = self.in_flight.contains_key(cache_key);
        Some(QueryStateSnapshot {
            status: QueryStatus::Success,
            fetch_status: if in_flight {
                FetchStatus::Fetching
            } else {
                FetchStatus::Idle
            },
            is_stale: Self::entry_is_stale(&entry),
            has_data: true,
            data_updated_at: Some(entry.fetched_at),
        })
    }

    /// Imperative fetch (`TanStack` `fetchQuery`): runs (or deduplicates
    /// against) the query fetch and caches the result.
    ///
    /// Retries are **forced off** — the query is cloned with a zero-retry
    /// [`RetryConfig`] unless the caller already disabled retries, mirroring
    /// `TanStack`'s `fetchQuery` retry forcing. Note that deduplication only
    /// applies to fetches registered as in-flight (e.g. via `spawn_query`);
    /// concurrent imperative `fetch_query` calls are not deduplicated against
    /// each other.
    pub async fn fetch_query<T>(&self, query: &Query<T>) -> QueryState<T>
    where
        T: Clone + Send + Sync + 'static + PartialEq,
    {
        let mut forced = query.clone();
        if forced.options.retry.max_retries > 0 {
            forced.options.retry = RetryConfig::new(0);
        }
        execute_query(self, &forced, CancellationToken::new()).await
    }

    /// Fire-and-forget fetch (`TanStack` `prefetchQuery`): awaits
    /// [`QueryClient::fetch_query`] and logs (then discards) any error, so it
    /// never rejects.
    pub async fn prefetch_query<T>(&self, query: &Query<T>)
    where
        T: Clone + Send + Sync + 'static + PartialEq,
    {
        let state = self.fetch_query(query).await;
        if let QueryState::Error { error, .. } = &state {
            tracing::debug!(key = %query.key, %error, "prefetch_query failed; error ignored");
        }
    }

    /// Fetch only when needed (`TanStack` `ensureQueryData`): fresh cached data
    /// is returned as-is; stale or missing data is fetched imperatively.
    pub async fn ensure_query_data<T>(&self, query: &Query<T>) -> QueryState<T>
    where
        T: Clone + Send + Sync + 'static + PartialEq,
    {
        if let Some(data) = self.get_query_data::<T>(&query.key)
            && !self.is_stale(&query.key)
        {
            return QueryState::Success(data);
        }
        self.fetch_query(query).await
    }

    /// Cancel in-flight fetches for every query matching `filters` (`TanStack`
    /// `cancelQueries`). Each cancelled fetch wakes its subscribers with an
    /// error notification (the revert-style variant) so deduplicated waiters
    /// do not hang. Returns the number of fetches actually cancelled.
    #[must_use]
    pub fn cancel_queries(&self, filters: &QueryFilters) -> usize {
        let mut cancelled = 0;
        for (cache_key, _) in self.matching_entries(filters) {
            if let Some((_, task)) = self.in_flight.remove(&cache_key) {
                task.cancel_token.cancel();
                task.abort_handle.abort();
                self.notify_subscribers(&cache_key, QueryStateVariant::Error);
                cancelled += 1;
            }
        }
        cancelled
    }

    /// Reset matching queries (`TanStack` `resetQueries`): in-flight fetches
    /// are cancelled, cache entries are removed and subscribers receive a
    /// loading notification. Returns the number of reset entries.
    ///
    /// `TanStack` additionally refetches *active* matched queries; here the
    /// refetch happens on the next observer update because the entry is gone
    /// from the cache.
    #[must_use]
    pub fn reset_queries(&self, filters: &QueryFilters) -> usize {
        let mut reset = 0;
        for (cache_key, _) in self.matching_entries(filters) {
            if let Some((_, task)) = self.in_flight.remove(&cache_key) {
                task.cancel_token.cancel();
                task.abort_handle.abort();
            }
            if self.cache.remove(&cache_key).is_some() {
                reset += 1;
            }
            self.notify_subscribers(&cache_key, QueryStateVariant::Loading);
        }
        reset
    }

    /// Remove matching cache entries and abort their in-flight fetches
    /// (`TanStack` `removeQueries`). Returns the number of removed entries.
    #[must_use]
    pub fn remove_queries(&self, filters: &QueryFilters) -> usize {
        let mut removed = 0;
        for (cache_key, _) in self.matching_entries(filters) {
            if let Some((_, task)) = self.in_flight.remove(&cache_key) {
                task.cancel_token.cancel();
                task.abort_handle.abort();
            }
            if self.cache.remove(&cache_key).is_some() {
                removed += 1;
            }
            // Wake deduplicated waiters: the entry no longer exists.
            self.notify_subscribers(&cache_key, QueryStateVariant::Error);
        }
        removed
    }

    /// Invalidate matching queries with `TanStack` `refetchType` semantics
    /// (`invalidateQueries`).
    ///
    /// Every match is marked stale. Then, unless `refetch_type` is
    /// [`RefetchType::None`], the matching subset (all / active / inactive)
    /// gets its in-flight fetch cancelled, a loading notification and a stale
    /// mark, so the next observer update triggers a real refetch.
    ///
    /// Limitation: the type-erased cache stores no fetch functions, so a
    /// refetch cannot be started from here — it is only *triggered*.
    #[must_use]
    pub fn invalidate_queries_filtered(&self, filters: InvalidateQueryFilters) -> usize {
        // Consume the filters object (`TanStack`'s `invalidateQueries` takes
        // an options object): split base filters and refetch type up front.
        let InvalidateQueryFilters {
            filters,
            refetch_type,
        } = filters;
        let matches = self.matching_entries(&filters);
        let invalidated = matches.len();
        for (cache_key, _) in &matches {
            if let Some(mut entry_mut) = self.cache.get_mut(cache_key) {
                entry_mut.is_stale = true;
            }
        }
        for (cache_key, snapshot) in &matches {
            let should_refetch = match refetch_type {
                RefetchType::All => true,
                RefetchType::Active => snapshot.is_active,
                RefetchType::Inactive => !snapshot.is_active,
                RefetchType::None => false,
            };
            if should_refetch {
                self.trigger_refetch(cache_key);
            }
        }
        invalidated
    }

    /// Cancel in-flight work and notify loading for one cache key, keeping
    /// the entry marked stale so the next observer update refetches it.
    fn trigger_refetch(&self, cache_key: &str) {
        if let Some((_, task)) = self.in_flight.remove(cache_key) {
            task.cancel_token.cancel();
            task.abort_handle.abort();
        }
        if let Some(mut entry_mut) = self.cache.get_mut(cache_key) {
            entry_mut.is_stale = true;
        }
        self.notify_subscribers(cache_key, QueryStateVariant::Loading);
    }

    /// Trigger refetches for matching queries (`TanStack` `refetchQueries`).
    ///
    /// Disabled queries and *static* queries with fresh data are skipped.
    /// With `cancel_refetch` (the `TanStack` default) any in-flight fetch is
    /// aborted first. Refetches are only *triggered* (cancel + loading
    /// notification + stale mark) because the cache does not store fetch
    /// functions; `throw_on_error` is accepted for API parity and has no
    /// effect since no promises are surfaced here.
    #[must_use]
    pub fn refetch_queries(&self, filters: QueryFilters, opts: RefetchQueryFilters) -> usize {
        // Consume the arguments (`TanStack`'s `refetchQueries` takes filters
        // and options objects): `opts.filters` is intentionally redundant —
        // the bare `filters` argument is authoritative — and
        // `throw_on_error` has no effect because no promises are surfaced.
        let RefetchQueryFilters {
            filters: _redundant_filters,
            cancel_refetch,
            throw_on_error: _,
        } = opts;
        let QueryFilters {
            query_key,
            exact,
            query_type,
            stale,
            fetch_status,
            status,
            predicate,
        } = filters;
        let matches = self.matching_entries(&QueryFilters {
            query_key,
            exact,
            query_type,
            stale,
            fetch_status,
            status,
            predicate,
        });
        let mut triggered = 0;
        for (cache_key, snapshot) in &matches {
            let Some(entry) = self.cache.get(cache_key) else {
                continue;
            };
            let enabled = entry.options.is_enabled();
            let static_fresh = entry.options.stale_time_static && !snapshot.is_stale;
            drop(entry);
            if !enabled || static_fresh {
                continue;
            }
            if cancel_refetch && let Some((_, task)) = self.in_flight.remove(cache_key) {
                task.cancel_token.cancel();
                task.abort_handle.abort();
            }
            if let Some(mut entry_mut) = self.cache.get_mut(cache_key) {
                entry_mut.is_stale = true;
            }
            self.notify_subscribers(cache_key, QueryStateVariant::Loading);
            triggered += 1;
        }
        triggered
    }

    /// Number of matching queries with a fetch in flight (`TanStack`
    /// `isFetching`).
    #[must_use]
    pub fn is_fetching_filtered(&self, filters: &QueryFilters) -> usize {
        self.matching_entries(filters)
            .into_iter()
            .filter(|(cache_key, _)| self.in_flight.contains_key(cache_key.as_str()))
            .count()
    }

    /// Client-wide default options (`TanStack` `getDefaultOptions`). Returns
    /// [`QueryOptions::default`] when none were configured.
    pub fn get_default_options(&self) -> QueryOptions {
        self.default_options
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_default()
    }

    /// Replace the client-wide default options (`TanStack` `setDefaultOptions`).
    /// These are applied to cache entries **created afterwards** by
    /// [`QueryClient::set_query_data`]; existing entries are untouched.
    pub fn set_default_options(&self, options: QueryOptions) {
        *self
            .default_options
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(options);
    }

    /// Merge caller-provided options over client defaults: every field the
    /// caller left at its crate default is taken from `defaults`.
    fn merge_options(defaults: &QueryOptions, provided: &QueryOptions) -> QueryOptions {
        let d = QueryOptions::default();
        let mut merged = defaults.clone();
        if provided.stale_time != d.stale_time {
            merged.stale_time = provided.stale_time;
        }
        if provided.stale_time_static != d.stale_time_static {
            merged.stale_time_static = provided.stale_time_static;
        }
        if provided.gc_time != d.gc_time {
            merged.gc_time = provided.gc_time;
        }
        if provided.retry != d.retry {
            merged.retry = provided.retry.clone();
        }
        if provided.enabled != d.enabled {
            merged.enabled = provided.enabled;
        }
        if provided.enabled_fn.is_some() {
            merged.enabled_fn.clone_from(&provided.enabled_fn);
        }
        if provided.refetch_on_window_focus != d.refetch_on_window_focus {
            merged.refetch_on_window_focus = provided.refetch_on_window_focus;
        }
        if provided.refetch_on_reconnect != d.refetch_on_reconnect {
            merged.refetch_on_reconnect = provided.refetch_on_reconnect;
        }
        if provided.refetch_on_mount != d.refetch_on_mount {
            merged.refetch_on_mount = provided.refetch_on_mount;
        }
        if provided.refetch_interval != d.refetch_interval {
            merged.refetch_interval = provided.refetch_interval;
        }
        if provided.refetch_interval_in_background != d.refetch_interval_in_background {
            merged.refetch_interval_in_background = provided.refetch_interval_in_background;
        }
        if provided.structural_sharing != d.structural_sharing {
            merged.structural_sharing = provided.structural_sharing;
        }
        if provided.network_mode != d.network_mode {
            merged.network_mode = provided.network_mode;
        }
        if provided.retry_on_mount != d.retry_on_mount {
            merged.retry_on_mount = provided.retry_on_mount;
        }
        if provided.throw_on_error != d.throw_on_error {
            merged.throw_on_error = provided.throw_on_error;
        }
        if provided.notify_on_change_props.is_some() {
            merged
                .notify_on_change_props
                .clone_from(&provided.notify_on_change_props);
        }
        if !provided.meta.is_empty() {
            merged.meta = provided.meta.clone();
        }
        merged
    }

    /// Whether a persisted-cache restore is in progress (`TanStack`
    /// `isRestoring`).
    #[must_use]
    pub fn is_restoring(&self) -> bool {
        self.is_restoring.load(Ordering::SeqCst)
    }

    /// Toggle the restore flag. While restoring, `execute_query` returns
    /// `QueryState::Idle` immediately without fetching (mirrors `TanStack`'s
    /// `_optimisticResults: 'isRestoring'` forcing `fetchStatus: 'idle'`).
    pub fn set_is_restoring(&self, restoring: bool) {
        self.is_restoring.store(restoring, Ordering::SeqCst);
    }

    /// Access the global connectivity manager (`TanStack` `onlineManager`).
    #[must_use]
    pub fn online_manager(&self) -> &OnlineManager {
        &self.online
    }

    /// Resume mutations that were paused while offline (`TanStack`
    /// `resumePausedMutations`).
    ///
    /// Placeholder: the mutation cache (`src/mutation_cache.rs`) owns paused
    /// mutations and will drive resumption in a later integration task, so
    /// this currently always reports `0` resumed mutations.
    #[must_use]
    pub fn resume_paused_mutations(&self) -> usize {
        0
    }

    /// Increment the mount ref-count (`TanStack` `QueryClient.mount`).
    ///
    /// On the first mount a watcher task is spawned that calls
    /// [`QueryClient::handle_reconnect`] whenever the online manager reports
    /// restored connectivity. Requires a running tokio runtime.
    pub fn mount(&self) {
        let count = self.mount_count.fetch_add(1, Ordering::SeqCst) + 1;
        if count == 1 {
            let mut rx = self.online.subscribe();
            let client = self.clone();
            let (stop_tx, mut stop_rx) = watch::channel(false);
            tokio::spawn(async move {
                // The online manager only emits *changes*, so a `true` update
                // always means a just-restored connection. A closed update
                // channel (online manager gone) also ends the loop.
                loop {
                    tokio::select! {
                        // Stop requested (`unmount`): leave the watcher.
                        _ = stop_rx.changed() => break,
                        changed = rx.changed() => {
                            if changed.is_ok() && *rx.borrow_and_update() {
                                client.handle_reconnect();
                            }
                        }
                    }
                }
            });
            *self
                .reconnect_task
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(stop_tx);
        }
    }

    /// Decrement the mount ref-count; the reconnect watcher is stopped when
    /// the count reaches zero. Must be paired with [`QueryClient::mount`].
    pub fn unmount(&self) {
        let count = self.mount_count.fetch_sub(1, Ordering::SeqCst);
        if count == 1
            && let Some(stop) = self
                .reconnect_task
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        {
            // Signal the watcher to stop; dropping the sender afterwards
            // closes the channel, so the task exits on its next poll.
            let _ = stop.send(true);
        }
    }

    /// Refetch-on-reconnect pass (`TanStack` `queryCache.onOnline`): every
    /// entry whose `refetch_on_reconnect` trigger allows it is marked stale
    /// and its subscribers notified, so observers refetch on their next
    /// update. Called automatically from the mount watcher and safe to call
    /// manually (e.g. from connectivity callbacks).
    pub fn handle_reconnect(&self) {
        let keys: Vec<String> = self.cache.iter().map(|entry| entry.key().clone()).collect();
        for key in keys {
            let Some(entry) = self.cache.get(&key) else {
                continue;
            };
            let is_stale = Self::entry_is_stale(&entry);
            if entry.options.refetch_on_reconnect.allows_refetch(is_stale) {
                drop(entry);
                if let Some(mut entry_mut) = self.cache.get_mut(&key) {
                    entry_mut.is_stale = true;
                }
                self.notify_subscribers(&key, QueryStateVariant::Stale);
            }
        }
    }

    // ----- Activity tracking -----

    /// Returns true if any query is currently fetching.
    #[must_use]
    pub fn is_fetching(&self) -> bool {
        self.fetching_count.load(Ordering::Relaxed) > 0
    }

    /// Returns the number of in-flight fetch operations.
    #[must_use]
    pub fn fetching_count(&self) -> usize {
        self.fetching_count.load(Ordering::Relaxed)
    }

    /// Returns true if any mutation is in progress.
    #[must_use]
    pub fn is_mutating(&self) -> bool {
        self.mutating_count.load(Ordering::Relaxed) > 0
    }

    /// Returns the number of in-flight mutations.
    #[must_use]
    pub fn mutating_count(&self) -> usize {
        self.mutating_count.load(Ordering::Relaxed)
    }

    /// Subscribe to activity events (fetching/mutating count changes).
    #[must_use]
    pub fn subscribe_activity(&self) -> broadcast::Receiver<ActivityEvent> {
        self.activity_tx.subscribe()
    }

    pub(crate) fn inc_fetching(&self) {
        let new = self.fetching_count.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self
            .activity_tx
            .send(ActivityEvent::FetchingCountChanged(new));
    }

    pub(crate) fn dec_fetching(&self) {
        let new = self.fetching_count.fetch_sub(1, Ordering::Relaxed) - 1;
        let _ = self
            .activity_tx
            .send(ActivityEvent::FetchingCountChanged(new));
    }

    pub(crate) fn inc_mutating(&self) {
        let new = self.mutating_count.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self
            .activity_tx
            .send(ActivityEvent::MutatingCountChanged(new));
    }

    pub(crate) fn dec_mutating(&self) {
        let new = self.mutating_count.fetch_sub(1, Ordering::Relaxed) - 1;
        let _ = self
            .activity_tx
            .send(ActivityEvent::MutatingCountChanged(new));
    }
}

impl Default for QueryClient {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for QueryClient {
    fn clone(&self) -> Self {
        Self {
            cache: Arc::clone(&self.cache),
            in_flight: Arc::clone(&self.in_flight),
            subscribers: Arc::clone(&self.subscribers),
            focus_manager: self.focus_manager.clone(),
            online: self.online.clone(),
            default_options: Arc::clone(&self.default_options),
            is_restoring: Arc::clone(&self.is_restoring),
            mount_count: Arc::clone(&self.mount_count),
            reconnect_task: Arc::clone(&self.reconnect_task),
            fetching_count: Arc::clone(&self.fetching_count),
            mutating_count: Arc::clone(&self.mutating_count),
            activity_tx: self.activity_tx.clone(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QueryError, QueryKey, QueryOptions};
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_set_and_get_query_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let data = "hello".to_string();

        client.set_query_data(&key, data.clone(), QueryOptions::default());
        let retrieved: Option<String> = client.get_query_data(&key);
        assert_eq!(retrieved, Some(data));
    }

    #[tokio::test]
    async fn test_get_query_data_stale() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let data = "hello".to_string();
        let options = QueryOptions {
            stale_time: Duration::from_millis(10),
            ..Default::default()
        };

        client.set_query_data(&key, data.clone(), options);
        sleep(Duration::from_millis(20)).await;

        let retrieved: Option<String> = client.get_query_data(&key);

        // Stale data should STILL be returned to the UI
        assert_eq!(retrieved, Some(data));
        // But the client should correctly identify it as stale
        assert!(client.is_stale(&key));
    }

    #[tokio::test]
    async fn test_invalidate_queries() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("users").with("id", 1);
        let key2 = QueryKey::new("users").with("id", 2);
        let data = "user".to_string();

        client.set_query_data(&key1, data.clone(), QueryOptions::default());
        client.set_query_data(&key2, data.clone(), QueryOptions::default());

        // Invalidate all users
        client.invalidate_queries(&QueryKey::new("users"));

        // Both should be stale now
        assert!(client.is_stale(&key1));
        assert!(client.is_stale(&key2));
    }

    #[tokio::test]
    async fn test_invalidate_queries_exact_match() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("users").with("id", 1);
        let key2 = QueryKey::new("users").with("id", 2);
        let data = "user".to_string();

        client.set_query_data(&key1, data.clone(), QueryOptions::default());
        client.set_query_data(&key2, data.clone(), QueryOptions::default());

        // Invalidate only key1
        client.invalidate_queries(&key1);

        assert!(client.is_stale(&key1));
        assert!(!client.is_stale(&key2));
    }

    #[tokio::test]
    async fn test_subscribe_and_notify() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let cache_key = key.cache_key().to_string();

        let mut rx = client.subscribe(&cache_key);

        // Set data triggers notification
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        let update = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("should receive notification")
            .expect("should be ok");

        assert_eq!(update.key, cache_key);
        assert!(matches!(update.state_variant, QueryStateVariant::Success));
    }

    #[tokio::test]
    async fn test_garbage_collection() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            gc_time: Duration::from_millis(50),
            ..Default::default()
        };

        client.set_query_data(&key, "data".to_string(), options);
        assert!(client.get_query_data::<String>(&key).is_some());

        // Wait for GC to run (background thread runs every 60s, so manually call gc)
        sleep(Duration::from_millis(60)).await;
        client.gc();

        assert!(client.get_query_data::<String>(&key).is_none());
    }

    #[tokio::test]
    async fn test_refetch_all_stale() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            stale_time: Duration::from_millis(10),
            refetch_on_window_focus: crate::options::RefetchTrigger::Enabled,
            ..Default::default()
        };

        client.set_query_data(&key, "data".to_string(), options);
        sleep(Duration::from_millis(20)).await;

        // Should be stale
        assert!(client.is_stale(&key));

        // Trigger refetch all stale (just ensures no panic)
        client.refetch_all_stale();
    }

    #[tokio::test]
    async fn test_is_in_flight_and_abort_handle() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        assert!(!client.is_in_flight(&key));

        // Simulate setting an in-flight task
        let handle = tokio::spawn(async {});
        let token = CancellationToken::new();
        client.set_in_flight(
            &key,
            InFlightTask {
                abort_handle: handle.abort_handle(),
                cancel_token: token,
            },
        );

        assert!(client.is_in_flight(&key));

        client.clear_in_flight(&key);
        assert!(!client.is_in_flight(&key));
    }

    // --- TESTS FOR 100% COVERAGE ---

    #[test]
    fn test_get_query_options() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            stale_time: Duration::from_secs(60),
            gc_time: Duration::from_secs(300),
            ..Default::default()
        };

        assert!(client.get_query_options(&key).is_none());

        client.set_query_data(&key, "data".to_string(), options.clone());
        let retrieved = client.get_query_options(&key).unwrap();
        assert_eq!(retrieved.stale_time, Duration::from_secs(60));
        assert_eq!(retrieved.gc_time, Duration::from_secs(300));
    }

    #[test]
    fn test_get_infinite_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        assert!(client.get_infinite_data::<Vec<i32>, i32>(&key).is_none());

        let data = InfiniteData::with_first_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data.clone(), QueryOptions::default());

        let retrieved = client.get_infinite_data::<Vec<i32>, i32>(&key).unwrap();
        assert_eq!(retrieved.page_count(), 1);
        assert_eq!(retrieved.pages[0], vec![1, 2, 3]);
    }

    #[test]
    fn test_set_infinite_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let data = InfiniteData::with_first_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let retrieved: Option<InfiniteData<Vec<i32>, i32>> = client.get_query_data(&key);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().pages[0], vec![1, 2, 3]);
    }

    #[test]
    fn test_is_stale_no_cache() {
        let client = QueryClient::new();
        let key = QueryKey::new("nonexistent");
        assert!(!client.is_stale(&key));
    }

    #[test]
    fn test_is_stale_not_stale() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());
        assert!(!client.is_stale(&key));
    }

    #[test]
    fn test_is_stale_flag_stale() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());
        client.invalidate_queries(&key);
        assert!(client.is_stale(&key));
    }

    #[test]
    fn test_is_stale_zero_stale_time() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        // stale_time of ZERO means data is ALWAYS fresh from a time perspective
        let options = QueryOptions {
            stale_time: Duration::ZERO,
            ..Default::default()
        };
        client.set_query_data(&key, "data".to_string(), options);
        std::thread::sleep(Duration::from_millis(10));
        assert!(!client.is_stale(&key));
    }

    #[test]
    fn test_get_query_data_wrong_type() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "string_data".to_string(), QueryOptions::default());

        // Try to get as wrong type
        let retrieved: Option<i32> = client.get_query_data(&key);
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_get_query_data_no_cache() {
        let client = QueryClient::new();
        let key = QueryKey::new("nonexistent");
        let retrieved: Option<String> = client.get_query_data(&key);
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_set_query_data_structural_sharing_disabled() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            structural_sharing: false,
            ..Default::default()
        };

        client.set_query_data(&key, "first".to_string(), options.clone());
        client.set_query_data(&key, "second".to_string(), options);

        let retrieved: Option<String> = client.get_query_data(&key);
        assert_eq!(retrieved, Some("second".to_string()));
    }

    #[test]
    fn test_set_query_data_structural_sharing_enabled_no_old_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            structural_sharing: true,
            ..Default::default()
        };

        // No old data exists, should still work
        client.set_query_data(&key, "data".to_string(), options);
        let retrieved: Option<String> = client.get_query_data(&key);
        assert_eq!(retrieved, Some("data".to_string()));
    }

    #[test]
    fn test_set_query_data_structural_sharing_wrong_type() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        // Store as String
        client.set_query_data(&key, "string".to_string(), QueryOptions::default());

        // Overwrite as i32 with structural sharing enabled
        // The old entry is String, new is i32 - type mismatch branch
        let options = QueryOptions {
            structural_sharing: true,
            ..Default::default()
        };
        client.set_query_data(&key, 42i32, options);

        let retrieved: Option<i32> = client.get_query_data(&key);
        assert_eq!(retrieved, Some(42));
    }

    #[test]
    fn test_append_infinite_page() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let data = InfiniteData::with_first_page(vec![1, 2], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let result = client.append_infinite_page(&key, vec![3, 4], 1, None);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![1, 2]);
        assert_eq!(result.pages[1], vec![3, 4]);
        assert_eq!(result.page_params, vec![0, 1]);
    }

    #[test]
    fn test_append_infinite_page_with_max_pages_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let data = InfiniteData::with_first_page(vec![1], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        // Max 2 pages, add second
        client.append_infinite_page(&key, vec![2], 1, Some(2));

        // Add third - should evict first
        let result = client.append_infinite_page(&key, vec![3], 2, Some(2));
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![2]);
        assert_eq!(result.pages[1], vec![3]);
    }

    #[test]
    fn test_append_infinite_page_with_max_pages_no_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let data = InfiniteData::with_first_page(vec![1], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        // Max 3 pages, only adding 1 more = 2 total, no eviction
        let result = client.append_infinite_page(&key, vec![2], 1, Some(3));
        assert!(result.is_some());
        assert_eq!(result.unwrap().page_count(), 2);
    }

    #[test]
    fn test_append_infinite_page_no_existing_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let result = client.append_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_append_infinite_page_wrong_type() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "wrong_type".to_string(), QueryOptions::default());

        let result = client.append_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_prepend_infinite_page() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let data = InfiniteData::with_first_page(vec![3, 4], 1);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let result = client.prepend_infinite_page(&key, vec![1, 2], 0, None);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![1, 2]);
        assert_eq!(result.pages[1], vec![3, 4]);
        assert_eq!(result.page_params, vec![0, 1]);
    }

    #[test]
    fn test_prepend_infinite_page_with_max_pages_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let data = InfiniteData::with_first_page(vec![3], 2);
        client.set_infinite_data(&key, data, QueryOptions::default());

        client.prepend_infinite_page(&key, vec![2], 1, Some(2));
        let result = client.prepend_infinite_page(&key, vec![1], 0, Some(2));

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![1]);
        assert_eq!(result.pages[1], vec![2]);
        assert_eq!(result.page_params, vec![0, 1]);
    }

    #[test]
    fn test_prepend_infinite_page_with_max_pages_no_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let data = InfiniteData::with_first_page(vec![2], 1);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let result = client.prepend_infinite_page(&key, vec![1], 0, Some(3));
        assert!(result.is_some());
        assert_eq!(result.unwrap().page_count(), 2);
    }

    #[test]
    fn test_prepend_infinite_page_no_existing_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let result = client.prepend_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_prepend_infinite_page_wrong_type() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "wrong_type".to_string(), QueryOptions::default());

        let result = client.prepend_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_invalidate_queries_with_abort_handle() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        // Set an in-flight task
        let handle = tokio::spawn(async {});
        let token = CancellationToken::new();
        client.set_in_flight(
            &key,
            InFlightTask {
                abort_handle: handle.abort_handle(),
                cancel_token: token,
            },
        );

        assert!(client.is_in_flight(&key));

        // Invalidate should cancel in-flight
        client.invalidate_queries(&key);

        assert!(!client.is_in_flight(&key));
        assert!(client.is_stale(&key));
    }

    #[tokio::test(start_paused = true)]
    async fn test_cancel_query_with_handle() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        // Create a handle via tokio::spawn
        let handle = tokio::spawn(async {
            // Long running task
            tokio::time::sleep(std::time::Duration::from_secs(100)).await;
        });
        let token = CancellationToken::new();
        client.set_in_flight(
            &key,
            InFlightTask {
                abort_handle: handle.abort_handle(),
                cancel_token: token,
            },
        );

        // Should be in flight
        assert!(client.is_in_flight(&key));

        // Let the spawned body run to completion on the paused clock so the
        // task's own lines are covered; aborting a finished task is a no-op.
        tokio::time::advance(std::time::Duration::from_secs(100)).await;
        handle.await.expect("task body completed");

        // Cancel should work
        client.cancel_query(&key);

        // Handle should be cleared
        assert!(!client.is_in_flight(&key));
    }

    #[test]
    fn test_cancel_query_no_handle() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        // Should not panic when no handle exists
        client.cancel_query(&key);
    }

    #[tokio::test(start_paused = true)]
    async fn test_clear_with_abort_handles() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("test1");
        let key2 = QueryKey::new("test2");

        // Create handles
        let handle1 = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(100)).await;
        });
        let handle2 = tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(100)).await;
        });

        let token1 = CancellationToken::new();
        let token2 = CancellationToken::new();
        client.set_in_flight(
            &key1,
            InFlightTask {
                abort_handle: handle1.abort_handle(),
                cancel_token: token1,
            },
        );
        client.set_in_flight(
            &key2,
            InFlightTask {
                abort_handle: handle2.abort_handle(),
                cancel_token: token2,
            },
        );

        // Both should be in flight
        assert!(client.is_in_flight(&key1));
        assert!(client.is_in_flight(&key2));

        // Let both spawned bodies run to completion on the paused clock so
        // their lines are covered; aborting finished tasks is a no-op.
        tokio::time::advance(std::time::Duration::from_secs(100)).await;
        let (first, second) = tokio::join!(handle1, handle2);
        first.expect("task body completed");
        second.expect("task body completed");

        // Clear should abort all
        client.clear();

        // Both handles should be cleared
        assert!(!client.is_in_flight(&key1));
        assert!(!client.is_in_flight(&key2));
    }

    #[test]
    fn test_clear_empty() {
        let client = QueryClient::new();
        // Should not panic on empty cache
        client.clear();
        assert!(client.cache.is_empty());
    }

    #[test]
    fn test_refetch_all_stale_not_stale() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            stale_time: Duration::from_secs(60),
            refetch_on_window_focus: crate::options::RefetchTrigger::Enabled,
            ..Default::default()
        };

        client.set_query_data(&key, "data".to_string(), options);

        // Data is fresh, refetch_all_stale should not notify
        client.refetch_all_stale();
        assert!(!client.is_stale(&key));
    }

    #[test]
    fn test_refetch_all_stale_refetch_disabled() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            stale_time: Duration::from_millis(1),
            refetch_on_window_focus: crate::options::RefetchTrigger::Disabled,
            ..Default::default()
        };

        client.set_query_data(&key, "data".to_string(), options);
        std::thread::sleep(Duration::from_millis(5));

        assert!(client.is_stale(&key));

        // refetch_on_window_focus is false, so refetch_all_stale should not notify
        client.refetch_all_stale();
        // The data should still be stale (not re-fetched)
        assert!(client.is_stale(&key));
    }

    #[test]
    fn test_refetch_all_stale_empty_cache() {
        let client = QueryClient::new();
        // Should not panic on empty cache
        client.refetch_all_stale();
    }

    #[test]
    fn test_refetch_all_stale_flag_stale_with_refetch_enabled() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            stale_time: Duration::ZERO, // time-based not stale
            refetch_on_window_focus: crate::options::RefetchTrigger::Enabled,
            ..Default::default()
        };

        client.set_query_data(&key, "data".to_string(), options);
        // Manually mark as stale via invalidation
        client.invalidate_queries(&key);

        // Now refetch_all_stale should pick it up (flag is stale, refetch enabled)
        client.refetch_all_stale();
        assert!(client.is_stale(&key));
    }

    #[test]
    fn test_subscribe_multiple_subscribers() {
        let client = QueryClient::new();
        let cache_key = "test".to_string();

        let mut rx1 = client.subscribe(&cache_key);
        let mut rx2 = client.subscribe(&cache_key);

        client.notify_subscribers(&cache_key, QueryStateVariant::Success);

        let u1 = rx1.try_recv().unwrap();
        let u2 = rx2.try_recv().unwrap();
        assert_eq!(u1.key, cache_key);
        assert_eq!(u2.key, cache_key);
    }

    #[test]
    fn test_notify_subscribers_no_subscribers() {
        let client = QueryClient::new();
        // Should not panic when no subscribers exist
        client.notify_subscribers("nonexistent", QueryStateVariant::Success);
    }

    #[test]
    fn test_client_default() {
        let client = QueryClient::default();
        assert!(client.cache.is_empty());
    }

    #[test]
    fn test_client_clone() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        let cloned = client.clone();
        let data: Option<String> = cloned.get_query_data(&key);
        assert_eq!(data, Some("data".to_string()));
    }

    #[test]
    fn test_gc_retains_fresh_entries() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            gc_time: Duration::from_secs(300),
            ..Default::default()
        };

        client.set_query_data(&key, "data".to_string(), options);
        client.gc();
        let data: Option<String> = client.get_query_data(&key);
        assert!(data.is_some());
    }

    #[test]
    fn test_invalidate_queries_no_matching_keys() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        // Pattern doesn't match
        client.invalidate_queries(&QueryKey::new("posts"));
        assert!(!client.is_stale(&key));
    }

    #[test]
    fn test_invalidate_queries_prefix_match() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("users").with("id", 1);
        let key2 = QueryKey::new("users").with("id", 2);
        let key3 = QueryKey::new("posts").with("id", 1);

        client.set_query_data(&key1, "u1".to_string(), QueryOptions::default());
        client.set_query_data(&key2, "u2".to_string(), QueryOptions::default());
        client.set_query_data(&key3, "p1".to_string(), QueryOptions::default());

        client.invalidate_queries(&QueryKey::new("users"));

        assert!(client.is_stale(&key1));
        assert!(client.is_stale(&key2));
        assert!(!client.is_stale(&key3));
    }

    #[tokio::test]
    async fn test_subscribe_creates_sender_lazily() {
        let client = QueryClient::new();
        let cache_key = "new_key".to_string();

        // This should create a new sender
        let mut rx = client.subscribe(&cache_key);
        client.notify_subscribers(&cache_key, QueryStateVariant::Loading);

        let update = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(update.state_variant, QueryStateVariant::Loading));
    }

    #[test]
    fn test_set_query_data_resets_stale_flag() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        client.set_query_data(&key, "data".to_string(), QueryOptions::default());
        client.invalidate_queries(&key);
        assert!(client.is_stale(&key));

        // Setting new data should reset stale flag
        client.set_query_data(&key, "fresh".to_string(), QueryOptions::default());
        assert!(!client.is_stale(&key));
    }

    // ----- TanStack-parity surface tests -----

    fn dummy_task() -> crate::client::InFlightTask {
        let handle = tokio::spawn(async {});
        crate::client::InFlightTask {
            abort_handle: handle.abort_handle(),
            cancel_token: CancellationToken::new(),
        }
    }

    #[test]
    fn test_get_query_state_with_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        let state = client.get_query_state(&key).expect("cached key has state");
        assert_eq!(state.status, QueryStatus::Success);
        assert_eq!(state.fetch_status, FetchStatus::Idle);
        assert!(!state.is_stale);
        assert!(state.has_data);
        assert!(state.data_updated_at.is_some());
    }

    #[test]
    fn test_get_query_state_missing() {
        let client = QueryClient::new();
        let key = QueryKey::new("missing");
        assert!(client.get_query_state(&key).is_none());
    }

    #[tokio::test]
    async fn test_get_query_state_in_flight_is_fetching() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());
        client.set_in_flight(&key, dummy_task());

        let state = client.get_query_state(&key).unwrap();
        assert_eq!(state.fetch_status, FetchStatus::Fetching);
    }

    #[test]
    fn test_get_query_state_stale_flag() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());
        client.invalidate_queries(&key);

        let state = client.get_query_state(&key).unwrap();
        assert!(state.is_stale);
    }

    #[tokio::test]
    async fn test_fetch_query_success_and_caches() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query = Query::new(key.clone(), || async { Ok("data".to_string()) });

        let state = client.fetch_query(&query).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"data".to_string()));
        let cached: Option<String> = client.get_query_data(&key);
        assert_eq!(cached, Some("data".to_string()));
    }

    #[tokio::test]
    async fn test_fetch_query_forces_retries_off() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let client = QueryClient::new();
        let key = QueryKey::new("test");
        // Default retry config allows 3 retries; fetch_query must force 0.
        let query = Query::new(key, move || {
            let a = Arc::clone(&attempts_clone);
            async move {
                a.fetch_add(1, Ordering::SeqCst);
                Err::<String, QueryError>(QueryError::network("down"))
            }
        });

        let state = client.fetch_query(&query).await;
        assert!(state.is_error());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_prefetch_query_ignores_errors() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<String> = Query::new(key, || async { Err(QueryError::custom("boom")) });

        // Must not panic / reject.
        client.prefetch_query(&query).await;
    }

    #[tokio::test]
    async fn test_prefetch_query_success_is_silent() {
        let client = QueryClient::new();
        let key = QueryKey::new("prefetch-ok");
        let query: Query<String> = Query::new(key, || async { Ok("data".to_string()) });

        // A successful prefetch caches the data without logging an error.
        client.prefetch_query(&query).await;
        assert_eq!(
            client.get_query_data::<String>(&query.key),
            Some("data".to_string())
        );
    }

    #[tokio::test]
    async fn test_ensure_query_data_fresh_skips_fetch() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let fetches_clone = Arc::clone(&fetches);

        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            stale_time: Duration::from_secs(60),
            ..Default::default()
        };
        client.set_query_data(&key, "cached".to_string(), options);

        let query = Query::new(key, move || {
            let f = Arc::clone(&fetches_clone);
            async move {
                f.fetch_add(1, Ordering::SeqCst);
                Ok::<String, QueryError>("fetched".to_string())
            }
        });

        let state = client.ensure_query_data(&query).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"cached".to_string()));
        assert_eq!(fetches.load(Ordering::SeqCst), 0);

        // Exercise the fetch closure itself: the fresh-skip path above never
        // invokes it, so run one imperative fetch to cover the fetch body.
        let state = client.fetch_query(&query).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"fetched".to_string()));
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_ensure_query_data_missing_fetches() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query = Query::new(key, || async {
            Ok::<String, QueryError>("fetched".to_string())
        });

        let state = client.ensure_query_data(&query).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&"fetched".to_string()));
    }

    #[tokio::test]
    async fn test_ensure_query_data_stale_fetches() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            stale_time: Duration::from_millis(1),
            ..Default::default()
        };
        client.set_query_data(&key, "old".to_string(), options);
        tokio::time::sleep(Duration::from_millis(10)).await;

        let query = Query::new(key, || async {
            Ok::<String, QueryError>("new".to_string())
        });
        let state = client.ensure_query_data(&query).await;
        assert_eq!(state.data(), Some(&"new".to_string()));
    }

    #[tokio::test]
    async fn test_cancel_queries_cancels_matching_in_flight() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("users").with("id", 1);
        let key2 = QueryKey::new("users").with("id", 2);
        let key3 = QueryKey::new("posts");
        for key in [&key1, &key2, &key3] {
            client.set_query_data(key, "data".to_string(), QueryOptions::default());
        }
        client.set_in_flight(&key1, dummy_task());
        client.set_in_flight(&key3, dummy_task());

        let filters = QueryFilters::with_key(QueryKey::new("users"));
        let cancelled = client.cancel_queries(&filters);

        assert_eq!(cancelled, 1);
        assert!(!client.is_in_flight(&key1));
        assert!(client.is_in_flight(&key3));
    }

    #[tokio::test]
    async fn test_cancel_queries_no_match() {
        let client = QueryClient::new();
        let filters = QueryFilters::all();
        assert_eq!(client.cancel_queries(&filters), 0);
    }

    #[tokio::test]
    async fn test_reset_queries_clears_data_and_notifies() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("users").with("id", 1);
        let key2 = QueryKey::new("posts");
        client.set_query_data(&key1, "u".to_string(), QueryOptions::default());
        client.set_query_data(&key2, "p".to_string(), QueryOptions::default());
        client.set_in_flight(&key1, dummy_task());

        let mut rx = client.subscribe(key1.cache_key());

        let filters = QueryFilters::with_key(QueryKey::new("users"));
        let reset = client.reset_queries(&filters);

        assert_eq!(reset, 1);
        assert!(client.get_query_data::<String>(&key1).is_none());
        assert!(!client.is_in_flight(&key1));
        assert!(client.get_query_data::<String>(&key2).is_some());

        let update = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("notification expected")
            .expect("should be ok");
        assert!(matches!(update.state_variant, QueryStateVariant::Loading));
    }

    #[tokio::test]
    async fn test_remove_queries_removes_entries() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("users").with("id", 1);
        let key2 = QueryKey::new("users").with("id", 2);
        client.set_query_data(&key1, "a".to_string(), QueryOptions::default());
        client.set_query_data(&key2, "b".to_string(), QueryOptions::default());
        client.set_in_flight(&key1, dummy_task());

        let filters = QueryFilters::with_key(key1.clone()).exact();
        let removed = client.remove_queries(&filters);

        assert_eq!(removed, 1);
        assert!(client.get_query_data::<String>(&key1).is_none());
        assert!(!client.is_in_flight(&key1));
        assert!(client.get_query_data::<String>(&key2).is_some());
    }

    #[tokio::test]
    async fn test_invalidate_queries_filtered_marks_all_stale() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("users").with("id", 1);
        let key2 = QueryKey::new("users").with("id", 2);
        client.set_query_data(&key1, "a".to_string(), QueryOptions::default());
        client.set_query_data(&key2, "b".to_string(), QueryOptions::default());

        let filters = InvalidateQueryFilters::new()
            .filters(QueryFilters::with_key(QueryKey::new("users")))
            .refetch_type(RefetchType::None);
        let count = client.invalidate_queries_filtered(filters);

        assert_eq!(count, 2);
        assert!(client.is_stale(&key1));
        assert!(client.is_stale(&key2));
    }

    #[tokio::test]
    async fn test_invalidate_queries_filtered_refetch_all_triggers() {
        let client = QueryClient::new();
        let key = QueryKey::new("users").with("id", 1);
        client.set_query_data(&key, "a".to_string(), QueryOptions::default());

        let mut rx = client.subscribe(key.cache_key());

        let filters = InvalidateQueryFilters::new()
            .filters(QueryFilters::with_key(QueryKey::new("users")))
            .refetch_type(RefetchType::All);
        let count = client.invalidate_queries_filtered(filters);

        assert_eq!(count, 1);
        assert!(client.is_stale(&key));
        let update = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("notification expected")
            .expect("should be ok");
        assert!(matches!(update.state_variant, QueryStateVariant::Loading));
    }

    #[tokio::test]
    async fn test_invalidate_queries_filtered_refetch_active_only() {
        let client = QueryClient::new();
        let active_key = QueryKey::new("users").with("id", 1);
        let inactive_key = QueryKey::new("users").with("id", 2);
        client.set_query_data(&active_key, "a".to_string(), QueryOptions::default());
        client.set_query_data(&inactive_key, "b".to_string(), QueryOptions::default());
        // Simulate "active" (in flight + enabled) for the first key.
        client.set_in_flight(&active_key, dummy_task());

        let mut rx = client.subscribe(inactive_key.cache_key());

        let filters = InvalidateQueryFilters::new()
            .filters(QueryFilters::with_key(QueryKey::new("users")))
            .refetch_type(RefetchType::Active);
        let count = client.invalidate_queries_filtered(filters);

        assert_eq!(count, 2);
        assert!(client.is_stale(&active_key));
        assert!(client.is_stale(&inactive_key));
        // Inactive entry was not refetch-triggered: no notification arrives.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_invalidate_queries_filtered_refetch_inactive_only() {
        let client = QueryClient::new();
        let active_key = QueryKey::new("users").with("id", 1);
        let inactive_key = QueryKey::new("users").with("id", 2);
        client.set_query_data(&active_key, "a".to_string(), QueryOptions::default());
        client.set_query_data(&inactive_key, "b".to_string(), QueryOptions::default());
        client.set_in_flight(&active_key, dummy_task());

        let mut rx = client.subscribe(inactive_key.cache_key());

        let filters = InvalidateQueryFilters::new()
            .filters(QueryFilters::with_key(QueryKey::new("users")))
            .refetch_type(RefetchType::Inactive);
        let count = client.invalidate_queries_filtered(filters);

        assert_eq!(count, 2);
        let update = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("notification expected")
            .expect("should be ok");
        assert!(matches!(update.state_variant, QueryStateVariant::Loading));
    }

    #[test]
    fn test_query_snapshots_rebuild_keys_from_empty_segments() {
        let client = QueryClient::new();
        // A key built from empty segments produces the cache key "::="; the
        // snapshot's split/re-join rebuild must reproduce it exactly,
        // including the empty leading segment.
        let key = QueryKey::new("").with("", "");
        assert_eq!(key.cache_key(), "::=");
        client.set_query_data(&key, "v".to_string(), QueryOptions::default());

        let snapshots = client.query_snapshots(&QueryFilters::default());
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].key.cache_key(), "::=");
        assert_eq!(snapshots[0].key.segments(), ["", "="]);
        assert_eq!(snapshots[0].status, QueryStatus::Success);
        assert!(snapshots[0].has_data);
    }

    #[tokio::test]
    async fn test_refetch_queries_skips_disabled_and_static_fresh() {
        let client = QueryClient::new();
        let normal = QueryKey::new("q").with("id", 1);
        let disabled = QueryKey::new("q").with("id", 2);
        let static_fresh = QueryKey::new("q").with("id", 3);

        client.set_query_data(&normal, "1".to_string(), QueryOptions::default());
        client.set_query_data(
            &disabled,
            "2".to_string(),
            QueryOptions::default().enabled(false),
        );
        client.set_query_data(
            &static_fresh,
            "3".to_string(),
            QueryOptions::default().stale_time_static(),
        );

        let triggered = client.refetch_queries(
            QueryFilters::with_key(QueryKey::new("q")),
            RefetchQueryFilters::new(),
        );

        assert_eq!(triggered, 1);
        assert!(client.is_stale(&normal));
        assert!(!client.is_stale(&disabled));
        assert!(!client.is_stale(&static_fresh));
    }

    #[tokio::test]
    async fn test_refetch_queries_cancels_in_flight_when_requested() {
        let client = QueryClient::new();
        let key = QueryKey::new("q");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());
        client.set_in_flight(&key, dummy_task());

        let triggered = client.refetch_queries(
            QueryFilters::with_key(QueryKey::new("q")),
            RefetchQueryFilters::new().cancel_refetch(true),
        );

        assert_eq!(triggered, 1);
        assert!(!client.is_in_flight(&key));
    }

    #[tokio::test]
    async fn test_is_fetching_filtered_counts_in_flight() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("users").with("id", 1);
        let key2 = QueryKey::new("users").with("id", 2);
        let key3 = QueryKey::new("posts");
        for key in [&key1, &key2, &key3] {
            client.set_query_data(key, "data".to_string(), QueryOptions::default());
        }
        client.set_in_flight(&key1, dummy_task());
        client.set_in_flight(&key3, dummy_task());

        let users = QueryFilters::with_key(QueryKey::new("users"));
        assert_eq!(client.is_fetching_filtered(&users), 1);
        assert_eq!(client.is_fetching_filtered(&QueryFilters::all()), 2);
        assert_eq!(
            client.is_fetching_filtered(&QueryFilters::with_key(QueryKey::new("missing"))),
            0
        );
    }

    #[test]
    fn test_default_options_applied_to_new_entries() {
        let client = QueryClient::new();
        let defaults = QueryOptions::default().stale_time(Duration::from_secs(60));
        client.set_default_options(defaults);
        assert_eq!(
            client.get_default_options().stale_time,
            Duration::from_secs(60)
        );

        let key = QueryKey::new("test");
        // Caller leaves stale_time at its default -> the client default wins.
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());
        let stored = client.get_query_options(&key).unwrap();
        assert_eq!(stored.stale_time, Duration::from_secs(60));

        // Caller-provided values override the defaults.
        client.set_default_options(QueryOptions::default());
        let key2 = QueryKey::new("test2");
        client.set_query_data(
            &key2,
            "data".to_string(),
            QueryOptions::default().stale_time(Duration::from_secs(1)),
        );
        let stored = client.get_query_options(&key2).unwrap();
        assert_eq!(stored.stale_time, Duration::from_secs(1));
    }

    #[test]
    fn test_default_options_do_not_touch_existing_entries() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "data".to_string(), QueryOptions::default());

        client.set_default_options(QueryOptions::default().gc_time(Duration::from_secs(999)));
        client.set_query_data(&key, "updated".to_string(), QueryOptions::default());

        let stored = client.get_query_options(&key).unwrap();
        assert_eq!(stored.gc_time, crate::options::DEFAULT_GC_TIME);
    }

    #[test]
    fn test_merge_options_defaults_win_for_default_fields() {
        let defaults = QueryOptions {
            stale_time: Duration::from_secs(30),
            gc_time: Duration::from_secs(600),
            meta: std::collections::BTreeMap::from([("origin".to_string(), "client".to_string())]),
            ..Default::default()
        };

        let provided = QueryOptions::default().retry(crate::options::RetryConfig::new(7));
        let merged = QueryClient::merge_options(&defaults, &provided);

        assert_eq!(merged.stale_time, Duration::from_secs(30));
        assert_eq!(merged.gc_time, Duration::from_secs(600));
        assert_eq!(merged.retry.max_retries, 7);
        assert_eq!(merged.meta.get("origin"), Some(&"client".to_string()));
    }

    #[test]
    fn test_is_restoring_toggle() {
        let client = QueryClient::new();
        assert!(!client.is_restoring());
        client.set_is_restoring(true);
        assert!(client.is_restoring());
        client.set_is_restoring(false);
        assert!(!client.is_restoring());
    }

    #[test]
    fn test_online_manager_accessor() {
        let client = QueryClient::new();
        assert!(client.online_manager().is_online());
        client.online_manager().set_online(false);
        assert!(!client.online_manager().is_online());
    }

    #[test]
    fn test_resume_paused_mutations_placeholder() {
        let client = QueryClient::new();
        assert_eq!(client.resume_paused_mutations(), 0);
    }

    #[tokio::test]
    async fn test_mount_unmount_and_reconnect_watcher() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        // `Always` refetches on reconnect even with fresh data.
        client.set_query_data(
            &key,
            "data".to_string(),
            QueryOptions::default().refetch_on_reconnect(crate::options::RefetchTrigger::Always),
        );

        client.mount();
        client.mount(); // ref-counted: second mount must not spawn a watcher
        client.unmount();
        client.unmount();

        // Reconnect watcher fires only on offline -> online transitions.
        client.online_manager().set_online(false);
        client.mount();
        client.online_manager().set_online(true);
        // Give the watcher task a moment to run the reconnect pass.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(client.is_stale(&key));
        client.unmount();
    }

    #[test]
    fn test_handle_reconnect_marks_reconnectable_entries_stale() {
        let client = QueryClient::new();
        let always = QueryKey::new("reconnect").with("id", 1);
        let disabled = QueryKey::new("reconnect").with("id", 2);
        let enabled_fresh = QueryKey::new("reconnect").with("id", 3);
        client.set_query_data(
            &always,
            "a".to_string(),
            QueryOptions::default().refetch_on_reconnect(crate::options::RefetchTrigger::Always),
        );
        client.set_query_data(
            &disabled,
            "b".to_string(),
            QueryOptions::default().refetch_on_reconnect(crate::options::RefetchTrigger::Disabled),
        );
        // Default trigger (`Enabled`) only refetches *stale* data, so fresh
        // data is left alone (TanStack shouldFetchOnReconnect semantics).
        client.set_query_data(&enabled_fresh, "c".to_string(), QueryOptions::default());

        client.handle_reconnect();

        assert!(client.is_stale(&always));
        assert!(!client.is_stale(&disabled));
        assert!(!client.is_stale(&enabled_fresh));
    }

    #[tokio::test]
    async fn test_query_snapshots_filters() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("users").with("id", 1);
        let key2 = QueryKey::new("posts");
        client.set_query_data(&key1, "a".to_string(), QueryOptions::default());
        client.set_query_data(&key2, "b".to_string(), QueryOptions::default());
        client.set_in_flight(&key1, dummy_task());

        let users = QueryFilters::with_key(QueryKey::new("users"));
        let snapshots = client.query_snapshots(&users);
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        assert_eq!(snapshot.key.cache_key(), key1.cache_key());
        assert!(snapshot.is_active, "in-flight + enabled counts as active");
        assert_eq!(snapshot.fetch_status, FetchStatus::Fetching);
        assert_eq!(snapshot.status, QueryStatus::Success);
        assert!(snapshot.has_data);

        let all = client.query_snapshots(&QueryFilters::all());
        assert_eq!(all.len(), 2);
        let posts = all
            .iter()
            .find(|s| s.key.cache_key() == key2.cache_key())
            .unwrap();
        assert!(!posts.is_active);
        assert_eq!(posts.fetch_status, FetchStatus::Idle);
    }

    #[test]
    fn test_query_snapshots_exact_filter() {
        let client = QueryClient::new();
        let key1 = QueryKey::new("users").with("id", 1);
        let key2 = QueryKey::new("users").with("id", 2);
        client.set_query_data(&key1, "a".to_string(), QueryOptions::default());
        client.set_query_data(&key2, "b".to_string(), QueryOptions::default());

        let exact = QueryFilters::with_key(key1.clone()).exact();
        let snapshots = client.query_snapshots(&exact);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].key.cache_key(), key1.cache_key());
    }

    // ----- Coverage: elapsed stale_time in snapshots / query state -----

    #[test]
    fn test_snapshots_and_query_state_report_elapsed_stale_time() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let options = QueryOptions {
            stale_time: Duration::from_millis(5),
            ..Default::default()
        };
        client.set_query_data(&key, "data".to_string(), options);
        std::thread::sleep(Duration::from_millis(20));

        let stale_filter = QueryFilters::with_key(QueryKey::new("test")).stale();
        let snapshots = client.query_snapshots(&stale_filter);
        assert_eq!(snapshots.len(), 1, "entry became stale by elapsed time");
        assert!(snapshots[0].is_stale);

        let state = client.get_query_state(&key).expect("cached key");
        assert!(state.is_stale);
    }

    // ----- Coverage: prefetch_query error path with a subscriber installed -----

    /// A no-op tracing subscriber that accepts every event, so `debug!`
    /// callsites evaluate as enabled during the prefetch logging test.
    struct RecordingSubscriber;

    impl tracing::Subscriber for RecordingSubscriber {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, _event: &tracing::Event<'_>) {}

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    #[test]
    fn test_prefetch_query_logs_error_when_subscriber_installed() {
        // `tracing` macros skip field formatting when no subscriber is
        // installed. Installing a *global* subscriber additionally rebuilds
        // the callsite interest cache and raises the max-level hint, so the
        // `debug!` event in `prefetch_query` dispatches even though parallel
        // tests may have evaluated that callsite without any subscriber.
        // Setting races with parallel tests, hence the `let _ =`; the
        // `with_default` scope below works with whichever global won.
        let _ = tracing::subscriber::set_global_default(RecordingSubscriber);

        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let query: Query<String> =
            Query::new(key, || async { Err(QueryError::custom("logged boom")) });

        tracing::subscriber::with_default(RecordingSubscriber, || {
            futures::executor::block_on(client.prefetch_query(&query));

            // Drive every `Subscriber` callback of the recorder: spans invoke
            // `new_span`, `enter`, `record` and `exit`, `follows_from`
            // invokes `record_follows_from`, and the events above (plus the
            // `debug!` below) invoke `event`.
            let span = tracing::span!(
                tracing::Level::DEBUG,
                "rs_query_test",
                answer = tracing::field::Empty
            );
            let _guard = span.enter();
            span.record("answer", 42);
            let parent = tracing::span!(tracing::Level::DEBUG, "rs_query_test_parent");
            parent.follows_from(&span);
            tracing::debug!("rs_query coverage: event dispatched to subscriber");
        });
    }

    // ----- Coverage: entries vanishing between the key snapshot and lookup -----

    /// Drive a remover thread that keeps dropping cache entries while `run`
    /// re-seeds and executes `f` repeatedly. Every iteration gives the remover
    /// another chance to delete an entry *after* the bulk operation collected
    /// its key snapshot but *before* the per-key lookup, exercising the
    /// "entry vanished mid-scan" guards (`refetch_all_stale`,
    /// `refetch_queries`, `handle_reconnect`). Locks are only ever taken one
    /// shard at a time and never while waiting on the other thread, so the
    /// interleaving cannot deadlock.
    fn run_with_concurrent_remover(run: impl Fn(&QueryClient)) {
        let client = QueryClient::new();
        let keys: Vec<String> = (0..64).map(|i| format!("race-removal-{i}")).collect();
        let stop = Arc::new(AtomicBool::new(false));
        let remover_cache = Arc::clone(&client.cache);
        let remover_keys = keys.clone();
        let remover_stop = Arc::clone(&stop);
        let remover = std::thread::spawn(move || {
            let mut rounds = 0usize;
            while rounds < 50_000 && !remover_stop.load(Ordering::Relaxed) {
                let key = &remover_keys[rounds % remover_keys.len()];
                remover_cache.remove(key.as_str());
                rounds += 1;
                std::thread::yield_now();
            }
        });

        for _ in 0..256 {
            for key in &keys {
                client.set_query_data(
                    &QueryKey::new(key.clone()),
                    "data".to_string(),
                    QueryOptions::default(),
                );
            }
            run(&client);
        }

        stop.store(true, Ordering::Relaxed);
        remover.join().expect("remover thread should stop");
    }

    #[test]
    fn test_refetch_all_stale_with_entry_removed_mid_scan() {
        // Entries stay fresh, so the only reachable guard for a seeded key is
        // the "not in the cache anymore" skip inside `refetch_all_stale`.
        run_with_concurrent_remover(super::QueryClient::refetch_all_stale);
    }

    #[test]
    fn test_refetch_queries_with_entry_removed_mid_scan() {
        run_with_concurrent_remover(|client| {
            let filters = QueryFilters {
                query_key: Some(QueryKey::new("race-removal-0")),
                ..Default::default()
            };
            let triggered = client.refetch_queries(filters, RefetchQueryFilters::default());
            // Entries that survived the whole scan are triggered; entries
            // that vanished between the snapshot and the lookup are skipped.
            assert!(triggered <= 64);
        });
    }

    #[test]
    fn test_handle_reconnect_with_entry_removed_mid_scan() {
        run_with_concurrent_remover(super::QueryClient::handle_reconnect);
    }

    // ----- Coverage: merge_options override matrix -----

    #[test]
    fn test_merge_options_every_provided_override_wins() {
        let defaults = QueryOptions::default().stale_time(Duration::from_secs(120));
        let provided = QueryOptions {
            stale_time: Duration::from_secs(5),
            stale_time_static: true,
            gc_time: Duration::from_secs(60),
            retry: crate::options::RetryConfig::new(5),
            enabled: false,
            enabled_fn: Some(Arc::new(|| false)),
            refetch_on_window_focus: crate::options::RefetchTrigger::Always,
            refetch_on_reconnect: crate::options::RefetchTrigger::Always,
            refetch_on_mount: crate::options::RefetchTrigger::Always,
            refetch_interval: Duration::from_secs(30),
            refetch_interval_in_background: true,
            structural_sharing: false,
            network_mode: crate::network::NetworkMode::Always,
            retry_on_mount: false,
            throw_on_error: true,
            notify_on_change_props: Some(vec!["a".to_string(), "b".to_string()]),
            meta: std::collections::BTreeMap::from([("k".to_string(), "v".to_string())]),
        };

        let merged = QueryClient::merge_options(&defaults, &provided);

        assert_eq!(merged.stale_time, Duration::from_secs(5));
        assert!(merged.stale_time_static);
        assert_eq!(merged.gc_time, Duration::from_secs(60));
        assert_eq!(merged.retry.max_retries, 5);
        assert!(!merged.enabled);
        let enabled_fn = merged.enabled_fn.expect("enabled_fn override");
        assert!(!enabled_fn());
        assert_eq!(
            merged.refetch_on_window_focus,
            crate::options::RefetchTrigger::Always
        );
        assert_eq!(
            merged.refetch_on_reconnect,
            crate::options::RefetchTrigger::Always
        );
        assert_eq!(
            merged.refetch_on_mount,
            crate::options::RefetchTrigger::Always
        );
        assert_eq!(merged.refetch_interval, Duration::from_secs(30));
        assert!(merged.refetch_interval_in_background);
        assert!(!merged.structural_sharing);
        assert_eq!(merged.network_mode, crate::network::NetworkMode::Always);
        assert!(!merged.retry_on_mount);
        assert!(merged.throw_on_error);
        assert_eq!(
            merged.notify_on_change_props,
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(merged.meta.get("k"), Some(&"v".to_string()));
    }

    #[test]
    fn test_default_options_merge_uses_caller_overrides_end_to_end() {
        let client = QueryClient::new();
        client.set_default_options(
            QueryOptions::default()
                .stale_time(Duration::from_secs(90))
                .gc_time(Duration::from_secs(45)),
        );

        let key = QueryKey::new("merged");
        client.set_query_data(
            &key,
            "data".to_string(),
            QueryOptions {
                gc_time: Duration::from_secs(10),
                network_mode: crate::network::NetworkMode::Always,
                ..QueryOptions::default()
            },
        );

        let stored = client.get_query_options(&key).unwrap();
        // Caller left stale_time at the crate default -> client default wins.
        assert_eq!(stored.stale_time, Duration::from_secs(90));
        // Caller-provided fields override the client defaults.
        assert_eq!(stored.gc_time, Duration::from_secs(10));
        assert_eq!(stored.network_mode, crate::network::NetworkMode::Always);
    }
}
