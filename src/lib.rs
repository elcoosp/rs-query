// src/lib.rs
//! rs-query — `TanStack` Query-inspired async state management, **portable to
//! every [quoin]-supported UI framework**.
//!
//! rs-query is a Rust port of the `TanStack` Query stack (query-core +
//! framework adapters). As of 0.2 it no longer hardcodes GPUI or tokio: the
//! core is runtime-agnostic and the reactive layer is generic over
//! [`quoin_core::ReactiveContext`], so the same code runs on gpui, leptos,
//! dioxus, xilem, floem — or with no UI framework at all (the default
//! [`PoolRuntime`]).
//!
//! # Quick start (headless)
//!
//! ```no_run
//! use rs_query::{QueryClient, Query, QueryKey};
//!
//! # async fn demo() {
//! let client = QueryClient::new();
//! let query = Query::new(QueryKey::new("todos"), || async {
//!     Ok(vec!["write the plan".to_string()])
//! });
//! let handle = rs_query::spawn_query(&client, &query);
//! let state = handle.await;
//! assert!(state.is_success());
//! # }
//! ```
//!
//! # Quick start (a quoin app)
//!
//! ```ignore
//! use quoin::ReactiveContext;
//!
//! let client = QueryClient::with_context(&cx);          // cx: the framework context
//! let binding = rs_query::use_query(&cx, &client, &query);
//! // binding.state is the framework's own signal: read it from your view.
//! ```
//!
//! See `docs/quoin-migration-plan.md` for the full migration contract and
//! `docs/research/` for the audited `TanStack` semantics behind every module.
//!
//! [quoin]: https://github.com/elcoosp/quoin

mod broadcast;
mod bus;
mod cancellation;
mod client;
mod error;
mod executor;
mod filters;
mod focus_manager;
mod hydration;
mod infinite;
mod infinite_executor;
mod key;
mod mutation;
mod mutation_cache;
mod mutation_observer;
mod network;
mod notify_manager;
mod observer;
mod options;
mod persist_client;
mod persister;
mod queries_observer;
mod query;
mod retryer;
mod runtime;
mod sharing;
mod state;
mod streamed_query;
mod timeout_manager;
mod updater;
mod watch;

#[cfg(feature = "devtools")]
pub mod devtools;

/// Bindings generic over any quoin [`quoin_core::ReactiveContext`].
pub mod hooks;

pub use broadcast::{
    BroadcastHandle, BroadcastQueryClientOptions, broadcast_query_client, publish_state,
};
pub use cancellation::{CancellationToken, cancellable_fetch};
pub use client::{ActivityEvent, QueryClient, QueryStateSnapshot};
pub use error::QueryError;
pub use executor::{execute_mutation, execute_query, spawn_mutation, spawn_query};
pub use filters::{
    InvalidateQueryFilters, MutationFilters, MutationSnapshot, QueryFilters, QuerySnapshot,
    QueryType, RefetchQueryFilters, RefetchType, match_mutation, match_query,
};
pub use focus_manager::FocusManager;
pub use hooks::{
    use_activity_feed, use_any_fetching, use_fetching_count, use_infinite_query, use_is_fetching,
    use_is_mutating, use_mutating_count, use_mutation, use_queries, use_query,
};
pub use hydration::{
    DehydrateOptions, DehydratedMutation, DehydratedQuery, DehydratedState, HydrateOptions,
    QUERY_TYPE_INFINITE, QUERY_TYPE_QUERY, dehydrate_state, hydrate_with_options,
};
pub use infinite::{InfiniteData, InfiniteQuery};
pub use infinite_executor::InfiniteQueryObserver;
pub use infinite_executor::spawn_infinite_query;
pub use key::QueryKey;
pub use mutation::{MutateOptions, Mutation as MutationCore, execute_mutation_full};
pub use mutation_cache::MutationScope;
pub use mutation_cache::{
    MutationCache, MutationCacheConfig, MutationCacheEntry, MutationCacheEvent,
};
pub use mutation_observer::MutationObserver;
pub use network::{NetworkMode, OnlineManager, can_fetch, can_retry};
pub use notify_manager::NotifyManager;
pub use observer::{QueryObserver, QueryResultFlags, QueryStateUpdate, QueryStateVariant};
pub use options::{
    BASE_RETRY_DELAY, DEFAULT_GC_TIME, MAX_RETRY_DELAY, PlaceholderData, QueryOptions,
    RefetchTrigger, RetryConfig,
};
pub use persist_client::{
    ClientPersister, InMemoryClientPersister, MAX_RESTORE_RETRIES, PersistQueryClientHandle,
    PersistQueryClientOptions, PersistedClient, persist_query_client, persist_query_client_restore,
    persist_query_client_save, persist_with_retry, remove_oldest_query,
};
pub use persister::{
    InMemoryQueryStorage, PersistedQuery, PersistedQueryState, QueryPersister,
    QueryPersisterOptions, QueryStorage,
};
#[cfg(feature = "serde")]
pub use persister::{deserialize_persisted_query, serialize_persisted_query};
pub use queries_observer::QueriesObserver;
pub use query::Query;
pub use retryer::{RetryDecision, Retryer, retry_delay};
pub use runtime::{
    BoxFuture, ExecutorRuntime, PoolRuntime, QueryRuntime, SpawnHandle, SpawnedTask,
};
pub use state::{
    FetchDirection, FetchStatus, MutationState, MutationStatus, QueryState, QueryStatus,
};
pub use streamed_query::{RefetchMode, StreamedQueryConfig, streamed_query};
pub use timeout_manager::{QuoinSleepProvider, SleepProvider, TimeoutManager};
pub use updater::{Updater, functional_update};

#[cfg(test)]
pub(crate) mod test_support {
    //! Shared test helpers: a client whose runtime is the ambient tokio test
    //! runtime (so paused-time tests control every timer deterministically).

    use crate::QueryClient;

    /// A client backed by the ambient tokio runtime (use inside
    /// `#[tokio::test]`); with `start_paused = true` all internal sleeps are
    /// virtual-time controlled.
    #[must_use]
    pub fn tokio_client() -> QueryClient {
        QueryClient::with_runtime(std::sync::Arc::new(crate::runtime::ExecutorRuntime::new(
            quoin_testkit::TokioExecutor::global(),
        )))
    }
}
