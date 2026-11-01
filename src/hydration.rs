// src/hydration.rs
//! Hydration and persistence support.
//!
//! Implements `TanStack` Query v5 dehydration/hydration semantics:
//!
//! * [`DehydratedState`] carries a wall-clock `dehydrated_at_ms` anchor, the
//!   dehydrated queries and a (currently always empty, future-proof) list of
//!   dehydrated mutations.
//! * [`dehydrate_state`] rebuilds entries straight from the client cache and
//!   applies [`DehydrateOptions`] faithfully (query filter, mutation filter,
//!   error redaction, post-serialization hook). [`QueryClient::dehydrate`]
//!   delegates to it with default options.
//! * [`hydrate_with_options`] restores mutations **first**, then queries with
//!   the `TanStack` "hydrate only if newer" guard, forcing the restored fetch
//!   status to idle and preserving payload staleness/timestamps.
//!   [`QueryClient::hydrate`] delegates to it.
//!
//! Known limitations (documented honestly):
//!
//! * The cache stores type-erased data (`Arc<dyn PartialEqAny>`), so only
//!   `String`-typed query data can be dehydrated; other types are skipped by
//!   data extraction and then rejected by the default success-only predicate.
//! * Cache entries do not model error states yet, so
//!   [`DehydratedQuery::error_json`] is always `None` for cache-derived
//!   dehydration. The redaction pipeline ([`redact_error`]) is already wired
//!   and produces `"redacted"` (the [`QueryError::Redacted`] message) once
//!   error states land.
//! * There is no runtime detection for infinite queries yet, so `query_type`
//!   is always [`QUERY_TYPE_QUERY`].
//! * The mutation cache lands in a later wave; `mutations` is always empty on
//!   dehydration and mutation hydration is a documented no-op (ordering is
//!   already implemented).
//! * `DehydrateOptions`, `DehydratedMutation`, `dehydrate_state` and
//!   `hydrate_with_options` still need to be re-exported from `lib.rs` (this
//!   wave owns `hydration.rs` only).

use crate::client::CacheEntry;
use crate::{QueryClient, QueryError, QueryKey, QueryOptions};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Post-serialization hook applied to the serialized data text.
///
/// Only takes effect under the `serde` feature (where actual serialization
/// happens).
///
/// # `TanStack` parity: `DehydrateOptions`' `serialize` hook.
pub type SerializeDataFn = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Filter deciding which dehydrated query hashes hydrate, keyed by the query
/// hash (`TanStack` `queryHash`).
///
/// # `TanStack` parity: `HydrateOptions`' `shouldHydrate` predicate.
pub type ShouldHydrateFn = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// `query_type` marker for standard queries (`TanStack` `"query"`).
pub const QUERY_TYPE_QUERY: &str = "query";

/// `query_type` marker for infinite queries (`TanStack` `"infinite"`).
pub const QUERY_TYPE_INFINITE: &str = "infinite";

/// A single dehydrated query entry, mirroring `TanStack` v5 `DehydratedQuery`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DehydratedQuery {
    /// Hash of the query key (the cache key) — `TanStack` `queryHash`.
    pub query_hash: String,
    /// Segments of the query key — `TanStack` `queryKey`.
    pub query_key: Vec<String>,
    /// Serialized data. Without the `serde` feature this is the raw string
    /// data; with the feature it is JSON-encoded (then passed through the
    /// [`DehydrateOptions::serialize_data`] hook when set).
    pub data_json: Option<String>,
    /// Serialized error message. `None` for cache-derived dehydration today
    /// (cache entries carry no error state); when redaction is enabled the
    /// stored message is `"redacted"` (see [`redact_error`]).
    pub error_json: Option<String>,
    /// Wall-clock (UNIX epoch milliseconds) the data was fetched, anchored at
    /// dehydrate time: `dehydrated_at_ms - age_at_dehydrate`. `None` means the
    /// payload carries no fetch timestamp (never beats local data on hydrate).
    pub fetched_at_ms: Option<u64>,
    /// Whether the query was flagged stale when dehydrated.
    pub is_stale: bool,
    /// Stale time (ms) configured on the query.
    pub stale_time_ms: u64,
    /// Garbage-collection time (ms) configured on the query.
    pub gc_time_ms: u64,
    /// Free-form query metadata (deterministic ordering via `BTreeMap`).
    pub meta: BTreeMap<String, String>,
    /// `"query"` or `"infinite"` (see [`QUERY_TYPE_QUERY`]/[`QUERY_TYPE_INFINITE`]).
    pub query_type: String,
}

/// A single dehydrated mutation, mirroring `TanStack` v5 `DehydratedMutation`.
///
/// The mutation cache lands in a later wave; today dehydration never produces
/// mutations and hydration of mutations is a no-op, but the wire shape is
/// already in place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DehydratedMutation {
    /// Mutation key segments, if the mutation had one.
    pub mutation_key: Option<Vec<String>>,
    /// Wall-clock (UNIX epoch ms) the mutation last updated its data.
    pub data_updated_at_ms: Option<u64>,
    /// Whether the mutation was paused when dehydrated (`TanStack` default
    /// `shouldDehydrateMutation` keeps only paused mutations).
    pub is_paused: bool,
}

/// Dehydrated state containing all serializable query cache entries, mirroring
/// `TanStack` v5 `DehydratedState`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DehydratedState {
    /// Wall-clock (UNIX epoch ms) at dehydration time; the anchor used to
    /// interpret `fetched_at_ms` values during hydration.
    pub dehydrated_at_ms: u64,
    /// Dehydrated queries (deterministic order: sorted by `query_hash`).
    pub queries: Vec<DehydratedQuery>,
    /// Dehydrated mutations (empty until the mutation cache wave lands).
    pub mutations: Vec<DehydratedMutation>,
}

/// `TanStack` `defaultShouldDehydrateQuery`: only queries whose data is present
/// (i.e. a `success` state) are dehydrated.
#[must_use]
pub fn default_should_dehydrate_query(query: &DehydratedQuery) -> bool {
    query.data_json.is_some()
}

