//! rs-query: TanStack Query-inspired data fetching for GPUI

mod client;
mod error;
mod executor;
mod focus_manager;
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

pub use client::QueryClient;
pub use error::QueryError;
pub use executor::{spawn_mutation, spawn_query};
pub use focus_manager::FocusManager;
pub use infinite::{InfiniteData, InfiniteQuery};
pub use infinite_executor::{spawn_infinite_query, InfiniteQueryObserver};
pub use key::QueryKey;
pub use mutation::{Mutation, MutationState, RollbackContext};
pub use observer::{QueryObserver, QueryStateUpdate, QueryStateVariant};
pub use options::{InitialData, PlaceholderData, QueryOptions, RefetchOnMount, RetryConfig};
pub use query::Query;
pub use sharing::replace_equal_deep;
pub use state::QueryState;

pub use hydration::{DehydratedQuery, DehydratedQueryOptions, DehydratedState, HydrateOptions};
