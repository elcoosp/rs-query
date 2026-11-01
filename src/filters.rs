// src/filters.rs
//! Query and mutation filters, mirroring `TanStack` Query's `QueryFilters`,
//! `InvalidateQueryFilters`, `RefetchQueryFilters`, `MutationFilters`,
//! `matchQuery` and `matchMutation`.

use crate::key::QueryKey;
use crate::state::{FetchStatus, MutationStatus, QueryStatus};
use std::sync::Arc;

/// Which set of queries a bulk operation applies to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum QueryType {
    /// Every matching query (default).
    #[default]
    All,
    /// Only queries with at least one active (subscribed) observer.
    Active,
    /// Only queries without active observers.
    Inactive,
}

/// Refetch scope used by `invalidateQueries` and `refetchQueries`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RefetchType {
    /// Refetch every matching query.
    All,
    /// Refetch only active queries (`TanStack` default for invalidation).
    #[default]
    Active,
    /// Refetch only inactive queries.
    Inactive,
    /// Mark stale without refetching.
    None,
}

/// Immutable snapshot of a cached query used for filter matching.
#[derive(Clone, Debug)]
pub struct QuerySnapshot {
    /// The query key.
    pub key: QueryKey,
    /// Whether at least one observer is subscribed and the query is enabled.
    pub is_active: bool,
    /// Whether the cached data is stale.
    pub is_stale: bool,
    /// Aggregate fetch status.
    pub fetch_status: FetchStatus,
    /// Aggregate status.
    pub status: QueryStatus,
    /// Whether cached data exists.
    pub has_data: bool,
}

impl QuerySnapshot {
    /// Build a snapshot from raw parts.
    #[must_use]
    pub fn new(
        key: QueryKey,
        is_active: bool,
        is_stale: bool,
        fetch_status: FetchStatus,
        status: QueryStatus,
        has_data: bool,
    ) -> Self {
        Self {
            key,
            is_active,
            is_stale,
            fetch_status,
            status,
            has_data,
        }
    }
}

/// Predicate receiving a [`QuerySnapshot`] for custom matching.
pub type QueryPredicate = Arc<dyn Fn(&QuerySnapshot) -> bool + Send + Sync>;

/// Filters used to select cached queries, mirroring `QueryFilters`.
#[derive(Clone, Default)]
pub struct QueryFilters {
    /// Restrict matching to this key (prefix match unless `exact`).
    pub query_key: Option<QueryKey>,
    /// Require the key to match exactly instead of by prefix.
    pub exact: bool,
    /// Restrict to active/inactive queries.
    pub query_type: QueryType,
    /// Only match queries whose data is stale.
    pub stale: bool,
    /// Only match queries with this fetch status.
    pub fetch_status: Option<FetchStatus>,
    /// Only match queries with this status.
    pub status: Option<QueryStatus>,
    /// Custom predicate; overrides every other criterion when set.
    pub predicate: Option<QueryPredicate>,
}

impl QueryFilters {
    /// Filter matching everything.
    #[must_use]
    pub fn all() -> Self {
        Self::default()
    }

    /// Filter by key prefix (or exact key when `exact`).
    #[must_use]
    pub fn with_key(key: QueryKey) -> Self {
        Self {
            query_key: Some(key),
            ..Self::default()
        }
    }

    /// Restrict to exact key matching.
    #[must_use]
    pub fn exact(mut self) -> Self {
        self.exact = true;
        self
    }

    /// Restrict to active queries.
    #[must_use]
    pub fn active(mut self) -> Self {
        self.query_type = QueryType::Active;
        self
    }

    /// Restrict to inactive queries.
    #[must_use]
    pub fn inactive(mut self) -> Self {
        self.query_type = QueryType::Inactive;
        self
    }

    /// Require stale data.
    #[must_use]
    pub fn stale(mut self) -> Self {
        self.stale = true;
        self
    }