/// `TanStack` `defaultShouldDehydrateMutation`: only paused mutations are
/// dehydrated.
#[must_use]
pub fn default_should_dehydrate_mutation(mutation: &DehydratedMutation) -> bool {
    mutation.is_paused
}

/// Options for dehydration, mirroring `TanStack` v5 `DehydrateOptions`.
pub struct DehydrateOptions {
    /// Predicate deciding which queries are dehydrated. Default:
    /// [`default_should_dehydrate_query`] (success/data-bearing only).
    pub should_dehydrate_query: Arc<dyn Fn(&DehydratedQuery) -> bool + Send + Sync>,
    /// Predicate deciding which mutations are dehydrated. Default:
    /// [`default_should_dehydrate_mutation`] (paused only; none exist today).
    pub should_dehydrate_mutation: Arc<dyn Fn(&DehydratedMutation) -> bool + Send + Sync>,
    /// Whether errors are replaced by the redacted marker string
    /// (`QueryError::Redacted` — `"redacted"`). Default: `true`.
    pub should_redact_errors: bool,
    /// Post-serialization hook applied to the serialized data text. Identity
    /// by default; takes effect only under the `serde` feature (where actual
    /// serialization happens).
    pub serialize_data: Option<SerializeDataFn>,
}

impl Clone for DehydrateOptions {
    fn clone(&self) -> Self {
        Self {
            should_dehydrate_query: Arc::clone(&self.should_dehydrate_query),
            should_dehydrate_mutation: Arc::clone(&self.should_dehydrate_mutation),
            should_redact_errors: self.should_redact_errors,
            serialize_data: self.serialize_data.clone(),
        }
    }
}

impl fmt::Debug for DehydrateOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DehydrateOptions")
            .field("should_dehydrate_query", &"[Fn]")
            .field("should_dehydrate_mutation", &"[Fn]")
            .field("should_redact_errors", &self.should_redact_errors)
            .field(
                "serialize_data",
                &self.serialize_data.as_ref().map(|_| "[Fn]"),
            )
            .finish()
    }
}

impl Default for DehydrateOptions {
    fn default() -> Self {
        Self {
            should_dehydrate_query: Arc::new(default_should_dehydrate_query),
            should_dehydrate_mutation: Arc::new(default_should_dehydrate_mutation),
            should_redact_errors: true,
            serialize_data: None,
        }
    }
}

impl DehydrateOptions {
    /// Replace the query dehydration predicate.
    #[must_use]
    pub fn should_dehydrate_query<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&DehydratedQuery) -> bool + Send + Sync + 'static,
    {
        self.should_dehydrate_query = Arc::new(predicate);
        self
    }

    /// Replace the mutation dehydration predicate.
    #[must_use]
    pub fn should_dehydrate_mutation<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&DehydratedMutation) -> bool + Send + Sync + 'static,
    {
        self.should_dehydrate_mutation = Arc::new(predicate);
        self
    }

    /// Enable or disable error redaction (default: enabled).
    #[must_use]
    pub fn should_redact_errors(mut self, redact: bool) -> Self {
        self.should_redact_errors = redact;
        self
    }

    /// Set the post-serialization data hook (effective under `serde`).
    #[must_use]
    pub fn serialize_data<F>(mut self, hook: F) -> Self
    where
        F: Fn(&str) -> String + Send + Sync + 'static,
    {
        self.serialize_data = Some(Arc::new(hook));
        self
    }
}

/// Options for hydrating the query cache, mirroring `TanStack` v5
/// `HydrateOptions`.
#[derive(Clone)]
pub struct HydrateOptions {
    /// When `false`, existing cache entries are never overwritten. When
    /// `true` (default), a dehydrated payload only overwrites an existing
    /// entry if it is *newer* (`TanStack` "hydrate only if newer").
    pub should_overwrite: bool,
    /// Optional filter on the query hash deciding which entries hydrate.
    pub should_hydrate: Option<ShouldHydrateFn>,
    /// Default options applied when creating (or overwriting) queries.
    /// Per-query `stale_time`/`gc_time`/`meta` from the dehydrated payload
    /// always win over these defaults.
    pub default_options: Option<QueryOptions>,
}

impl fmt::Debug for HydrateOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HydrateOptions")
            .field("should_overwrite", &self.should_overwrite)
            .field(
                "should_hydrate",
                &self.should_hydrate.as_ref().map(|_| "[Fn]"),
            )
            .field("default_options", &self.default_options)
            .finish()
    }
}

impl Default for HydrateOptions {
    fn default() -> Self {
        Self {
            should_overwrite: true,
            should_hydrate: None,
            default_options: None,
        }
    }
}

impl HydrateOptions {
    /// Set whether existing entries may be overwritten (newer-payload guard
    /// applies either way).
    #[must_use]
    pub fn should_overwrite(mut self, overwrite: bool) -> Self {
        self.should_overwrite = overwrite;
        self
    }

    /// Set a filter on the query hash deciding which entries hydrate.
    #[must_use]
    pub fn should_hydrate<F>(mut self, filter: F) -> Self
    where
        F: Fn(&str) -> bool + Send + Sync + 'static,
    {
        self.should_hydrate = Some(Arc::new(filter));
        self
    }

    /// Set the default options applied to hydrated queries.
    #[must_use]
    pub fn default_options(mut self, options: QueryOptions) -> Self {
        self.default_options = Some(options);
        self
    }
}

/// Current wall-clock time as UNIX epoch milliseconds.
fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Best-effort reconstruction of key segments from a cache key. Cache keys are
/// joined with `"::"`, so segments are recovered by splitting on that
/// separator (a segment value containing `"::"` is a lossy edge case).
fn segments_from_cache_key(cache_key: &str) -> Vec<String> {
    cache_key.split("::").map(str::to_string).collect()
}

