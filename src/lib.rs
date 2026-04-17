// src/lib.rs

pub mod cancellation;
pub mod client;
#[cfg(feature = "devtools")]
pub mod devtools;
pub mod error;
pub mod executor;
pub mod focus_manager;
pub mod hooks;
pub mod infinite;
pub mod infinite_executor;
pub mod key;
pub mod mutation;
pub mod observer;
pub mod options;
pub mod query; // ← new file (previously part of state.rs)
pub mod sharing;
pub mod state; // ← now contains only QueryState/MutationState definitions

// Re-exports for convenience
pub use client::QueryClient;
pub use error::QueryError;
pub use executor::{spawn_infinite_query, spawn_mutation, spawn_query};
pub use hooks::{use_fetching_count, use_is_fetching, use_is_mutating, use_mutating_count};
pub use infinite::InfiniteData;
pub use infinite_executor::InfiniteQueryObserver;
pub use key::QueryKey;
pub use mutation::Mutation;
pub use observer::QueryObserver;
pub use options::{QueryOptions, RetryConfig};
pub use query::{PlaceholderData, Query};
pub use state::{MutationState, QueryState, QueryStateUpdate, QueryStateVariant};