    /// Require a specific fetch status.
    #[must_use]
    pub fn fetch_status(mut self, fetch_status: FetchStatus) -> Self {
        self.fetch_status = Some(fetch_status);
        self
    }

    /// Require a specific status.
    #[must_use]
    pub fn status(mut self, status: QueryStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Set a custom predicate.
    #[must_use]
    pub fn predicate(
        mut self,
        predicate: impl Fn(&QuerySnapshot) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.predicate = Some(Arc::new(predicate));
        self
    }

    /// `TanStack` parity helper: filters where `stale: true`.
    #[must_use]
    pub fn is_stale_only(&self) -> bool {
        self.stale
    }
}

impl std::fmt::Debug for QueryFilters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("QueryFilters");
        debug
            .field("query_key", &self.query_key)
            .field("exact", &self.exact)
            .field("query_type", &self.query_type)
            .field("stale", &self.stale)
            .field("fetch_status", &self.fetch_status)
            .field("status", &self.status);
        if self.predicate.is_some() {
            debug.field("predicate", &"<fn>");
        }
        debug.finish()
    }
}

/// Filters for `invalidateQueries` (adds `refetch_type`), mirroring
/// `InvalidateQueryFilters`.
#[derive(Clone, Debug, Default)]
pub struct InvalidateQueryFilters {
    /// Base filters.
    pub filters: QueryFilters,
    /// Which matching queries to refetch (default: active only).
    pub refetch_type: RefetchType,
}

impl InvalidateQueryFilters {
    /// Invalidate with default (active-only) refetching.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set base filters.
    #[must_use]
    pub fn filters(mut self, filters: QueryFilters) -> Self {
        self.filters = filters;
        self
    }

    /// Set the refetch scope.
    #[must_use]
    pub fn refetch_type(mut self, refetch_type: RefetchType) -> Self {
        self.refetch_type = refetch_type;
        self
    }
}

/// Filters for `refetchQueries` (adds `cancel_refetch` and `throw_on_error`),
/// mirroring `RefetchQueryFilters`.
#[derive(Clone, Debug, Default)]
pub struct RefetchQueryFilters {
    /// Base filters.
    pub filters: QueryFilters,
    /// Cancel in-flight fetches instead of deduplicating (default `true`).
    pub cancel_refetch: bool,
    /// Propagate fetch errors to the caller (default `false`).
    pub throw_on_error: bool,
}

impl RefetchQueryFilters {
    /// Refetch with `TanStack` defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            filters: QueryFilters::default(),
            cancel_refetch: true,
            throw_on_error: false,
        }
    }

    /// Set base filters.
    #[must_use]
    pub fn filters(mut self, filters: QueryFilters) -> Self {
        self.filters = filters;
        self
    }

    /// Set `cancel_refetch`.
    #[must_use]
    pub fn cancel_refetch(mut self, cancel_refetch: bool) -> Self {
        self.cancel_refetch = cancel_refetch;
        self
    }

    /// Set `throw_on_error`.
    #[must_use]
    pub fn throw_on_error(mut self, throw_on_error: bool) -> Self {
        self.throw_on_error = throw_on_error;
        self
    }
}

/// Immutable snapshot of a mutation used for filter matching.
#[derive(Clone, Debug)]
pub struct MutationSnapshot {
    /// The mutation key, if any.
    pub mutation_key: Option<QueryKey>,
    /// Aggregate mutation status.
    pub status: MutationStatus,
    /// Whether the mutation is currently paused (offline).
    pub is_paused: bool,
}

impl MutationSnapshot {
    /// Build a snapshot from raw parts.
    #[must_use]
    pub fn new(mutation_key: Option<QueryKey>, status: MutationStatus, is_paused: bool) -> Self {
        Self {
            mutation_key,
            status,
            is_paused,
        }
    }
}

/// Predicate receiving a [`MutationSnapshot`].
pub type MutationPredicate = Arc<dyn Fn(&MutationSnapshot) -> bool + Send + Sync>;