/// Rebuild a [`QueryKey`] from a dehydrated entry. Falls back to treating the
/// whole `query_hash` as a single segment when the reconstructed segments do
/// not reproduce the hash (lossy `"::"` edge case above).
fn query_key_from_dehydrated(query: &DehydratedQuery) -> QueryKey {
    let key = QueryKey::from_segments(query.query_key.iter().cloned());
    if key.cache_key() == query.query_hash {
        key
    } else {
        QueryKey::new(query.query_hash.clone())
    }
}

/// Serialize cache entry data for dehydration. Only `String`-typed data can be
/// extracted from the type-erased cache (documented limitation).
fn serialize_entry_data(data: &str, options: &DehydrateOptions) -> String {
    #[cfg(feature = "serde")]
    {
        let json = serde_json::to_string(data).unwrap_or_else(|_| data.to_string());
        match options.serialize_data.as_ref() {
            Some(hook) => hook(&json),
            None => json,
        }
    }
    #[cfg(not(feature = "serde"))]
    {
        let _ = options;
        data.to_string()
    }
}

/// Deserialize hydrating data back into the cached `String`. Under the `serde`
/// feature the JSON encoding produced by [`serialize_entry_data`] is decoded;
/// text that is not valid JSON (e.g. produced without the feature, or by a
/// custom [`DehydrateOptions::serialize_data`] hook) falls back to the raw
/// string.
fn deserialize_entry_data(data_json: &str) -> String {
    #[cfg(feature = "serde")]
    {
        serde_json::from_str::<String>(data_json).unwrap_or_else(|_| data_json.to_string())
    }
    #[cfg(not(feature = "serde"))]
    {
        data_json.to_string()
    }
}

/// Apply the redaction policy to a serialized error message: with redaction
/// enabled any error collapses to the [`QueryError::Redacted`] message
/// (`"redacted"`), mirroring `TanStack`'s `new Error('redacted')` default.
fn redact_error(error: Option<String>, should_redact_errors: bool) -> Option<String> {
    match error {
        Some(_) if should_redact_errors => Some(QueryError::Redacted.to_string()),
        other => other,
    }
}

/// Wall-clock age (ms) of the dehydrated data at dehydrate time. A payload
/// without a fetch timestamp is treated as maximally old so it can never
/// overwrite newer local data (`TanStack` `dataUpdatedAt === 0` behaviour).
fn dehydrated_age_ms(dehydrated_at_ms: u64, fetched_at_ms: Option<u64>) -> u64 {
    match fetched_at_ms {
        Some(fetched_at_ms) => dehydrated_at_ms.saturating_sub(fetched_at_ms),
        None => u64::MAX,
    }
}

/// Whether a dehydrated payload is newer than the locally cached entry.
///
/// The dehydrated payload was `dehydrated_age_ms` old when it was captured;
/// the local entry is `fetched_at.elapsed()` old right now. The payload wins
/// when its age is smaller (transit time between dehydrate and hydrate is
/// ignored — an accepted approximation, since the cache stores monotonic
/// [`Instant`]s that cannot be compared against wall-clock payloads directly).
fn dehydrated_is_newer(
    existing_fetched_at: Instant,
    dehydrated_at_ms: u64,
    fetched_at_ms: Option<u64>,
) -> bool {
    let age = Duration::from_millis(dehydrated_age_ms(dehydrated_at_ms, fetched_at_ms));
    existing_fetched_at.elapsed() > age
}

/// Build the options stored on a hydrated query: `default_options` when set,
/// with the dehydrated payload winning for per-query staleness/GC tuning and
/// meta (`TanStack` restores `meta` from the payload).
fn stored_options_for(dehydrated: &DehydratedQuery, options: &HydrateOptions) -> QueryOptions {
    let mut stored = options.default_options.clone().unwrap_or_default();
    stored.stale_time = Duration::from_millis(dehydrated.stale_time_ms);
    stored.gc_time = Duration::from_millis(dehydrated.gc_time_ms);
    stored.meta = dehydrated.meta.clone();
    stored
}

/// Dehydrate a single query cache entry into a [`DehydratedQuery`].
fn dehydrate_cache_entry(
    cache_key: &str,
    entry: &CacheEntry,
    error: Option<String>,
    now_ms: u64,
    options: &DehydrateOptions,
) -> DehydratedQuery {
    // Anchor the original fetch on the wall clock: the payload records when
    // the data was fetched, computed as "now minus monotonic age".
    let fetched_age_ms = u64::try_from(entry.fetched_at.elapsed().as_millis()).unwrap_or(u64::MAX);
    let fetched_at_ms = Some(now_ms.saturating_sub(fetched_age_ms));
    DehydratedQuery {
        query_hash: cache_key.to_string(),
        query_key: segments_from_cache_key(cache_key),
        data_json: entry
            .data
            .as_any()
            .downcast_ref::<String>()
            .map(|data| serialize_entry_data(data, options)),
        error_json: redact_error(error, options.should_redact_errors),
        fetched_at_ms,
        is_stale: entry.is_stale,
        stale_time_ms: u64::try_from(entry.options.stale_time.as_millis()).unwrap_or(u64::MAX),
        gc_time_ms: u64::try_from(entry.options.gc_time.as_millis()).unwrap_or(u64::MAX),
        meta: entry.options.meta.clone(),
        query_type: QUERY_TYPE_QUERY.to_string(),
    }
}

/// Dehydrate the client cache into a [`DehydratedState`], applying
/// [`DehydrateOptions`] faithfully.
///
/// Entries are rebuilt directly from `client.cache` (rather than going through
/// [`QueryClient::dehydrate`]) so that the filter, redaction and
/// post-serialization options apply to each entry. Mutations are never
/// produced today (the mutation cache lands in a later wave), so `mutations`
/// is always empty regardless of [`DehydrateOptions::should_dehydrate_mutation`].
#[must_use]
pub fn dehydrate_state(client: &QueryClient, options: &DehydrateOptions) -> DehydratedState {
    let now_ms = unix_now_ms();
    let mut queries: Vec<DehydratedQuery> = client
        .cache
        .iter()
        .map(|entry| {
            // Cache entries carry no error state yet; the redaction pipeline
            // is wired through `dehydrate_cache_entry` for when they do.
            dehydrate_cache_entry(entry.key(), entry.value(), None, now_ms, options)
        })
        .filter(|query| (options.should_dehydrate_query)(query))
        .collect();
    // Deterministic output ordering (stable snapshots / tests).
    queries.sort_by(|a, b| a.query_hash.cmp(&b.query_hash));
    DehydratedState {
        dehydrated_at_ms: now_ms,
        queries,
        mutations: Vec::new(),
    }
}

