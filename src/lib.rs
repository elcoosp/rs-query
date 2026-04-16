//! rs-query: TanStack Query-inspired data fetching for GPUI

mod client;
mod error;
mod executor;
mod key;
mod mutation;
mod observer;
mod options;
mod query;
mod state;

pub use client::{QueryClient, QueryStateUpdate};
pub use error::QueryError;
pub use executor::{spawn_mutation, spawn_query};
pub use key::QueryKey;
pub use mutation::{Mutation, MutationState, RollbackContext};
pub use observer::{QueryObserver, QueryStateVariant};
pub use options::{QueryOptions, RefetchOnMount, RetryConfig};
pub use query::Query;
pub use state::QueryState;