/// Filters used to select mutations, mirroring `MutationFilters`.
#[derive(Clone, Default)]
pub struct MutationFilters {
    /// Restrict matching to this mutation key (prefix match unless `exact`).
    pub mutation_key: Option<QueryKey>,
    /// Require the key to match exactly.
    pub exact: bool,
    /// Only match mutations with this status.
    pub status: Option<MutationStatus>,
    /// Only match paused mutations when `true` (`TanStack` `fetchStatus` proxy).
    pub is_paused: Option<bool>,
    /// Custom predicate; overrides every other criterion when set.
    pub predicate: Option<MutationPredicate>,
}

impl MutationFilters {
    /// Filter matching everything.
    #[must_use]
    pub fn all() -> Self {
        Self::default()
    }

    /// Filter by mutation key prefix (or exact when `exact`).
    #[must_use]
    pub fn with_key(key: QueryKey) -> Self {
        Self {
            mutation_key: Some(key),
            ..Self::default()
        }
    }

    /// Restrict to exact key matching.
    #[must_use]
    pub fn exact(mut self) -> Self {
        self.exact = true;
        self
    }

    /// Require a specific mutation status.
    #[must_use]
    pub fn status(mut self, status: MutationStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Require paused (offline) mutations.
    #[must_use]
    pub fn paused(mut self) -> Self {
        self.is_paused = Some(true);
        self
    }

    /// Set a custom predicate.
    #[must_use]
    pub fn predicate(
        mut self,
        predicate: impl Fn(&MutationSnapshot) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.predicate = Some(Arc::new(predicate));
        self
    }
}

impl std::fmt::Debug for MutationFilters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("MutationFilters");
        debug
            .field("mutation_key", &self.mutation_key)
            .field("exact", &self.exact)
            .field("status", &self.status)
            .field("is_paused", &self.is_paused);
        if self.predicate.is_some() {
            debug.field("predicate", &"<fn>");
        }
        debug.finish()
    }
}

/// Shared empty key for mutations without a key.
static EMPTY_KEY: std::sync::LazyLock<QueryKey> = std::sync::LazyLock::new(|| QueryKey::new(""));

/// Key matching used by all filter types: prefix match by default, exact when
/// requested.
#[must_use]
fn key_matches(filter_key: Option<&QueryKey>, candidate: &QueryKey, exact: bool) -> bool {
    match filter_key {
        None => true,
        Some(filter) => {
            if exact {
                filter == candidate
            } else {
                candidate.starts_with(filter)
            }
        }
    }
}

/// `TanStack` `matchQuery`: does `snapshot` satisfy `filters`?
#[must_use]
pub fn match_query(snapshot: &QuerySnapshot, filters: &QueryFilters) -> bool {
    if let Some(predicate) = &filters.predicate {
        return predicate(snapshot);
    }
    if !key_matches(filters.query_key.as_ref(), &snapshot.key, filters.exact) {
        return false;
    }
    match filters.query_type {
        QueryType::Active if !snapshot.is_active => return false,
        QueryType::Inactive if snapshot.is_active => return false,
        QueryType::All | QueryType::Active | QueryType::Inactive => {}
    }
    if filters.stale && !snapshot.is_stale {
        return false;
    }
    if let Some(fetch_status) = filters.fetch_status
        && snapshot.fetch_status != fetch_status
    {
        return false;
    }
    if let Some(status) = filters.status
        && snapshot.status != status
    {
        return false;
    }
    true
}

