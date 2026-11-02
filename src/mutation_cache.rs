// src/mutation_cache.rs
//! Mutation cache, mirroring `TanStack` Query's `mutationCache.ts`.
//!
//! Stores a snapshot entry for every submitted mutation, fans lifecycle events
//! out through a bounded broadcast bus ([`crate::bus`]) and serializes scoped
//! mutations: mutations sharing a [`MutationScope`] run one at a time; later
//! submissions are added as paused and queued until the current one finishes.
//!
//! Cache-level config callbacks ([`MutationCacheConfig`]) receive an immutable
//! snapshot of the entry and run *outside* the cache locks, so they may safely
//! call back into the cache without re-entrancy hazards. Per `TanStack` they
//! run *before* the corresponding per-mutation option callbacks.

use crate::bus;
use crate::error::QueryError;
use crate::filters::{MutationFilters, MutationSnapshot, match_mutation};
use crate::key::QueryKey;
use crate::state::MutationStatus;
use crate::watch::{Watch, Watcher};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Broadcast capacity for [`MutationCacheEvent`] deliveries.
const EVENT_CAPACITY: usize = 256;

/// Identifies a mutation scope; mutations sharing an id run in serial.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MutationScope {
    /// Unique id of the scope.
    pub id: String,
}

impl MutationScope {
    /// Create a scope with the given id.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

/// Callback invoked with a snapshot of a mutation cache entry.
///
/// Callbacks return `()` and therefore cannot fail; they are invoked outside
/// the cache locks (no re-entrancy) with a [`MutationCacheEntry`] snapshot.
pub type MutationCacheCallback = Arc<dyn Fn(&MutationCacheEntry) + Send + Sync>;

/// Cache-level mutation callbacks, mirroring `TanStack`'s `MutationCacheConfig`.
///
/// Per `TanStack` ordering these fire *before* the matching per-mutation option
/// callbacks.
#[derive(Clone, Default)]
pub struct MutationCacheConfig {
    /// Invoked when a mutation is submitted (before the option-level
    /// `on_mutate`).
    pub on_mutate: Option<MutationCacheCallback>,
    /// Invoked after a mutation succeeded.
    pub on_success: Option<MutationCacheCallback>,
    /// Invoked after a mutation failed.
    pub on_error: Option<MutationCacheCallback>,
    /// Invoked after the mutation settled (success or error).
    pub on_settled: Option<MutationCacheCallback>,
}

impl std::fmt::Debug for MutationCacheConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("MutationCacheConfig");
        for (name, callback) in [
            ("on_mutate", &self.on_mutate),
            ("on_success", &self.on_success),
            ("on_error", &self.on_error),
            ("on_settled", &self.on_settled),
        ] {
            if callback.is_some() {
                debug.field(name, &"<fn>");
            }
        }
        debug.finish()
    }
}

/// Immutable snapshot of a submitted mutation, mirroring the parts of
/// `TanStack`'s `Mutation` state the cache exposes.
#[derive(Clone, Debug)]
pub struct MutationCacheEntry {
    /// Cache-assigned id (monotonic, starts at 1).
    pub id: u64,
    /// Mutation key, if any.
    pub mutation_key: Option<QueryKey>,
    /// Scope, if any.
    pub scope: Option<MutationScope>,
    /// Aggregate status.
    pub status: MutationStatus,
    /// Whether the mutation is paused (offline, or waiting for its scope).
    pub is_paused: bool,
    /// Wall-clock submission time in milliseconds since the Unix epoch.
    pub submitted_at_ms: Option<u64>,
    /// Whether the mutation produced a success result.
    pub has_result: bool,
    /// The terminal error, if any.
    pub error: Option<QueryError>,
    /// Whether `on_mutate` produced an optimistic context/rollback.
    pub has_context: bool,
}

impl MutationCacheEntry {
    /// Build a fresh, idle entry without a submission timestamp.
    #[must_use]
    pub fn new(mutation_key: Option<QueryKey>, scope: Option<MutationScope>) -> Self {
        Self {
            id: 0,
            mutation_key,
            scope,
            status: MutationStatus::Idle,
            is_paused: false,
            submitted_at_ms: None,
            has_result: false,
            error: None,
            has_context: false,
        }
    }

