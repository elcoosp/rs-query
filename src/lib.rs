//! rs-query: TanStack Query-inspired data fetching for GPUI
//!
//! # Features
//!
//! - **Declarative queries** - Define once, auto-cache
//! - **spawn_query/spawn_mutation** - One-liner async execution
//! - **Stale-while-revalidate** - Show cached data while fetching
//! - **Automatic retries** - Exponential backoff for transient failures
//! - **Cache invalidation** - Hierarchical key patterns
//!
//! # Example
//!
//! ```rust,ignore
//! use rs_query::{QueryClient, QueryKey, QueryState, Query, spawn_query};
//!
//! // Define a query
//! let query = Query::new(QueryKey::new("users"), || async {
//!     fetch_users().await
//! });
//!
//! // Execute with automatic state management
//! spawn_query(cx, &client, &query, |this, state, cx| {
//!     match state {
//!         QueryState::Success(users) => this.users = users,
//!         QueryState::Error { error, .. } => this.error = Some(error),
//!         _ => {}
//!     }
//!     cx.notify();
//! });
//! ```

mod client;
mod error;
mod executor;
mod key;
mod mutation;
mod options;
mod query;
mod state;

pub use client::QueryClient;
pub use error::QueryError;
pub use executor::{spawn_mutation, spawn_query};
pub use key::QueryKey;
pub use mutation::{Mutation, MutationState};
pub use options::{QueryOptions, RefetchOnMount, RetryConfig};
pub use query::Query;
pub use state::QueryState;
