# rs-query

**rs-query** is a Rust port of the popular [TanStack Query](https://tanstack.com/query) data‑fetching library, built specifically for the [GPUI](https://www.gpui.rs/) framework. It provides declarative, cache‑aware queries and mutations with automatic background refetching, retries, hierarchical cache invalidation, and reactive observers.

## Features

- **Declarative Queries** – Define a query once with a unique key and async fetch function.
- **Automatic Caching** – Data is stored by key and served instantly while background updates occur.
- **Stale‑While‑Revalidate** – Show cached (stale) data immediately and refresh in the background.
- **Smart Retries** – Exponential backoff for transient failures, configurable per query.
- **Request Deduplication** – Identical concurrent queries are coalesced into a single network call.
- **Hierarchical Invalidation** – Invalidate whole families of queries using key prefix matching.
- **Reactive Observers** – `QueryObserver` subscribes to cache changes for automatic UI updates.
- **Optimistic Updates** – Mutations support `on_mutate` with automatic rollback on failure.
- **Infinite Queries** – Paginated data with `fetch_next_page`, `has_next_page`, and `max_pages`.
- **Query Options** – `initialData`, `placeholderData`, and `select` transformations.
- **Background Refetch** – Configurable `refetch_interval` and window focus refetching.
- **Hydration & Persistence** – `dehydrate` and `hydrate` APIs for SSR and offline storage.
- **Devtools** – Optional GPUI component for inspecting and debugging the query cache.
- **Type‑Safe** – Fully generic over your data types with `Send + Sync` bounds for seamless async use.
- **GPUI Native** – Integrates directly with GPUI’s `Context` and `BackgroundExecutor`.

## Installation

Add `rs-query` as a Git dependency in your `Cargo.toml`:

```toml
[dependencies]
rs-query = { git = "https://github.com/elcoosp/rs-query" }

# Optional: enable devtools
# rs-query = { git = "...", features = ["devtools"] }
```

Make sure your project already depends on `gpui` and `tokio` (with `rt-multi-thread`, `time`, and `sync` features).

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

### 3. Observe and Execute the Query

Use `QueryObserver` to get reactive state updates:

```rust
use rs_query::{QueryObserver, spawn_query};

fn fetch_users(&mut self, cx: &mut ViewContext<Self>) {
    let client = self.query_client.clone();
    let query = self.users_query.clone();

    let observer = QueryObserver::new(&client, query.key.clone());
    spawn_query(cx, &client, &query, move |this, state, cx| {
        // The observer will automatically update its state
        cx.notify();
    });

    // Store the observer to access state later
    self.users_observer = Some(observer);
}
```

### 4. Perform Mutations with Optimistic Updates

```rust
use rs_query::{Mutation, spawn_mutation, MutationState};

let create_user = Mutation::new(|params: CreateUserParams| async move {
    api::create_user(params).await
})
.invalidates_key(QueryKey::new("users"))
.on_mutate(|client, params| {
    // Optimistically add to cache
    client.set_query_data(&QueryKey::new("users"), |old: Option<Vec<User>>| {
        let mut users = old.unwrap_or_default();
        users.push(User { id: params.id, name: params.name.clone() });
        users
    });
    // Return rollback function
    Ok(Box::new(move |client| {
        client.set_query_data(&QueryKey::new("users"), |old: Option<Vec<User>>| {
            old.map(|mut users| {
                users.retain(|u| u.id != params.id);
                users
            })
        });
    }))
});

fn create_user(&mut self, cx: &mut ViewContext<Self>, params: CreateUserParams) {
    let client = self.query_client.clone();
    let mutation = self.create_user_mutation.clone();

    spawn_mutation(cx, &client, &mutation, params, move |this, state, cx| {
        match state {
            MutationState::Success(user) => {
                this.show_success("User created");
            }
            MutationState::Error(e) => {
                // Rollback happens automatically
                this.show_error(e.to_string());
            }
            _ => {}
        }
        cx.notify();
    });
}
```

### 5. Infinite Queries

```rust
use rs_query::{InfiniteQuery, spawn_infinite_query};

let projects_query = InfiniteQuery::new(
    QueryKey::new("projects"),
    |cursor| async move { fetch_projects(cursor).await },
    0, // initial_page_param
)
.get_next_page_param(|last_page, _pages| last_page.next_cursor)
.max_pages(3);

let observer = spawn_infinite_query(cx, &client, &query, |this, state, cx| {
    match state {
        QueryState::Success(data) => {
            this.projects = data.pages.into_iter().flatten().collect();
        }
        _ => {}
    }
    cx.notify();
});

// Later: observer.fetch_next_page();
```

## API Overview

### `QueryClient`

Central cache and query manager.

- `get_query_data<T>(key: &QueryKey) -> Option<T>`
- `set_query_data<T>(key: &QueryKey, data: T, options: QueryOptions)`
- `invalidate_queries(pattern: &QueryKey)` – marks matching entries as stale
- `refetch_all_stale()` – triggers refetch of all stale queries (e.g., on window focus)
- `dehydrate() -> DehydratedState` – serializes cache for persistence
- `hydrate(state: DehydratedState)` – restores cache from dehydrated state
- `clear()` – empties the entire cache
- `gc()` – removes entries older than their `gc_time`

### `Query<T>`

Definition of a query.

- `Query::new(key, fetch_fn)` – builder starts here
- `.stale_time(duration)` – how long data is considered fresh
- `.gc_time(duration)` – inactive cache lifetime
- `.retry(config)` – custom retry behaviour
- `.enabled(bool)` – conditionally disable the query
- `.initial_data(data)` / `.initial_data_fn(f)` – populate cache if empty
- `.placeholder_data(data)` / `.placeholder_data_fn(f)` – show while loading
- `.select(f)` – transform data before returning
- `.refetch_interval(duration)` – poll at interval
- `.refetch_on_window_focus(bool)` – refetch when window regains focus

### `InfiniteQuery<T, P>`

Definition of an infinite query.

- `InfiniteQuery::new(key, fetch_fn, initial_page_param)`
- `.get_next_page_param(f)` – function to extract next cursor
- `.get_previous_page_param(f)` – function to extract previous cursor
- `.max_pages(n)` – limit stored pages
- Supports same options as `Query<T>`

### `Mutation<T, P>`

Definition of a mutation.

- `Mutation::new(mutate_fn)` – builder start
- `.invalidates_key(key)` / `.invalidates(keys)` – keys to invalidate on success
- `.on_mutate(f)` – optimistic update with rollback

### `QueryObserver<T>`

Reactive observer for a query.

- `QueryObserver::new(client, key)`
- `.state() -> &QueryState<T>` – current state
- `.update()` – refresh state from cache
- `.refetch()` – manually trigger refetch

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
- `spawn_mutation(cx, client, mutation, params, callback)` – executes a mutation with optional optimistic updates.
- `spawn_infinite_query(cx, client, query, callback) -> InfiniteQueryObserver` – executes an infinite query.

### Hydration

- `client.dehydrate() -> DehydratedState` – serializes cache state.
- `client.hydrate(state, options)` – restores cache from dehydrated state.
- Enable `serde` feature for JSON serialization support.

### Devtools

Enable the `devtools` feature to use the `QueryDevtools` component:

```rust
#[cfg(feature = "devtools")]
let devtools = QueryDevtools::new(client.clone(), cx);
// Add to your window
```

The devtools panel shows active queries, a timeline of state changes, and cache inspection.

## Comparison with TanStack Query

rs‑query aims to provide the core experience of TanStack Query while respecting Rust’s ownership model and GPUI’s async patterns. Below is a summary of what is currently implemented.

| Feature                                   | Status          |
|-------------------------------------------|-----------------|
| Query cache & GC                          | ✅ Implemented  |
| Stale‑while‑revalidate                    | ✅ Implemented  |
| Automatic retries (exponential backoff)   | ✅ Implemented  |
| Request deduplication                     | ✅ Implemented  |
| Hierarchical key invalidation             | ✅ Implemented  |
| Query observers (reactive state)          | ✅ Implemented  |
| Optimistic updates with rollback          | ✅ Implemented  |
| Infinite queries / pagination             | ✅ Implemented  |
| `initialData` / `placeholderData`         | ✅ Implemented  |
| `select` transformation                   | ✅ Implemented  |
| `refetchInterval` / window focus refetch  | ✅ Implemented  |
| Hydration / persistence                   | ✅ Implemented  |
| Devtools                                  | ✅ Implemented  |
| Structural sharing                        | ⚠️ Placeholder  |
| Query cancellation                        | ✅ Implemented  |
| Suspense integration                      | N/A (GPUI)     |
| `useIsFetching` / `useIsMutating` helpers | ❌ Not yet     |
| Full query filters API                    | ❌ Not yet     |

## Contributing

Contributions are welcome! If you'd like to help improve rs-query, please open an issue or pull request on the [repository](https://github.com/elcoosp/rs-query). Focus areas include:

- Full structural sharing implementation
- `useIsFetching` / `useIsMutating` helpers
- Advanced query filters
- Framework adapters for other Rust UI libraries (Dioxus, Leptos, etc.)

## License

This project is licensed under the MIT License.

## Acknowledgements

This project is heavily inspired by [TanStack Query](https://tanstack.com/query) and built for the [GPUI](https://www.gpui.rs/) ecosystem. Thanks to the maintainers of both projects for their excellent work.