    /// Project the filter-matching snapshot for this entry.
    #[must_use]
    pub fn snapshot(&self) -> MutationSnapshot {
        MutationSnapshot::new(self.mutation_key.clone(), self.status, self.is_paused)
    }
}

/// Lifecycle event broadcast by the [`MutationCache`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MutationCacheEvent {
    /// A mutation entry was added (carries the entry id).
    Added(u64),
    /// A mutation entry was removed (carries the entry id).
    Removed(u64),
    /// A mutation entry changed state (carries the entry id).
    Updated(u64),
    /// The next queued mutation of a scope may run (carries the scope id).
    ScopeResumed(String),
    /// The cache was cleared.
    Cleared,
}

/// Per-scope run state: who is running and who waits.
#[derive(Default)]
struct ScopeState {
    running: bool,
    queue: VecDeque<u64>,
}

/// Cache of mutation entries, mirroring `TanStack`'s `MutationCache`.
///
/// All methods are `&self`; interior mutability keeps the cache shareable as
/// `Arc<MutationCache>` across executors and observers.
#[derive(Clone)]
pub struct MutationCache {
    entries: Arc<Mutex<Vec<MutationCacheEntry>>>,
    scopes: Arc<Mutex<HashMap<String, ScopeState>>>,
    next_id: Arc<AtomicU64>,
    config: Arc<MutationCacheConfig>,
    tx: Arc<bus::Sender<MutationCacheEvent>>,
    /// Per-scope release signals: waiting mutations park on the scope's
    /// watcher instead of polling.
    scope_release: Arc<Mutex<HashMap<String, Watch<()>>>>,
}

impl Default for MutationCache {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for MutationCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MutationCache")
            .field("len", &self.len())
            .field(
                "scopes",
                &self
                    .scopes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .keys()
                    .collect::<Vec<_>>(),
            )
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// Current wall-clock time in milliseconds since the Unix epoch.
#[must_use]
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

impl MutationCache {
    /// Create an empty cache without config callbacks.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(MutationCacheConfig::default())
    }

