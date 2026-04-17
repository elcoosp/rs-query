// src/lib.rs
//! rs-query - TanStack Query-inspired async state management for GPUI

mod cancellation;
mod client;
mod error;
mod executor;
mod focus_manager;
mod hooks;
mod hydration;
mod infinite;
mod infinite_executor;
mod key;
mod mutation;
mod observer;
mod options;
mod query;
mod sharing;
mod state;

#[cfg(feature = "devtools")]
pub mod devtools;

pub use cancellation::cancellable_fetch;
pub use client::{ActivityEvent, QueryClient};
pub use error::QueryError;
pub use executor::{spawn_mutation, spawn_query};
pub use focus_manager::FocusManager;
pub use hooks::{use_fetching_count, use_is_fetching, use_is_mutating, use_mutating_count};
pub use hydration::{DehydratedQuery, DehydratedState, HydrateOptions};
pub use infinite::{InfiniteData, InfiniteQuery};
pub use infinite_executor::spawn_infinite_query;
pub use key::QueryKey;
pub use mutation::Mutation;
pub use observer::{QueryObserver, QueryStateUpdate, QueryStateVariant};
pub use options::{PlaceholderData, QueryOptions, RetryConfig};
pub use query::Query;
pub use state::{MutationState, QueryState};
