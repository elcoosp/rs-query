# rs-query

**rs-query** is a Rust port of the popular [TanStack Query](https://tanstack.com/query) data‑fetching library, built specifically for the [GPUI](https://www.gpui.rs/) framework. It provides declarative, cache‑aware queries and mutations with automatic background refetching, retries, and hierarchical cache invalidation.

## Features

- **Declarative Queries** – Define a query once with a unique key and async fetch function.  
- **Automatic Caching** – Data is stored by key and served instantly while background updates occur.  
- **Stale‑While‑Revalidate** – Show cached (stale) data immediately and refresh in the background.  
- **Smart Retries** – Exponential backoff for transient failures, configurable per query.  
- **Request Deduplication** – Identical concurrent queries are coalesced into a single network call.  
- **Hierarchical Invalidation** – Invalidate whole families of queries using key prefix matching.  
- **Type‑Safe** – Fully generic over your data types with `Send + Sync` bounds for seamless async use.  
- **GPUI Native** – Integrates directly with GPUI’s `Context` and `BackgroundExecutor`.

## Installation

Add `rs-query` to your `Cargo.toml`:

```toml
[dependencies]
rs-query = "0.1.0"   # replace with actual version
```

Make sure your project also depends on `gpui` and a Tokio runtime (rs‑query uses `tokio::spawn` internally for retry delays).

## Quick Start

### 1. Create a QueryClient

Hold a `QueryClient` in your application state (e.g., in a GPUI `struct`). It manages the cache and can be cloned cheaply.

```rust
use rs_query::QueryClient;

struct AppState {
    query_client: QueryClient,
}

impl AppState {
    fn new() -> Self {
        Self {
            query_client: QueryClient::new(),
        }
    }
}
```

### 2. Define a Query

```rust
use rs_query::{Query, QueryKey, QueryError};

async fn fetch_users() -> Result<Vec<User>, QueryError> {
    // Your actual API call
    Ok(vec![])
}

let users_query = Query::new(QueryKey::new("users"), fetch_users)
    .stale_time(std::time::Duration::from_secs(60));
```

### 3. Execute the Query in a GPUI View

Use `spawn_query` inside your `render` or an action handler. It automatically updates the cache and invokes your callback when the fetch completes (or fails).

```rust
use rs_query::{spawn_query, QueryState};

fn fetch_users(&mut self, cx: &mut ViewContext<Self>) {
    let client = self.query_client.clone();
    let query = self.users_query.clone();

    spawn_query(cx, &client, &query, move |this, state, cx| {
        match state {
            QueryState::Success(users) => {
                this.users = users;
                this.error = None;
            }
            QueryState::Error { error, stale_data } => {
                this.error = Some(error.to_string());
                // Optionally fall back to stale_data
                if let Some(stale) = stale_data {
                    this.users = stale;
                }
            }
            QueryState::Loading => {
                // Show a loading indicator
            }
            _ => {}
        }
        cx.notify();
    });
}
```

### 4. Perform Mutations with Automatic Invalidation

```rust
use rs_query::{Mutation, spawn_mutation, MutationState};

let create_user = Mutation::new(|params: CreateUserParams| async move {
    api::create_user(params).await
})
.invalidates_key(QueryKey::new("users"));

fn create_user(&mut self, cx: &mut ViewContext<Self>, params: CreateUserParams) {
    let client = self.query_client.clone();
    let mutation = self.create_user_mutation.clone();

    spawn_mutation(cx, &client, &mutation, params, move |this, state, cx| {
        match state {
            MutationState::Success(user) => {
                this.show_success("User created");
                // The "users" query is automatically invalidated and will refetch next time
            }
            MutationState::Error(e) => {
                this.show_error(e.to_string());
            }
            _ => {}
        }
        cx.notify();
    });
}
```

## API Overview

### `QueryClient`

Central cache and query manager.  
- `get_query_data<T>(key: &QueryKey) -> Option<T>`  
- `set_query_data<T>(key: &QueryKey, data: T, options: QueryOptions)`  
- `invalidate_queries(pattern: &QueryKey)` – marks matching entries as stale  
- `clear()` – empties the entire cache  
- `gc()` – removes entries older than their `gc_time`

### `Query<T>`

Definition of a query.  
- `Query::new(key, fetch_fn)` – builder starts here  
- `.stale_time(duration)` – how long data is considered fresh  
- `.gc_time(duration)` – inactive cache lifetime  
- `.retry(config)` – custom retry behaviour  
- `.enabled(bool)` – conditionally disable the query

### `Mutation<T, P>`

Definition of a mutation.  
- `Mutation::new(mutate_fn)` – builder start  
- `.invalidates_key(key)` / `.invalidates(keys)` – keys to invalidate on success

### `QueryKey`

Hierarchical key system for cache lookups and invalidation.

```rust
// Static key
QueryKey::new("users")

// Parameterised
QueryKey::new("users").with("id", 42)

// Nested
QueryKey::new("users").segment("posts").with("id", 100)
```

Invalidation is prefix‑based: invalidating `QueryKey::new("users")` will mark `users::id=42` and `users::posts::id=100` as stale.

### `QueryState<T>` & `MutationState<T>`

Enums that carry data alongside the state.  
- `QueryState::Idle` / `Loading` / `Refetching(T)` / `Success(T)` / `Stale(T)` / `Error { error, stale_data }`  
- Convenience methods: `.data()`, `.is_loading()`, `.is_success()`, etc.

### Executors

- `spawn_query(cx, client, query, callback)` – executes a query on GPUI’s background executor.  
- `spawn_mutation(cx, client, mutation, params, callback)` – executes a mutation and invalidates relevant queries on success.

## Comparison with TanStack Query

rs‑query aims to provide the core experience of TanStack Query while respecting Rust’s ownership model and GPUI’s async patterns. Below is a summary of what is currently implemented and what remains on the roadmap.

| Feature                                   | Status         |
|-------------------------------------------|----------------|
| Query cache & GC                          | Implemented    |
| Stale‑while‑revalidate                    | Implemented    |
| Automatic retries (exponential backoff)   | Implemented    |
| Request deduplication                     | Implemented    |
| Hierarchical key invalidation             | Implemented    |
| Query / mutation observers                | Not yet        |
| Optimistic updates                        | Not yet        |
| Infinite queries / pagination             | Not yet        |
| `placeholderData` / `initialData`         | Not yet        |
| `select` transformation                   | Not yet        |
| `refetchInterval` / window focus refetch  | Not yet (GPUI lacks focus events) |
| Hydration / persistence                   | Not yet        |
| Devtools                                  | Not yet        |
| Suspense integration                      | N/A (GPUI does not use Suspense) |
| Structural sharing                        | Not yet        |
| Query cancellation                        | Partial (Tokio tasks can be aborted) |

## Contributing

Contributions are welcome! If you’d like to help implement missing features or improve existing ones, please open an issue or pull request on the repository. Focus areas include:

- Infinite query support  
- Optimistic mutation updates  
- Cache persistence with `serde`  
- Framework adapters for other Rust UI libraries (Dioxus, Leptos, etc.)

## License

rs‑query is distributed under the terms of both the MIT license and the Apache License (Version 2.0). See [LICENSE-APACHE](LICENSE-APACHE) and [LICENSE-MIT](LICENSE-MIT) for details.

## Acknowledgements

This project is heavily inspired by [TanStack Query](https://tanstack.com/query) and built for the [GPUI](https://www.gpui.rs/) ecosystem. Thanks to the maintainers of both projects for their excellent work.