    /// Create an empty cache with the given config callbacks.
    #[must_use]
    pub fn with_config(config: MutationCacheConfig) -> Self {
        let (tx, _rx) = bus::Sender::channel(EVENT_CAPACITY);
        Self {
            entries: Arc::new(Mutex::new(Vec::new())),
            scopes: Arc::new(Mutex::new(HashMap::new())),
            scope_release: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(0)),
            config: Arc::new(config),
            tx: Arc::new(tx),
        }
    }

    /// The cache-level callbacks configured for this cache.
    #[must_use]
    pub fn config(&self) -> &MutationCacheConfig {
        &self.config
    }

    /// Subscribe to cache lifecycle events.
    #[must_use]
    pub fn subscribe(&self) -> bus::Receiver<MutationCacheEvent> {
        self.tx.subscribe()
    }

    /// Assign an id to `entry`, insert it and notify `Added` subscribers.
    ///
    /// If the entry carries a scope that currently has a running mutation, the
    /// entry is inserted as paused and queued behind it (`TanStack` scope
    /// semantics). A paused flag already set by the caller (e.g. offline) is
    /// never cleared here.
    #[must_use]
    pub fn add(&self, entry: MutationCacheEntry) -> u64 {
        self.add_with_hook(entry, None)
    }

    /// TanStack-parity alias for [`MutationCache::add`].
    #[must_use]
    pub fn build(&self, entry: MutationCacheEntry) -> u64 {
        self.add(entry)
    }

    /// [`MutationCache::add`] with a hook invoked with the assigned id
    /// *before* the `Added` event is broadcast, so observers can correlate
    /// events race-free.
    pub(crate) fn add_with_hook(
        &self,
        mut entry: MutationCacheEntry,
        on_assigned: Option<&(dyn Fn(u64) + Send + Sync)>,
    ) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        entry.id = id;
        if let Some(scope) = &entry.scope {
            let mut scopes = self
                .scopes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = scopes.entry(scope.id.clone()).or_default();
            if state.running {
                entry.is_paused = true;
                state.queue.push_back(id);
            }
        }
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(entry);
        if let Some(hook) = on_assigned {
            hook(id);
        }
        self.tx.send(&MutationCacheEvent::Added(id));
        id
    }

    /// Apply `apply` to the entry with the given id and notify `Updated`.
    ///
    /// Returns `false` (and notifies nothing) when no entry matches.
    pub fn update(&self, id: u64, apply: impl FnOnce(&mut MutationCacheEntry)) -> bool {
        let found = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match entries.iter_mut().find(|entry| entry.id == id) {
                Some(entry) => {
                    apply(entry);
                    true
                }
                None => false,
            }
        };
        if found {
            self.tx.send(&MutationCacheEvent::Updated(id));
        }
        found
    }

    /// Remove the entry with the given id; splices it out of its scope queue.
    ///
    /// Mirroring `TanStack`, a `Removed` event is emitted even when the id was
    /// not present. Returns whether an entry was actually removed.
    pub fn remove(&self, id: u64) -> bool {
        let removed = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match entries.iter().position(|entry| entry.id == id) {
                Some(index) => {
                    let entry = entries.remove(index);
                    Some(entry.scope.map(|scope| scope.id))
                }
                None => None,
            }
        };
        let found = removed.is_some();
        if let Some(Some(scope)) = removed {
            let mut scopes = self
                .scopes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(state) = scopes.get_mut(&scope) {
                state.queue.retain(|queued| *queued != id);
            }
        }
        self.tx.send(&MutationCacheEvent::Removed(id));
        found
    }

    /// Remove every entry, splice every scope queue and notify `Removed` per
    /// entry followed by a single [`MutationCacheEvent::Cleared`].
    pub fn clear(&self) {
        let ids = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let ids: Vec<u64> = entries.iter().map(|entry| entry.id).collect();
            entries.clear();
            ids
        };
        self.scopes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        for id in ids {
            self.tx.send(&MutationCacheEvent::Removed(id));
        }
        self.tx.send(&MutationCacheEvent::Cleared);
    }

    /// Snapshot of every entry (insertion order).
    #[must_use]
    pub fn get_all(&self) -> Vec<MutationCacheEntry> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Snapshot of the entry with the given id, if present.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<MutationCacheEntry> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
    }

    /// First entry matching `filters` (prefix key match unless exact).
    #[must_use]
    pub fn find(&self, filters: &MutationFilters) -> Option<MutationCacheEntry> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|entry| match_mutation(&entry.snapshot(), filters))
            .cloned()
    }

    /// Every entry matching `filters` (prefix key match unless exact).
    #[must_use]
    pub fn find_all(&self, filters: &MutationFilters) -> Vec<MutationCacheEntry> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|entry| match_mutation(&entry.snapshot(), filters))
            .cloned()
            .collect()
    }

    /// Number of cached entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Whether the cache holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether a mutation may start in `scope` right now: `true` when no other
    /// mutation of that scope is running (unscoped callers need not ask).
    #[must_use]
    pub fn can_run(&self, scope: &str) -> bool {
        !self
            .scopes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(scope)
            .is_some_and(|state| state.running)
    }

    /// Mark `scope` as occupied by the calling mutation.
    pub fn mark_running(&self, scope: &str) {
        self.scopes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(scope.to_string())
            .or_default()
            .running = true;
    }

    /// Release `scope`; the calling mutation has finished.
    ///
    /// Wakes every mutation parked on [`MutationCache::subscribe_scope`].
    pub fn mark_finished(&self, scope: &str) {
        {
            let mut scopes = self
                .scopes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(state) = scopes.get_mut(scope) {
                state.running = false;
                if state.queue.is_empty() {
                    scopes.remove(scope);
                }
            }
        }
        self.notify_scope(scope);
    }

    /// A watcher that resolves when `scope` is released (`mark_finished` /
    /// `run_next`). The scope gate parks on this instead of polling.
    #[must_use]
    pub fn subscribe_scope(&self, scope: &str) -> Watcher<()> {
        let mut release = self
            .scope_release
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(watch) = release.get(scope) {
            return watch.subscribe();
        }
        let (watch, _rx) = Watch::new(());
        release.insert(scope.to_string(), watch.clone());
        watch.subscribe()
    }

    fn notify_scope(&self, scope: &str) {
        let watch = {
            let release = self
                .scope_release
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match release.get(scope) {
                Some(watch) => watch.clone(),
                None => return,
            }
        };
        watch.send(());
    }

    /// Pop the next queued mutation id of `scope` and notify `ScopeResumed`.
    ///
    /// Waiting mutations park on [`MutationCache::subscribe_scope`]; the event
    /// is a notification for observers, not the wakeup primitive itself.
    pub fn run_next(&self, scope: &str) -> Option<u64> {
        let next = self
            .scopes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(scope)
            .and_then(|state| state.queue.pop_front());
        if next.is_some() {
            self.tx
                .send(&MutationCacheEvent::ScopeResumed(scope.to_string()));
            self.notify_scope(scope);
        }
        next
    }

    /// Unpause every paused entry, notify `Updated` per entry and return how
    /// many entries were resumed.
    pub fn resume_paused_mutations(&self) -> usize {
        let resumed = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut resumed = Vec::new();
            for entry in entries.iter_mut() {
                if entry.is_paused {
                    entry.is_paused = false;
                    resumed.push(entry.id);
                }
            }
            resumed
        };
        for id in &resumed {
            self.tx.send(&MutationCacheEvent::Updated(*id));
        }
        resumed.len()
    }

    /// Fire the cache-level `on_mutate` config callback with an entry
    /// snapshot; a no-op when unset or the id is unknown.
    pub fn fire_on_mutate(&self, id: u64) {
        if let Some(callback) = &self.config.on_mutate
            && let Some(entry) = self.get(id)
        {
            callback(&entry);
        }
    }

    /// Fire the cache-level `on_success` config callback with an entry
    /// snapshot; a no-op when unset or the id is unknown.
    pub fn fire_on_success(&self, id: u64) {
        if let Some(callback) = &self.config.on_success
            && let Some(entry) = self.get(id)
        {
            callback(&entry);
        }
    }

    /// Fire the cache-level `on_error` config callback with an entry snapshot;
    /// a no-op when unset or the id is unknown.
    pub fn fire_on_error(&self, id: u64) {
        if let Some(callback) = &self.config.on_error
            && let Some(entry) = self.get(id)
        {
            callback(&entry);
        }
    }

    /// Fire the cache-level `on_settled` config callback with an entry
    /// snapshot; a no-op when unset or the id is unknown.
    pub fn fire_on_settled(&self, id: u64) {
        if let Some(callback) = &self.config.on_settled
            && let Some(entry) = self.get(id)
        {
            callback(&entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::QueryError;
    use crate::key::QueryKey;
    use crate::state::MutationStatus;

    fn keyed_entry(key: &str) -> MutationCacheEntry {
        MutationCacheEntry::new(Some(QueryKey::new(key)), None)
    }

    fn drain(rx: &mut crate::bus::Receiver<MutationCacheEvent>) -> Vec<MutationCacheEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    #[test]
    fn test_scope_new_and_defaults() {
        assert_eq!(
            MutationScope::new("hero"),
            MutationScope {
                id: "hero".to_string()
            }
        );
        assert_eq!(MutationScope::default().id, "");
    }

    #[test]
    fn test_entry_new_defaults() {
        let entry =
            MutationCacheEntry::new(Some(QueryKey::new("k")), Some(MutationScope::new("s")));
        assert_eq!(entry.id, 0);
        assert_eq!(entry.status, MutationStatus::Idle);
        assert!(!entry.is_paused);
        assert!(entry.submitted_at_ms.is_none());
        assert!(!entry.has_result);
        assert!(entry.error.is_none());
        assert!(!entry.has_context);
        let snapshot = entry.snapshot();
        assert_eq!(snapshot.mutation_key, Some(QueryKey::new("k")));
        assert_eq!(snapshot.status, MutationStatus::Idle);
        assert!(!snapshot.is_paused);
    }

    #[test]
    fn test_add_assigns_ids_and_notifies() {
        let cache = MutationCache::new();
        let mut rx = cache.subscribe();

        let first = cache.add(keyed_entry("posts"));
        let second = cache.add(keyed_entry("users"));
        assert_eq!(first, 1);
        assert_eq!(second, 2);
        assert_eq!(cache.len(), 2);
        assert!(!cache.is_empty());

        assert_eq!(cache.get(first).map(|e| e.id), Some(first));
        assert_eq!(
            drain(&mut rx),
            vec![MutationCacheEvent::Added(1), MutationCacheEvent::Added(2)]
        );
    }

    #[test]
    fn test_build_is_add_alias() {
        let cache = MutationCache::new();
        let id = cache.build(keyed_entry("posts"));
        assert_eq!(id, 1);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_find_and_find_all_with_filters() {
        let cache = MutationCache::new();

        let mut success = keyed_entry("posts");
        success.status = MutationStatus::Success;
        success.has_result = true;
        let _ = cache.add(success);

        let mut pending = keyed_entry("posts");
        pending.mutation_key = Some(QueryKey::new("posts").segment("detail"));
        pending.status = MutationStatus::Pending;
        let _ = cache.add(pending);

        let mut errored = keyed_entry("users");
        errored.status = MutationStatus::Error;
        errored.error = Some(QueryError::custom("boom"));
        errored.is_paused = true;
        let _ = cache.add(errored);

        assert_eq!(cache.find_all(&MutationFilters::all()).len(), 3);
        assert_eq!(
            cache
                .find_all(&MutationFilters::with_key(QueryKey::new("posts")))
                .len(),
            2
        );
        assert_eq!(
            cache
                .find_all(&MutationFilters::with_key(QueryKey::new("posts")).exact())
                .len(),
            1
        );
        assert_eq!(
            cache
                .find_all(&MutationFilters::all().status(MutationStatus::Error))
                .len(),
            1
        );
        assert_eq!(cache.find_all(&MutationFilters::all().paused()).len(), 1);

        let found = cache.find(&MutationFilters::with_key(QueryKey::new("users")));
        assert_eq!(
            found.as_ref().map(|entry| entry.mutation_key.clone()),
            Some(Some(QueryKey::new("users")))
        );
        assert_eq!(
            found.and_then(|entry| entry.error),
            Some(QueryError::custom("boom"))
        );
        assert!(
            cache
                .find(&MutationFilters::with_key(QueryKey::new("nope")))
                .is_none()
        );
    }

    #[test]
    fn test_remove_notifies_even_when_absent() {
        let cache = MutationCache::new();
        let mut rx = cache.subscribe();
        let id = cache.add(keyed_entry("posts"));

        assert!(cache.remove(id));
        assert!(cache.get(id).is_none());
        assert_eq!(cache.len(), 0);
        // TanStack parity: removing an unknown id still notifies.
        assert!(!cache.remove(999));

        assert_eq!(
            drain(&mut rx),
            vec![
                MutationCacheEvent::Added(id),
                MutationCacheEvent::Removed(id),
                MutationCacheEvent::Removed(999),
            ]
        );
    }

    #[test]
    fn test_remove_splices_scope_queue() {
        let cache = MutationCache::new();
        cache.mark_running("s");

        let kept = cache.add(MutationCacheEntry::new(None, Some(MutationScope::new("s"))));
        let dropped = cache.add(MutationCacheEntry::new(None, Some(MutationScope::new("s"))));
        assert!(cache.get(kept).is_some_and(|e| e.is_paused));
        assert!(cache.get(dropped).is_some_and(|e| e.is_paused));

        cache.remove(dropped);
        assert_eq!(cache.run_next("s"), Some(kept));
        assert_eq!(cache.run_next("s"), None);
    }

    #[test]
    fn test_clear_notifies_removed_per_entry_then_cleared() {
        let cache = MutationCache::new();
        let mut rx = cache.subscribe();
        let first = cache.add(keyed_entry("posts"));
        let second = cache.add(keyed_entry("users"));

        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(
            drain(&mut rx),
            vec![
                MutationCacheEvent::Added(first),
                MutationCacheEvent::Added(second),
                MutationCacheEvent::Removed(first),
                MutationCacheEvent::Removed(second),
                MutationCacheEvent::Cleared,
            ]
        );
    }

    #[test]
    fn test_update_broadcasts_only_when_found() {
        let cache = MutationCache::new();
        let mut rx = cache.subscribe();
        let id = cache.add(keyed_entry("posts"));

        assert!(cache.update(id, |entry| {
            entry.status = MutationStatus::Success;
            entry.has_result = true;
        }));
        assert!(!cache.update(999, |entry| entry.has_result = true));

        let entry = cache.get(id).expect("entry exists");
        assert_eq!(entry.status, MutationStatus::Success);
        assert!(entry.has_result);
        assert_eq!(
            drain(&mut rx),
            vec![
                MutationCacheEvent::Added(id),
                MutationCacheEvent::Updated(id)
            ]
        );
    }

    #[test]
    fn test_scope_can_run_mark_running_finished() {
        let cache = MutationCache::new();
        let mut rx = cache.subscribe();

        assert!(cache.can_run("s"));
        cache.mark_running("s");
        assert!(!cache.can_run("s"));

        // A mutation added while the scope runs is paused and queued.
        let id = cache.add(MutationCacheEntry::new(None, Some(MutationScope::new("s"))));
        assert!(cache.get(id).is_some_and(|e| e.is_paused));

        cache.mark_finished("s");
        assert!(cache.can_run("s"));
        assert_eq!(cache.run_next("s"), Some(id));
        assert_eq!(cache.run_next("s"), None);
        assert_eq!(
            drain(&mut rx).last(),
            Some(&MutationCacheEvent::ScopeResumed("s".to_string()))
        );
    }

    #[test]
    fn test_mark_finished_drops_empty_scope() {
        let cache = MutationCache::new();
        cache.mark_running("s");
        cache.mark_finished("s");
        assert!(cache.can_run("s"));
        // No state left behind for the finished scope.
        assert!(
            !cache
                .scopes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key("s")
        );
    }

    #[test]
    fn test_resume_paused_mutations() {
        let cache = MutationCache::new();
        let mut rx = cache.subscribe();

        let mut offline = keyed_entry("posts");
        offline.is_paused = true;
        let _ = cache.add(offline);

        let mut queued = MutationCacheEntry::new(None, Some(MutationScope::new("s")));
        queued.is_paused = true;
        let _ = cache.add(queued);

        let _ = cache.add(keyed_entry("users"));

        assert_eq!(cache.resume_paused_mutations(), 2);
        assert!(cache.get_all().iter().all(|entry| !entry.is_paused));
        // Resuming again is a no-op.
        assert_eq!(cache.resume_paused_mutations(), 0);

        let events = drain(&mut rx);
        assert_eq!(events.len(), 5, "3 added + 2 resumed");
        assert!(
            events[3..]
                .iter()
                .all(|event| matches!(event, MutationCacheEvent::Updated(_)))
        );
    }

    #[test]
    fn test_config_callbacks_receive_entry_snapshots() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let success_seen = Arc::clone(&seen);
        let settled_seen = Arc::clone(&seen);
        let cache = MutationCache::with_config(MutationCacheConfig {
            on_success: Some(Arc::new(move |entry: &MutationCacheEntry| {
                success_seen
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(format!("success:{}:{}", entry.id, entry.status.as_str()));
            })),
            on_settled: Some(Arc::new(move |entry: &MutationCacheEntry| {
                settled_seen
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(format!("settled:{}", entry.id));
            })),
            ..MutationCacheConfig::default()
        });

        let id = cache.add(keyed_entry("posts"));
        cache.update(id, |entry| {
            entry.status = MutationStatus::Success;
            entry.has_result = true;
        });

        // fire_* with an unknown id is a no-op.
        cache.fire_on_success(999);

        cache.fire_on_success(id);
        cache.fire_on_settled(id);
        assert_eq!(
            *seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["success:1:success".to_string(), "settled:1".to_string()]
        );
    }

    #[test]
    fn test_config_debug_hides_closures() {
        let plain = MutationCacheConfig::default();
        assert!(format!("{plain:?}").contains("MutationCacheConfig"));
        let with_cb = MutationCacheConfig {
            on_error: Some(Arc::new(|_entry: &MutationCacheEntry| {})),
            ..MutationCacheConfig::default()
        };
        let debug = format!("{with_cb:?}");
        assert!(debug.contains("on_error"));
        assert!(debug.contains("<fn>"));
    }

    #[tokio::test]
    async fn test_two_scoped_mutations_run_sequentially() {
        use crate::client::QueryClient;
        use crate::mutation::Mutation;
        use std::sync::atomic::AtomicUsize;
        use std::time::Duration;

        let cache = Arc::new(MutationCache::new());
        let mut rx = cache.subscribe();
        let client = QueryClient::new();

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
        .scope(MutationScope::new("hero"));

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
        .scope(MutationScope::new("hero"));

        let first_handle = {
            let cache = Arc::clone(&cache);
            let client = client.clone();
            tokio::spawn(async move {
                crate::mutation::execute_mutation_full(&client, &cache, &first, 1, None).await
            })
        };
        // Let the first mutation acquire the scope and park in its mutate_fn.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let second_handle = {
            let cache = Arc::clone(&cache);
            let client = client.clone();
            tokio::spawn(async move {
                crate::mutation::execute_mutation_full(&client, &cache, &second, 2, None).await
            })
        };
        // Let the second mutation submit and pause behind the scope.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let paused: Vec<MutationCacheEntry> = cache
            .find_all(&MutationFilters::all().paused())
            .into_iter()
            .filter(|entry| entry.scope.is_some())
            .collect();
        assert_eq!(paused.len(), 1, "the queued mutation is paused");
        assert_eq!(live.load(Ordering::SeqCst), 1, "only the first runs");

        release.notify_one();

        let first_result = first_handle
            .await
            .expect("first task joins")
            .expect("first succeeds");
        let second_result = second_handle
            .await
            .expect("second task joins")
            .expect("second succeeds");
        assert_eq!(first_result, "first");
        assert_eq!(second_result, "second");

        assert_eq!(
            max_live.load(Ordering::SeqCst),
            1,
            "scope serializes execution"
        );
        let events = drain(&mut rx);
        assert!(
            events.contains(&MutationCacheEvent::ScopeResumed("hero".to_string())),
            "scope resume is broadcast: {events:?}"
        );
    }

    #[test]
    fn test_default_creates_empty_cache() {
        let cache = MutationCache::default();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_debug_lists_len_scopes_and_config() {
        let cache = MutationCache::new();
        cache.mark_running("hero");
        let _ = cache.add(keyed_entry("posts"));

        let debug = format!("{cache:?}");
        assert!(debug.contains("MutationCache"), "{debug}");
        assert!(debug.contains("len: 1"), "{debug}");
        assert!(debug.contains("hero"), "{debug}");
        assert!(debug.contains("config"), "{debug}");
    }

    #[test]
    fn test_config_accessor_exposes_callbacks() {
        let plain = MutationCache::new();
        assert!(plain.config().on_mutate.is_none());
        assert!(plain.config().on_success.is_none());
        assert!(plain.config().on_error.is_none());
        assert!(plain.config().on_settled.is_none());

        let cache = MutationCache::with_config(MutationCacheConfig {
            on_success: Some(Arc::new(|_entry: &MutationCacheEntry| {})),
            ..MutationCacheConfig::default()
        });
        assert!(cache.config().on_success.is_some());
    }

    #[test]
    fn test_mark_finished_for_unknown_scope_is_noop() {
        let cache = MutationCache::new();
        // No state was ever created for this scope.
        cache.mark_finished("ghost");
        assert!(cache.can_run("ghost"));
    }

    #[test]
    fn test_remove_after_scope_state_gone_does_not_panic() {
        let cache = MutationCache::new();
        cache.mark_running("s");
        let id = cache.add(MutationCacheEntry::new(None, Some(MutationScope::new("s"))));
        assert_eq!(cache.run_next("s"), Some(id));
        // Finishing with an empty queue removes the scope state entirely while
        // the entry itself stays in the cache.
        cache.mark_finished("s");
        assert!(cache.can_run("s"));
        // Removing the orphaned entry must tolerate the missing scope state.
        assert!(cache.remove(id));
        assert!(cache.get(id).is_none());
    }

    #[test]
    fn test_config_on_mutate_and_on_error_fire_with_entry_snapshot() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mutate_seen = Arc::clone(&seen);
        let error_seen = Arc::clone(&seen);
        let cache = MutationCache::with_config(MutationCacheConfig {
            on_mutate: Some(Arc::new(move |entry: &MutationCacheEntry| {
                mutate_seen
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(format!("mutate:{}", entry.id));
            })),
            on_error: Some(Arc::new(move |entry: &MutationCacheEntry| {
                error_seen
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(format!("error:{}", entry.id));
            })),
            ..MutationCacheConfig::default()
        });

        let id = cache.add(keyed_entry("posts"));
        cache.fire_on_mutate(id);
        cache.update(id, |entry| {
            entry.status = MutationStatus::Error;
            entry.error = Some(QueryError::custom("boom"));
        });
        cache.fire_on_error(id);

        assert_eq!(
            *seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["mutate:1".to_string(), "error:1".to_string()]
        );
    }
}