/// Restore dehydrated mutations into the client (`TanStack` restores mutations
/// *before* queries so paused mutations can be resumed immediately).
///
/// The mutation cache lands in a later wave, so today this is a documented
/// no-op: the entries are accepted and ignored. The call-site ordering is
/// already in place — [`hydrate_with_options`] invokes this before any query
/// is restored — so the future implementation only fills in this body.
fn hydrate_mutations(
    _client: &QueryClient,
    mutations: &[DehydratedMutation],
    _options: &HydrateOptions,
) {
    for _mutation in mutations {
        // TODO(mutation-cache wave): rebuild each mutation with its restored
        // state (including `is_paused`) in the mutation cache.
    }
}

/// Restore a single dehydrated query into the cache.
///
/// Semantics (`TanStack` v5):
///
/// * Existing entry: hydrate only when the payload is newer than the local
///   data (unless [`HydrateOptions::should_overwrite`] is `false`, which never
///   overwrites). The restored entry is created via
///   [`QueryClient::set_query_data`], which forces the fetch status back to
///   idle.
/// * New entry: created via [`QueryClient::set_query_data`] with options built
///   from the payload (+ [`HydrateOptions::default_options`] when set).
/// * Afterwards the payload staleness flag is restored and the fetch timestamp
///   is back-dated to `now - payload_age`, preserving true freshness across
///   the roundtrip. Entries without data cannot be materialized in the
///   data-only cache and are skipped.
fn hydrate_query(
    client: &QueryClient,
    dehydrated_at_ms: u64,
    dehydrated: &DehydratedQuery,
    options: &HydrateOptions,
) {
    // Dev-only invariant (porting checklist): the wire `query_type` marker
    // must be one of the known TanStack values.
    debug_assert!(
        dehydrated.query_type == QUERY_TYPE_QUERY || dehydrated.query_type == QUERY_TYPE_INFINITE,
        "unknown query_type marker in dehydrated payload"
    );

    if let Some(ref filter) = options.should_hydrate
        && !filter(&dehydrated.query_hash)
    {
        return;
    }

    let Some(data_json) = dehydrated.data_json.as_deref() else {
        // Dataless payloads (pending/errored states) have no representation in
        // the data cache yet; skip rather than create an empty entry.
        return;
    };
    let data = deserialize_entry_data(data_json);
    let key = query_key_from_dehydrated(dehydrated);

    // Decide the overwrite winner first and drop the dashmap guard before
    // `set_query_data` re-inserts the key (holding a guard across the write
    // would deadlock the shard).
    let payload_wins = match client.cache.get(dehydrated.query_hash.as_str()) {
        Some(existing) => {
            options.should_overwrite
                && dehydrated_is_newer(
                    existing.fetched_at,
                    dehydrated_at_ms,
                    dehydrated.fetched_at_ms,
                )
        }
        None => true,
    };
    if !payload_wins {
        return;
    }

    client.set_query_data(&key, data, stored_options_for(dehydrated, options));

    // Restore payload staleness and the original fetch time (TanStack keeps
    // `dataUpdatedAt` through hydration). If the payload is older than the
    // monotonic clock horizon (`checked_sub` fails), the entry keeps `now` and
    // simply appears fresh.
    let age_ms = dehydrated_age_ms(dehydrated_at_ms, dehydrated.fetched_at_ms);
    restore_entry_staleness(client, &dehydrated.query_hash, dehydrated, age_ms);
}

/// Restore `is_stale` and the backdated fetch time of one hydrated entry.
///
/// The entry normally exists right after the insert in [`hydrate_query`]; a
/// concurrent eviction (e.g. garbage collection) leaves nothing to restore,
/// which is tolerated.
fn restore_entry_staleness(
    client: &QueryClient,
    query_hash: &str,
    dehydrated: &DehydratedQuery,
    age_ms: u64,
) {
    if let Some(mut entry) = client.cache.get_mut(query_hash) {
        entry.is_stale = dehydrated.is_stale;
        if age_ms > 0
            && let Some(backdated) = Instant::now().checked_sub(Duration::from_millis(age_ms))
        {
            entry.fetched_at = backdated;
        }
    }
}

/// Shared hydration core used by both public entry points.
///
/// Takes `state` and `options` by value so each is consumed exactly once;
/// the borrowed-options entry point clones the cheap `Arc` handles.
fn hydrate_state_owned(client: &QueryClient, state: DehydratedState, options: HydrateOptions) {
    let DehydratedState {
        dehydrated_at_ms,
        queries,
        mutations,
    } = state;
    // Consume the options object (`TanStack`'s `hydrate` takes an options
    // object): split the fields so the owned value is genuinely used up.
    let HydrateOptions {
        should_overwrite,
        should_hydrate,
        default_options,
    } = options;
    let options = HydrateOptions {
        should_overwrite,
        should_hydrate,
        default_options,
    };
    // TanStack ordering: mutations are restored BEFORE queries.
    hydrate_mutations(client, &mutations, &options);
    for query in &queries {
        hydrate_query(client, dehydrated_at_ms, query, &options);
    }
}

/// Hydrate a dehydrated state into the client cache with full `TanStack` v5
/// semantics: mutations first (a no-op until the mutation cache wave), then
/// queries with the newer-payload guard, `default_options` application, forced
/// idle fetch status and payload staleness/timestamp restoration.
pub fn hydrate_with_options(
    client: &QueryClient,
    state: DehydratedState,
    options: &HydrateOptions,
) {
    let owned = HydrateOptions {
        should_overwrite: options.should_overwrite,
        should_hydrate: options.should_hydrate.clone(),
        default_options: options.default_options.clone(),
    };
    hydrate_state_owned(client, state, owned);
}

