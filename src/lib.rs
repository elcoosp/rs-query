// src/lib.rs
//! rs-query — `TanStack` Query-inspired async state management for GPUI.
//!
//! A Rust port of the `TanStack` Query stack (query-core + framework adapters)
//! built on [GPUI]. See `docs/port-plan.md` for the full parity contract and
//! `docs/research/` for the audited `TanStack` semantics behind every module.
//!
//! [GPUI]: https://github.com/zed-industries/zed

mod broadcast;
mod cancellation;
mod client;
mod error;
mod executor;
mod filters;
mod focus_manager;
mod hooks;
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
mod sharing;
mod state;
mod streamed_query;
mod timeout_manager;
mod updater;

#[cfg(feature = "devtools")]
pub mod devtools;

pub use broadcast::{
    BroadcastHandle, BroadcastQueryClientOptions, broadcast_query_client, publish_state,
};
pub use cancellation::cancellable_fetch;
pub use client::{ActivityEvent, QueryClient, QueryStateSnapshot};
pub use error::QueryError;
pub use executor::{spawn_mutation, spawn_query};
pub use filters::{
    InvalidateQueryFilters, MutationFilters, MutationSnapshot, QueryFilters, QuerySnapshot,
    QueryType, RefetchQueryFilters, RefetchType, match_mutation, match_query,
};
pub use focus_manager::FocusManager;
pub use hooks::{
    use_activity_feed, use_any_fetching, use_fetching_count, use_is_fetching, use_is_mutating,
    use_mutating_count, use_queries,
};
pub use hydration::{
    DehydrateOptions, DehydratedMutation, DehydratedQuery, DehydratedState, HydrateOptions,
    QUERY_TYPE_INFINITE, QUERY_TYPE_QUERY, dehydrate_state, hydrate_with_options,
};
pub use infinite::{InfiniteData, InfiniteQuery};
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
pub use state::{
    FetchDirection, FetchStatus, MutationState, MutationStatus, QueryState, QueryStatus,
};
pub use streamed_query::{RefetchMode, StreamedQueryConfig, streamed_query};
pub use timeout_manager::{SleepProvider, TimeoutManager, TokioSleepProvider};
pub use updater::{Updater, functional_update};