/// `TanStack` `matchMutation`: does `snapshot` satisfy `filters`?
#[must_use]
pub fn match_mutation(snapshot: &MutationSnapshot, filters: &MutationFilters) -> bool {
    if let Some(predicate) = &filters.predicate {
        return predicate(snapshot);
    }
    if !key_matches(
        filters.mutation_key.as_ref(),
        snapshot.mutation_key.as_ref().unwrap_or(&EMPTY_KEY),
        filters.exact,
    ) {
        return false;
    }
    if let Some(status) = filters.status
        && snapshot.status != status
    {
        return false;
    }
    if let Some(is_paused) = filters.is_paused
        && snapshot.is_paused != is_paused
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn query_snap(
        is_active: bool,
        is_stale: bool,
        fetch_status: FetchStatus,
        status: QueryStatus,
    ) -> QuerySnapshot {
        QuerySnapshot::new(
            QueryKey::new("users").with("id", 1),
            is_active,
            is_stale,
            fetch_status,
            status,
            true,
        )
    }

    #[test]
    fn query_type_defaults_and_equality() {
        assert_eq!(QueryType::default(), QueryType::All);
        assert_eq!(QueryType::All, QueryType::All);
        assert_ne!(QueryType::All, QueryType::Active);
        assert_ne!(QueryType::Active, QueryType::Inactive);
        assert_eq!(format!("{:?}", QueryType::Inactive), "Inactive");

        let mut set = HashSet::new();
        set.insert(QueryType::All);
        set.insert(QueryType::Active);
        set.insert(QueryType::All);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn refetch_type_defaults_and_equality() {
        assert_eq!(RefetchType::default(), RefetchType::Active);
        assert_eq!(RefetchType::All, RefetchType::All);
        assert_ne!(RefetchType::All, RefetchType::None);
        assert_ne!(RefetchType::Inactive, RefetchType::Active);
        assert_eq!(format!("{:?}", RefetchType::None), "None");

        let mut set = HashSet::new();
        set.insert(RefetchType::Active);
        set.insert(RefetchType::Inactive);
        set.insert(RefetchType::Active);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn query_snapshot_new_exposes_every_part() {
        let key = QueryKey::new("todos");
        let snapshot = QuerySnapshot::new(
            key.clone(),
            true,
            false,
            FetchStatus::Fetching,
            QueryStatus::Success,
            true,
        );
        assert_eq!(snapshot.key, key);
        assert!(snapshot.is_active);
        assert!(!snapshot.is_stale);
        assert_eq!(snapshot.fetch_status, FetchStatus::Fetching);
        assert_eq!(snapshot.status, QueryStatus::Success);
        assert!(snapshot.has_data);

        let clone = snapshot.clone();
        assert_eq!(clone.key, key);
        assert!(format!("{clone:?}").contains("todos"));
    }

    #[test]
    fn query_filters_all_matches_everything() {
        let filters = QueryFilters::all();
        let active_fresh = query_snap(true, false, FetchStatus::Idle, QueryStatus::Success);
        let inactive_pending = query_snap(false, false, FetchStatus::Idle, QueryStatus::Pending);
        assert!(match_query(&active_fresh, &filters));
        assert!(match_query(&inactive_pending, &filters));
        assert!(filters.query_key.is_none());
        assert!(!filters.exact);
        assert_eq!(filters.query_type, QueryType::All);
        assert!(!filters.stale);
        assert!(filters.fetch_status.is_none());
        assert!(filters.status.is_none());
        assert!(filters.predicate.is_none());
        assert!(!filters.is_stale_only());
    }

    #[test]
    fn query_filters_with_key_prefix_matches_by_default() {
        let filters = QueryFilters::with_key(QueryKey::new("users"));
        let nested = QuerySnapshot::new(
            QueryKey::new("users").with("id", 7),
            true,
            false,
            FetchStatus::Idle,
            QueryStatus::Success,
            true,
        );
        let sibling = QuerySnapshot::new(
            QueryKey::new("teams"),
            true,
            false,
            FetchStatus::Idle,
            QueryStatus::Success,
            true,
        );
        assert!(match_query(&nested, &filters));
        assert!(!match_query(&sibling, &filters));
    }

    #[test]
    fn query_filters_exact_requires_full_key_equality() {
        let filters = QueryFilters::with_key(QueryKey::new("users")).exact();
        assert!(filters.exact);

        let exact = QuerySnapshot::new(
            QueryKey::new("users"),
            true,
            false,
            FetchStatus::Idle,
            QueryStatus::Success,
            true,
        );
        let nested = QuerySnapshot::new(
            QueryKey::new("users").with("id", 7),
            true,
            false,
            FetchStatus::Idle,
            QueryStatus::Success,
            true,
        );
        assert!(match_query(&exact, &filters));
        assert!(!match_query(&nested, &filters));
    }

    #[test]
    fn query_filters_active_and_inactive_types() {
        let active = QuerySnapshot::new(
            QueryKey::new("k"),
            true,
            false,
            FetchStatus::Idle,
            QueryStatus::Success,
            true,
        );
        let inactive = QuerySnapshot::new(
            QueryKey::new("k"),
            false,
            false,
            FetchStatus::Idle,
            QueryStatus::Success,
            true,
        );

        let active_filters = QueryFilters::all().active();
        assert_eq!(active_filters.query_type, QueryType::Active);
        assert!(match_query(&active, &active_filters));
        assert!(!match_query(&inactive, &active_filters));

        let inactive_filters = QueryFilters::all().inactive();
        assert_eq!(inactive_filters.query_type, QueryType::Inactive);
        assert!(match_query(&inactive, &inactive_filters));
        assert!(!match_query(&active, &inactive_filters));
    }

    #[test]
    fn query_filters_stale_only_matches_stale_data() {
        let filters = QueryFilters::all().stale();
        assert!(filters.stale);
        assert!(filters.is_stale_only());

        let stale = query_snap(true, true, FetchStatus::Idle, QueryStatus::Success);
        let fresh = query_snap(true, false, FetchStatus::Idle, QueryStatus::Success);
        assert!(match_query(&stale, &filters));
        assert!(!match_query(&fresh, &filters));
    }

    #[test]
    fn query_filters_fetch_status_and_status() {
        let filters = QueryFilters::all()
            .fetch_status(FetchStatus::Fetching)
            .status(QueryStatus::Pending);
        assert_eq!(filters.fetch_status, Some(FetchStatus::Fetching));
        assert_eq!(filters.status, Some(QueryStatus::Pending));

        let matching = query_snap(true, false, FetchStatus::Fetching, QueryStatus::Pending);
        let wrong_fetch = query_snap(true, false, FetchStatus::Paused, QueryStatus::Pending);
        let wrong_status = query_snap(true, false, FetchStatus::Fetching, QueryStatus::Success);
        assert!(match_query(&matching, &filters));
        assert!(!match_query(&wrong_fetch, &filters));
        assert!(!match_query(&wrong_status, &filters));
    }

    #[test]
    fn query_filters_predicate_overrides_all_other_criteria() {
        let filters = QueryFilters::all().predicate(|snapshot| snapshot.has_data);
        assert!(filters.predicate.is_some());

        let with_data = query_snap(false, false, FetchStatus::Idle, QueryStatus::Pending);
        let without_data = QuerySnapshot::new(
            QueryKey::new("other"),
            false,
            false,
            FetchStatus::Idle,
            QueryStatus::Pending,
            false,
        );
        assert!(match_query(&with_data, &filters));
        assert!(!match_query(&without_data, &filters));

        // The predicate wins even when the key would not match.
        let keyed = QueryFilters::with_key(QueryKey::new("nomatch")).predicate(|_| true);
        assert!(match_query(&with_data, &keyed));
    }

    #[test]
    fn query_filters_debug_omits_or_marks_predicate() {
        let plain = QueryFilters::all();
        let plain_debug = format!("{plain:?}");
        assert!(plain_debug.contains("QueryFilters"));
        assert!(plain_debug.contains("query_key"));
        assert!(plain_debug.contains("exact"));
        assert!(plain_debug.contains("query_type"));
        assert!(plain_debug.contains("stale"));
        assert!(plain_debug.contains("fetch_status"));
        assert!(plain_debug.contains("status"));
        assert!(!plain_debug.contains("predicate"));

        let with_predicate = QueryFilters::all().predicate(|_| true);
        let predicate_debug = format!("{with_predicate:?}");
        assert!(predicate_debug.contains("predicate"));
        assert!(predicate_debug.contains("<fn>"));

        let keyed = format!("{:?}", QueryFilters::with_key(QueryKey::new("users")));
        assert!(keyed.contains("users"));
    }

    #[test]
    fn invalidate_query_filters_builders() {
        let default = InvalidateQueryFilters::new();
        assert_eq!(default.refetch_type, RefetchType::Active);
        assert!(default.filters.query_key.is_none());
        assert!(!default.filters.stale);

        let custom = InvalidateQueryFilters::new()
            .filters(QueryFilters::with_key(QueryKey::new("users")).stale())
            .refetch_type(RefetchType::None);
        assert_eq!(custom.refetch_type, RefetchType::None);
        assert!(custom.filters.stale);
        assert!(format!("{custom:?}").contains("InvalidateQueryFilters"));

        let clone = custom.clone();
        assert_eq!(clone.refetch_type, RefetchType::None);
    }

    #[test]
    fn refetch_query_filters_builders() {
        let default = RefetchQueryFilters::new();
        assert!(default.cancel_refetch);
        assert!(!default.throw_on_error);
        assert!(default.filters.query_key.is_none());

        let custom = RefetchQueryFilters::new()
            .filters(QueryFilters::with_key(QueryKey::new("teams")))
            .cancel_refetch(false)
            .throw_on_error(true);
        assert!(!custom.cancel_refetch);
        assert!(custom.throw_on_error);
        assert_eq!(custom.filters.query_key, Some(QueryKey::new("teams")));

        let clone = custom.clone();
        assert!(clone.throw_on_error);
        assert!(format!("{clone:?}").contains("RefetchQueryFilters"));
    }

    #[test]
    fn mutation_snapshot_new_exposes_every_part() {
        let snapshot = MutationSnapshot::new(
            Some(QueryKey::new("todo").with("id", 3)),
            MutationStatus::Pending,
            true,
        );
        assert_eq!(
            snapshot.mutation_key,
            Some(QueryKey::new("todo").with("id", 3))
        );
        assert_eq!(snapshot.status, MutationStatus::Pending);
        assert!(snapshot.is_paused);

        let keyless = MutationSnapshot::new(None, MutationStatus::Idle, false);
        assert!(keyless.mutation_key.is_none());
        assert_eq!(keyless.status, MutationStatus::Idle);
        assert!(!keyless.is_paused);

        let clone = keyless.clone();
        assert!(clone.mutation_key.is_none());
        assert!(format!("{snapshot:?}").contains("todo"));
    }

    #[test]
    fn mutation_filters_builders() {
        let all = MutationFilters::all();
        assert!(all.mutation_key.is_none());
        assert!(!all.exact);
        assert!(all.status.is_none());
        assert!(all.is_paused.is_none());

        let keyed = MutationFilters::with_key(QueryKey::new("todo"));
        assert_eq!(keyed.mutation_key, Some(QueryKey::new("todo")));

        let custom = MutationFilters::all()
            .exact()
            .status(MutationStatus::Success)
            .paused();
        assert!(custom.exact);
        assert_eq!(custom.status, Some(MutationStatus::Success));
        assert_eq!(custom.is_paused, Some(true));
    }

    #[test]
    fn mutation_filters_debug_omits_or_marks_predicate() {
        let plain = MutationFilters::all();
        let plain_debug = format!("{plain:?}");
        assert!(plain_debug.contains("MutationFilters"));
        assert!(plain_debug.contains("mutation_key"));
        assert!(plain_debug.contains("exact"));
        assert!(plain_debug.contains("status"));
        assert!(plain_debug.contains("is_paused"));
        assert!(!plain_debug.contains("predicate"));

        let with_predicate = MutationFilters::all().predicate(|_| true);
        let predicate_debug = format!("{with_predicate:?}");
        assert!(predicate_debug.contains("predicate"));
        assert!(predicate_debug.contains("<fn>"));
    }

    #[test]
    fn match_mutation_prefix_and_exact_key_matching() {
        let keyed = MutationSnapshot::new(
            Some(QueryKey::new("todo").with("id", 1)),
            MutationStatus::Idle,
            false,
        );

        let prefix = MutationFilters::with_key(QueryKey::new("todo"));
        assert!(match_mutation(&keyed, &prefix));

        let exact = prefix.clone().exact();
        assert!(!match_mutation(&keyed, &exact));
        let exact_match = MutationFilters::with_key(QueryKey::new("todo").with("id", 1)).exact();
        assert!(match_mutation(&keyed, &exact_match));

        let other = MutationFilters::with_key(QueryKey::new("posts"));
        assert!(!match_mutation(&keyed, &other));
    }

    #[test]
    fn match_mutation_keyless_snapshot_uses_empty_key() {
        let keyless = MutationSnapshot::new(None, MutationStatus::Idle, false);

        // Any concrete filter key cannot prefix-match the shared empty key.
        let filters = MutationFilters::with_key(QueryKey::new("todo"));
        assert!(!match_mutation(&keyless, &filters));

        // Filtering by the empty key matches keyless mutations.
        let empty_key = MutationFilters::with_key(QueryKey::new(""));
        assert!(match_mutation(&keyless, &empty_key));

        // No key filter matches everything, keyless or not.
        assert!(match_mutation(&keyless, &MutationFilters::all()));
    }

    #[test]
    fn match_mutation_status_and_paused() {
        let snapshot =
            MutationSnapshot::new(Some(QueryKey::new("todo")), MutationStatus::Error, true);

        let matching = MutationFilters::all()
            .status(MutationStatus::Error)
            .paused();
        assert!(match_mutation(&snapshot, &matching));

        let wrong_status = MutationFilters::all().status(MutationStatus::Pending);
        assert!(!match_mutation(&snapshot, &wrong_status));

        let paused_only = MutationFilters::all().paused();
        assert!(match_mutation(&snapshot, &paused_only));

        let not_paused =
            MutationSnapshot::new(Some(QueryKey::new("todo")), MutationStatus::Idle, false);
        assert!(!match_mutation(&not_paused, &paused_only));
    }

    #[test]
    fn match_mutation_predicate_overrides_all_other_criteria() {
        let snapshot =
            MutationSnapshot::new(Some(QueryKey::new("todo")), MutationStatus::Idle, false);

        let filters = MutationFilters::with_key(QueryKey::new("posts"))
            .status(MutationStatus::Pending)
            .predicate(|s| s.status == MutationStatus::Idle);
        assert!(match_mutation(&snapshot, &filters));

        let rejecting = MutationFilters::all().predicate(|_| false);
        assert!(!match_mutation(&snapshot, &rejecting));
    }

    #[test]
    fn match_query_status_and_fetch_status_mismatches_return_false() {
        let snapshot = query_snap(true, false, FetchStatus::Fetching, QueryStatus::Pending);

        let fetch_ok = QueryFilters::all().fetch_status(FetchStatus::Fetching);
        assert!(match_query(&snapshot, &fetch_ok));
        let fetch_bad = QueryFilters::all().fetch_status(FetchStatus::Paused);
        assert!(!match_query(&snapshot, &fetch_bad));

        let status_ok = QueryFilters::all().status(QueryStatus::Pending);
        assert!(match_query(&snapshot, &status_ok));
        let status_bad = QueryFilters::all().status(QueryStatus::Error);
        assert!(!match_query(&snapshot, &status_bad));
    }
}