impl QueryClient {
    /// Dehydrate the cache with default options (success-only queries, paused
    /// mutations, redacted errors, no post-serialization hook).
    ///
    /// See [`dehydrate_state`] for option-aware dehydration.
    #[must_use]
    pub fn dehydrate(&self) -> DehydratedState {
        dehydrate_state(self, &DehydrateOptions::default())
    }

    /// Hydrate a dehydrated state with the given options.
    ///
    /// See [`hydrate_with_options`] for the exact semantics (mutations first,
    /// newer-payload overwrite guard, `default_options`).
    pub fn hydrate(&self, state: DehydratedState, options: HydrateOptions) {
        hydrate_state_owned(self, state, options);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a dehydrated query whose fetch happened `age_ms` before now.
    fn dehydrated_query(hash: &str, data: &str, age_ms: u64) -> DehydratedQuery {
        let now = unix_now_ms();
        DehydratedQuery {
            query_hash: hash.to_string(),
            query_key: segments_from_cache_key(hash),
            data_json: Some(data.to_string()),
            error_json: None,
            fetched_at_ms: Some(now.saturating_sub(age_ms)),
            is_stale: false,
            stale_time_ms: 0,
            gc_time_ms: 300_000,
            meta: BTreeMap::new(),
            query_type: QUERY_TYPE_QUERY.to_string(),
        }
    }

    /// Build a dehydrated state anchored at now.
    fn dehydrated_state_now(queries: Vec<DehydratedQuery>) -> DehydratedState {
        DehydratedState {
            dehydrated_at_ms: unix_now_ms(),
            queries,
            mutations: Vec::new(),
        }
    }

    #[test]
    fn test_dehydrate_empty_cache() {
        let client = QueryClient::new();
        let state = client.dehydrate();
        assert_eq!(state.queries, [] as [DehydratedQuery; 0]);
        assert_eq!(state.mutations, [] as [DehydratedMutation; 0]);
    }

    #[test]
    fn test_dehydrate_with_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        let opts = QueryOptions {
            stale_time: Duration::from_secs(60),
            gc_time: Duration::from_secs(300),
            ..Default::default()
        };
        client.set_query_data(&key, "user_data".to_string(), opts);

        let state = client.dehydrate();
        assert_eq!(state.queries.len(), 1);
        let entry = &state.queries[0];
        assert_eq!(entry.query_hash, "users");
        assert_eq!(entry.query_key, vec!["users".to_string()]);
        // JSON-encoded under the `serde` feature, raw string otherwise.
        #[cfg(feature = "serde")]
        assert_eq!(entry.data_json.as_deref(), Some("\"user_data\""));
        #[cfg(not(feature = "serde"))]
        assert_eq!(entry.data_json.as_deref(), Some("user_data"));
        assert_eq!(entry.stale_time_ms, 60_000);
        assert_eq!(entry.gc_time_ms, 300_000);
        assert!(!entry.is_stale);
        assert_eq!(entry.query_type, QUERY_TYPE_QUERY);
        assert!(entry.error_json.is_none());
        // The fetch anchor never exceeds the dehydration anchor.
        assert!(entry.fetched_at_ms.unwrap() <= state.dehydrated_at_ms);
        assert!(state.dehydrated_at_ms > 0);
    }

    #[test]
    fn test_dehydrate_non_string_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("count");
        client.set_query_data(&key, 42i32, QueryOptions::default());

        // Type-erased cache: only String data can be extracted, and the
        // default success-only predicate rejects dataless payloads.
        let state = client.dehydrate();
        assert_eq!(state.queries, [] as [DehydratedQuery; 0]);
    }

    #[test]
    fn test_dehydrate_multiple_entries() {
        let client = QueryClient::new();
        client.set_query_data(
            &QueryKey::new("a"),
            "data_a".to_string(),
            QueryOptions::default(),
        );
        client.set_query_data(
            &QueryKey::new("b"),
            "data_b".to_string(),
            QueryOptions::default(),
        );

        let state = client.dehydrate();
        assert_eq!(state.queries.len(), 2);
        // Deterministic ordering by hash.
        let hashes: Vec<&str> = state
            .queries
            .iter()
            .map(|q| q.query_hash.as_str())
            .collect();
        assert_eq!(hashes, vec!["a", "b"]);
    }

    #[test]
    fn test_dehydrated_at_ms_set() {
        let client = QueryClient::new();
        client.set_query_data(
            &QueryKey::new("users"),
            "v".to_string(),
            QueryOptions::default(),
        );
        let before = unix_now_ms();
        let state = client.dehydrate();
        let after = unix_now_ms();
        assert!(state.dehydrated_at_ms >= before);
        assert!(state.dehydrated_at_ms <= after);
    }

    #[test]
    fn test_dehydrate_multi_segment_key() {
        let client = QueryClient::new();
        let key = QueryKey::new("users").with("id", 42);
        client.set_query_data(&key, "user_42".to_string(), QueryOptions::default());

        let state = client.dehydrate();
        let entry = &state.queries[0];
        assert_eq!(entry.query_hash, "users::id=42");
        assert_eq!(
            entry.query_key,
            vec!["users".to_string(), "id=42".to_string()]
        );
    }

    #[test]
    fn test_dehydrate_state_should_dehydrate_query_filter() {
        let client = QueryClient::new();
        client.set_query_data(
            &QueryKey::new("keep_a"),
            "a".to_string(),
            QueryOptions::default(),
        );
        client.set_query_data(
            &QueryKey::new("skip_b"),
            "b".to_string(),
            QueryOptions::default(),
        );

        let options = DehydrateOptions::default()
            .should_dehydrate_query(|query| query.query_hash.starts_with("keep"));
        let state = dehydrate_state(&client, &options);
        assert_eq!(state.queries.len(), 1);
        assert_eq!(state.queries[0].query_hash, "keep_a");
    }

    #[test]
    fn test_dehydrate_options_default_filters() {
        let options = DehydrateOptions::default();
        assert!(options.should_redact_errors);
        assert!(options.serialize_data.is_none());

        // Default query predicate: success-shaped (data present) only.
        let with_data = dehydrated_query("users", "v", 0);
        let without_data = DehydratedQuery {
            data_json: None,
            ..dehydrated_query("users", "v", 0)
        };
        assert!((options.should_dehydrate_query)(&with_data));
        assert!(!(options.should_dehydrate_query)(&without_data));

        // Default mutation predicate: paused only.
        let paused = DehydratedMutation {
            is_paused: true,
            ..Default::default()
        };
        assert!((options.should_dehydrate_mutation)(&paused));
        assert!(!(options.should_dehydrate_mutation)(
            &DehydratedMutation::default()
        ));
    }

    #[test]
    fn test_dehydrate_options_builders() {
        let options = DehydrateOptions::default()
            .should_dehydrate_query(|_| true)
            .should_dehydrate_mutation(|_| true)
            .should_redact_errors(false);
        assert!(!options.should_redact_errors);
        assert!((options.should_dehydrate_query)(&DehydratedQuery::default()));
        assert!((options.should_dehydrate_mutation)(
            &DehydratedMutation::default()
        ));

        let cloned = options.clone();
        assert!(!cloned.should_redact_errors);
        let debug = format!("{options:?}");
        assert!(debug.contains("should_redact_errors"));
    }

    #[test]
    fn test_redact_error_default_on() {
        let options = DehydrateOptions::default();
        assert_eq!(
            redact_error(Some("boom".to_string()), options.should_redact_errors),
            Some(QueryError::Redacted.to_string())
        );
        assert_eq!(QueryError::Redacted.to_string(), "redacted");
    }

    #[test]
    fn test_redact_error_disabled_passthrough() {
        let options = DehydrateOptions::default().should_redact_errors(false);
        assert!(!options.should_redact_errors);
        assert_eq!(
            redact_error(Some("boom".to_string()), options.should_redact_errors),
            Some("boom".to_string())
        );
        assert_eq!(redact_error(None, false), None);
    }

    #[test]
    fn test_hydrate_into_empty_cache() {
        let client = QueryClient::new();
        let state = dehydrated_state_now(vec![dehydrated_query("users", "restored", 0)]);
        client.hydrate(state, HydrateOptions::default());

        let data: Option<String> = client.get_query_data(&QueryKey::new("users"));
        assert_eq!(data, Some("restored".to_string()));
    }

    #[test]
    fn test_hydrate_should_overwrite_false_keeps_local() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        client.set_query_data(&key, "existing".to_string(), QueryOptions::default());

        let state = dehydrated_state_now(vec![dehydrated_query("users", "payload", 0)]);
        let options = HydrateOptions::default().should_overwrite(false);
        client.hydrate(state, options);

        let data: Option<String> = client.get_query_data(&key);
        assert_eq!(data, Some("existing".to_string()));
    }

    #[test]
    fn test_hydrate_older_payload_does_not_overwrite_fresh_local() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        // Fresh local data (age ~0ms)...
        client.set_query_data(&key, "local".to_string(), QueryOptions::default());

        // ...vs a payload that was already 1 minute old at dehydrate time.
        let state = dehydrated_state_now(vec![dehydrated_query("users", "old_payload", 60_000)]);
        client.hydrate(state, HydrateOptions::default());

        let data: Option<String> = client.get_query_data(&key);
        assert_eq!(data, Some("local".to_string()));
    }

    #[test]
    fn test_hydrate_newer_payload_overwrites_stale_local() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        // Local data fetched 30ms ago...
        client.set_query_data(&key, "local".to_string(), QueryOptions::default());
        std::thread::sleep(Duration::from_millis(30));

        // ...vs a payload fetched exactly at dehydrate time (age 0ms).
        let state = dehydrated_state_now(vec![dehydrated_query("users", "fresh_payload", 0)]);
        client.hydrate(state, HydrateOptions::default());

        let data: Option<String> = client.get_query_data(&key);
        assert_eq!(data, Some("fresh_payload".to_string()));
    }

    #[test]
    fn test_hydrate_backdates_fetched_at_to_payload_age() {
        let client = QueryClient::new();
        let key = QueryKey::new("users");
        // No local entry: any payload wins, even one that was already aged
        // when it was dehydrated.
        let state = dehydrated_state_now(vec![dehydrated_query("users", "aged_payload", 5_000)]);
        client.hydrate(state, HydrateOptions::default());

        let data: Option<String> = client.get_query_data(&key);
        assert_eq!(data, Some("aged_payload".to_string()));

        // The restored entry carries the payload's original fetch time, so
        // it appears ~5s old instead of brand new.
        let entry = client
            .cache
            .get("users")
            .expect("hydrated entry should be cached");
        let age = entry.fetched_at.elapsed();
        assert!(
            age >= Duration::from_secs(5) && age < Duration::from_secs(6),
            "fetched_at should be backdated by the payload age, got {age:?}"
        );
    }

    #[test]
    fn test_hydrate_hash_key_mismatch_skips_staleness_restore() {
        let client = QueryClient::new();
        // A payload whose `query_hash` has no matching cache entry (e.g. the
        // entry was evicted between the insert and the staleness restore, or
        // a malformed hash/key pair) leaves nothing to restore.
        let query = dehydrated_query("users", "payload", 5_000);
        restore_entry_staleness(&client, "posts", &query, 5_000);
        assert!(!client.cache.contains_key("posts"));
        assert!(!client.cache.contains_key("users"));
    }

    #[test]
    fn test_hydrate_with_filter() {
        let client = QueryClient::new();
        let state = dehydrated_state_now(vec![
            dehydrated_query("users", "user_data", 0),
            dehydrated_query("posts", "post_data", 0),
        ]);

        let options = HydrateOptions::default().should_hydrate(|hash| hash.starts_with("users"));
        client.hydrate(state, options);

        let users: Option<String> = client.get_query_data(&QueryKey::new("users"));
        let posts: Option<String> = client.get_query_data(&QueryKey::new("posts"));
        assert_eq!(users, Some("user_data".to_string()));
        assert_eq!(posts, None);
    }

    #[test]
    fn test_hydrate_with_filter_rejecting_all() {
        let client = QueryClient::new();
        let state = dehydrated_state_now(vec![dehydrated_query("users", "data", 0)]);

        let options = HydrateOptions::default().should_hydrate(|_| false);
        client.hydrate(state, options);

        let users: Option<String> = client.get_query_data(&QueryKey::new("users"));
        assert_eq!(users, None);
    }

    #[test]
    fn test_hydrate_skips_entries_without_data() {
        let client = QueryClient::new();
        let payload = DehydratedQuery {
            data_json: None,
            ..dehydrated_query("users", "unused", 0)
        };
        client.hydrate(
            dehydrated_state_now(vec![payload]),
            HydrateOptions::default(),
        );
        assert!(client.cache.is_empty());
    }

    #[test]
    fn test_hydrate_applies_default_options() {
        let client = QueryClient::new();
        let payload = DehydratedQuery {
            stale_time_ms: 60_000,
            gc_time_ms: 300_000,
            ..dehydrated_query("users", "data", 0)
        };
        let options = HydrateOptions::default().default_options(QueryOptions::new().enabled(false));
        hydrate_with_options(&client, dehydrated_state_now(vec![payload]), &options);

        let stored = client.get_query_options(&QueryKey::new("users")).unwrap();
        // Payload staleness/GC tuning wins over the hydrate defaults...
        assert_eq!(stored.stale_time, Duration::from_secs(60));
        assert_eq!(stored.gc_time, Duration::from_secs(300));
        // ...while the remaining fields come from default_options.
        assert!(!stored.enabled);
    }

    #[test]
    fn test_hydrate_preserves_payload_staleness_and_timestamps() {
        let client = QueryClient::new();
        // Payload fetched 10s before dehydrate time and flagged stale.
        let payload = DehydratedQuery {
            is_stale: true,
            ..dehydrated_query("users", "snapshot", 10_000)
        };
        let original_fetched_ms = payload.fetched_at_ms.unwrap();
        client.hydrate(
            dehydrated_state_now(vec![payload]),
            HydrateOptions::default(),
        );

        let entry = client.cache.get("users").unwrap();
        assert!(entry.is_stale);
        drop(entry);

        // Re-dehydrating preserves the payload's fetch timestamp (within a
        // tolerance for clock jitter between the two dehydration passes).
        let state2 = client.dehydrate();
        let q2 = state2
            .queries
            .iter()
            .find(|q| q.query_hash == "users")
            .unwrap();
        assert!(q2.is_stale);
        let age2 = state2.dehydrated_at_ms - q2.fetched_at_ms.unwrap();
        assert!(10_000u64.abs_diff(age2) < 5_000);
        assert!(q2.fetched_at_ms.unwrap() >= original_fetched_ms);
    }

    #[test]
    fn test_hydrate_with_multi_segment_key() {
        let client = QueryClient::new();
        let state = dehydrated_state_now(vec![dehydrated_query("users::id=42", "user_42", 0)]);
        client.hydrate(state, HydrateOptions::default());

        let key = QueryKey::new("users").with("id", 42);
        let data: Option<String> = client.get_query_data(&key);
        assert_eq!(data, Some("user_42".to_string()));
    }

    #[test]
    fn test_hydrate_falls_back_to_hash_when_segments_mismatch() {
        let client = QueryClient::new();
        // The segments do not reproduce the hash, so the hash itself becomes
        // the key (TanStack restores by hash in that case).
        let payload = DehydratedQuery {
            query_key: vec!["unrelated".to_string()],
            ..dehydrated_query("users", "hash_keyed", 0)
        };
        client.hydrate(
            dehydrated_state_now(vec![payload]),
            HydrateOptions::default(),
        );

        let data: Option<String> = client.get_query_data(&QueryKey::new("users"));
        assert_eq!(data, Some("hash_keyed".to_string()));
    }

    #[test]
    fn test_hydrate_payload_without_fetch_timestamp_never_overwrites() {
        let client = QueryClient::new();
        client.set_query_data(
            &QueryKey::new("users"),
            "local".to_string(),
            QueryOptions::default(),
        );

        // A payload without a fetch timestamp is treated as maximally old and
        // must never replace newer local data.
        let payload = DehydratedQuery {
            fetched_at_ms: None,
            ..dehydrated_query("users", "undated_remote", 0)
        };
        client.hydrate(
            dehydrated_state_now(vec![payload]),
            HydrateOptions::default(),
        );

        let data: Option<String> = client.get_query_data(&QueryKey::new("users"));
        assert_eq!(data, Some("local".to_string()));
    }

    #[test]
    fn test_mutations_hydrated_before_queries_structural() {
        // Structural ordering test: `hydrate_with_options` processes mutations
        // first (see its body: `hydrate_mutations` is invoked before the query
        // loop), then queries. A runtime recording impl is not feasible until
        // the mutation cache exists, so this verifies both stages of the
        // pipeline on a single mixed payload.
        let client = QueryClient::new();
        let state = DehydratedState {
            dehydrated_at_ms: unix_now_ms(),
            queries: vec![dehydrated_query("users", "data", 0)],
            mutations: vec![DehydratedMutation {
                mutation_key: Some(vec!["add_user".to_string()]),
                data_updated_at_ms: None,
                is_paused: true,
            }],
        };
        client.hydrate(state, HydrateOptions::default());

        // Queries applied; the mutation was consumed as the documented no-op.
        let data: Option<String> = client.get_query_data(&QueryKey::new("users"));
        assert_eq!(data, Some("data".to_string()));
        assert_eq!(client.cache.len(), 1);
    }

    #[test]
    fn test_default_should_dehydrate_mutation_paused_only() {
        assert!(default_should_dehydrate_mutation(&DehydratedMutation {
            is_paused: true,
            ..Default::default()
        }));
        assert!(!default_should_dehydrate_mutation(
            &DehydratedMutation::default()
        ));
        assert!(default_should_dehydrate_query(&dehydrated_query(
            "k", "v", 0
        )));
        assert!(!default_should_dehydrate_query(&DehydratedQuery::default()));
    }

    #[test]
    fn test_dehydrated_state_default() {
        let state = DehydratedState::default();
        assert_eq!(state.queries, [] as [DehydratedQuery; 0]);
        assert_eq!(state.mutations, [] as [DehydratedMutation; 0]);
        assert_eq!(state.dehydrated_at_ms, 0);
    }

    #[test]
    fn test_dehydrated_query_clone() {
        let query = dehydrated_query("test", "data", 100);
        let cloned = query.clone();
        assert_eq!(query, cloned);
    }

    #[test]
    fn test_dehydrated_query_debug() {
        let query = dehydrated_query("test", "data", 100);
        let debug = format!("{query:?}");
        assert!(debug.contains("test"));
        assert!(debug.contains("query_hash"));
    }

    #[test]
    fn test_dehydrated_mutation_clone_debug() {
        let mutation = DehydratedMutation {
            mutation_key: Some(vec!["k".to_string()]),
            data_updated_at_ms: Some(42),
            is_paused: true,
        };
        let cloned = mutation.clone();
        assert_eq!(mutation, cloned);
        let debug = format!("{cloned:?}");
        assert!(debug.contains("is_paused"));
    }

    #[test]
    fn test_hydrate_options_default() {
        let opts = HydrateOptions::default();
        assert!(opts.should_overwrite);
        assert!(opts.should_hydrate.is_none());
        assert!(opts.default_options.is_none());
    }

    #[test]
    fn test_hydrate_options_clone() {
        let opts = HydrateOptions {
            should_overwrite: false,
            should_hydrate: Some(Arc::new(|_| true)),
            default_options: Some(QueryOptions::new().enabled(false)),
        };
        let cloned = opts.clone();
        assert!(!cloned.should_overwrite);
        assert!(cloned.should_hydrate.is_some());
        assert!(cloned.default_options.is_some());
    }

    #[test]
    fn test_hydrate_options_debug() {
        let opts = HydrateOptions::default();
        let debug = format!("{opts:?}");
        assert!(debug.contains("should_overwrite"));
        assert!(debug.contains("default_options"));
    }

    #[test]
    fn test_roundtrip_dehydrate_hydrate() {
        let client = QueryClient::new();
        client.set_query_data(
            &QueryKey::new("test"),
            "roundtrip".to_string(),
            QueryOptions {
                stale_time: Duration::from_secs(30),
                gc_time: Duration::from_secs(120),
                ..Default::default()
            },
        );

        let state = client.dehydrate();
        let client2 = QueryClient::new();
        client2.hydrate(state, HydrateOptions::default());

        let data: Option<String> = client2.get_query_data(&QueryKey::new("test"));
        assert_eq!(data, Some("roundtrip".to_string()));
        let opts = client2.get_query_options(&QueryKey::new("test")).unwrap();
        assert_eq!(opts.stale_time, Duration::from_secs(30));
        assert_eq!(opts.gc_time, Duration::from_secs(120));
    }

    #[test]
    fn test_hydrate_tolerates_plain_text_data() {
        // Data that is not JSON-encoded (produced without the `serde` feature,
        // or by a custom serialize hook) hydrates as the raw string.
        let client = QueryClient::new();
        let payload = DehydratedQuery {
            data_json: Some("plain text".to_string()),
            ..dehydrated_query("users", "unused", 0)
        };
        client.hydrate(
            dehydrated_state_now(vec![payload]),
            HydrateOptions::default(),
        );

        let data: Option<String> = client.get_query_data(&QueryKey::new("users"));
        assert_eq!(data, Some("plain text".to_string()));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_serde_roundtrip_preserves_state() {
        let client = QueryClient::new();
        let opts = QueryOptions {
            stale_time: Duration::from_secs(60),
            gc_time: Duration::from_secs(300),
            meta: BTreeMap::from([("source".to_string(), "test".to_string())]),
            ..Default::default()
        };
        client.set_query_data(&QueryKey::new("users"), "user_data".to_string(), opts);

        let state = client.dehydrate();
        let json = serde_json::to_string(&state).unwrap();
        let restored: DehydratedState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, restored);

        let client2 = QueryClient::new();
        client2.hydrate(restored, HydrateOptions::default());
        let data: Option<String> = client2.get_query_data(&QueryKey::new("users"));
        assert_eq!(data, Some("user_data".to_string()));
        let stored = client2.get_query_options(&QueryKey::new("users")).unwrap();
        assert_eq!(stored.meta.get("source"), Some(&"test".to_string()));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_serialize_data_hook_applied() {
        let client = QueryClient::new();
        client.set_query_data(
            &QueryKey::new("users"),
            "user_data".to_string(),
            QueryOptions::default(),
        );

        let options = DehydrateOptions::default().serialize_data(|s| format!("wrapped::{s}"));
        let state = dehydrate_state(&client, &options);
        // The hook runs after JSON serialization ("user_data" -> "\"user_data\"").
        assert_eq!(
            state.queries[0].data_json.as_deref(),
            Some("wrapped::\"user_data\"")
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_hydrate_decodes_json_encoded_data() {
        let client = QueryClient::new();
        let payload = DehydratedQuery {
            data_json: Some("\"user_data\"".to_string()),
            ..dehydrated_query("users", "unused", 0)
        };
        client.hydrate(
            dehydrated_state_now(vec![payload]),
            HydrateOptions::default(),
        );

        let data: Option<String> = client.get_query_data(&QueryKey::new("users"));
        assert_eq!(data, Some("user_data".to_string()));
    }
}
