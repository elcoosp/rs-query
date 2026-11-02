<div align="center">
  <img src="docs/logo.png" alt="rs-query" width="300"/>
  <h1>rs-query</h1>
  <p><strong>TanStack Query v5 parity for Rust — portable to every quoin-supported UI framework.</strong></p>
  <p>
    <a href="https://github.com/elcoosp/rs-query/actions/workflows/ci.yml"><img src="https://img.shields.io/badge/CI-Linux%20%7C%20macOS%20%7C%20Windows-9D4EDD?style=flat-square&logo=github" alt="CI"/></a>
    <a href="https://github.com/elcoosp/rs-query/blob/main/LICENSE"><img src="https://img.shields.io/badge/License-MIT-blue?style=flat-square" alt="License"/></a>
    <a href="https://crates.io/crates/rs-query"><img src="https://img.shields.io/badge/Rust-stable%20%7C%20edition%202024-000000?style=flat-square&logo=rust" alt="Rust"/></a>
    <a href="#observers--quoin-bindings"><img src="https://img.shields.io/badge/Frameworks-gpui%20%7C%20leptos%20%7C%20dioxus%20%7C%20xilem%20%7C%20floem-9D4EDD?style=flat-square" alt="Frameworks"/></a>
    <a href="#testing--quality-gates"><img src="https://img.shields.io/badge/Tests-713%20Rust-brightgreen?style=flat-square" alt="Tests"/></a>
  </p>
  <p>
    <a href="#installation">Install</a> ·
    <a href="#feature-overview">Features</a> ·
    <a href="#architecture">Architecture</a> ·
    <a href="#quick-start">Quick Start</a> ·
    <a href="#testing--quality-gates">Testing</a> ·
    <a href="#license">License</a>
  </p>
</div>

<br/>

**rs-query** is a Rust port of [TanStack Query](https://tanstack.com/query) built on
[quoin](https://github.com/elcoosp/quoin), the framework-portable reactive abstraction
layer. It gives applications declarative, cache-aware async state management — queries and
mutations with retries, deduplication, cancellation, invalidation, offline pause/resume,
persistence, hydration and reactive observers — **without hardcoding any UI framework**.
The same crate runs headless (shared worker pool + portable timers, no runtime needed) or
inside a quoin application on **gpui, leptos, dioxus, xilem or floem**.

This crate targets **TanStack Query v5 parity** — the porting contract lives in
[`docs/port-plan.md`](docs/port-plan.md), the quoin migration contract in
[`docs/quoin-migration-plan.md`](docs/quoin-migration-plan.md), and the audited upstream
semantics behind every module in [`docs/research/`](docs/research/).

| | |
|---|---|
| CI | GitHub Actions: `cargo fmt`, `clippy --all-features -- -D warnings` (pedantic, zero `#[allow]`), tests on default + all features, `cargo-llvm-cov` coverage |
| Tests | 713 tests (`cargo test --all-features`), framework-free |
| Runtime | no tokio, no gpui — quoin-core only; `futures` + portable timers |
| Edition | Rust 2024, `clippy::pedantic` clean |

## Feature overview

### Core query engine
- **Declarative queries** — `Query::new(key, fetch_fn)` with a `QueryKey` hierarchy and
  stable `cache_key()` hashing; `initial_data`, `placeholder_data` (incl.
  `keep_previous_data`-style placeholders) and `select` transformations.
- **Caching** — stale-while-revalidate cache with `staleTime` / `staleTime: 'static'`,
  `gcTime`, freshness by time, manual invalidation, `structural_sharing`
  (`Arc` reuse when data is equal, `Arc::ptr_eq` change detection in views).
- **Retries** — TanStack-exact backoff `min(1000 · 2^n, 30 s)` (no jitter), `RetryDecision`
  (never / count / always / closure), `failure_count` + `failure_reason` surfaced in
  observer flags.
- **Deduplication & cancellation** — concurrent fetches for a key coalesce; cooperative
  cancellation via `CancellationToken` plus tokio abort handles; `cancelQueries`-style
  revert semantics (`QueryError::Cancelled { revert, silent }`).
- **Invalidation with filters** — `QueryFilters` (key prefix/exact, active/inactive, stale,
  status, fetch status, predicates), `invalidate_queries_filtered` with TanStack's
  `refetchType` matrix (`all | active | inactive | none`), `refetch_queries`,
  `cancel_queries`, `reset_queries`, `remove_queries`.
- **Imperative API** — `fetch_query` (retry-forced-off, TanStack `query()` semantics),
  `prefetch_query` (fire-and-forget), `ensure_query_data` (fresh data short-circuit).
- **Network modes** — `NetworkMode::Online | Always | OfflineFirst`, `OnlineManager`
  with watch-based connectivity, offline pause/resume, refetch-on-reconnect triggers,
  `mount()`/`unmount()` lifecycle.
- **Managers** — `NotifyManager` (batch collapsing, nested batches), injectable
  `TimeoutManager` (deterministic tests), `FocusManager`.

### Mutations
- `Mutation<T, P>` with `on_mutate` (+ rollback), `on_success`, `on_error`, `on_settled`,
  `retry` (default 0), `mutation_key`, `scope { id }` serialization (scoped mutations run
  one at a time), `network_mode`, invalidation of query keys.
- **`MutationCache`** — filter-aware `find`/`find_all`, event broadcast
  (added/removed/updated/scope-resumed), cache-level callbacks, `resume_paused_mutations`.
- TanStack-exact side-effect ordering: cache `on_mutate` → options `on_mutate` → pending
  dispatch → mutation fn → cache/options `on_success`/`on_error` → per-call `MutateOptions`
  (fire last) → settle chain.

### Observers & quoin bindings
- `QueryObserver<T>` with `QueryResultFlags` — TanStack `QueryObserverResult` parity:
  `data_updated_at`, `error_updated_at`, `error_update_count`, `failure_count`,
  `failure_reason`, `is_fetched`, `is_fetched_after_mount`, `is_loading_error`,
  `is_refetch_error`, `is_placeholder_data`, `is_paused`, `is_enabled`.
- `spawn_query(client, query)` / `spawn_mutation(client, mutation, params)` return
  awaitable, abortable handles on the client's runtime, with an `is_restoring` gate for
  hydration-driven apps.
- **Bindings generic over any `quoin_core::ReactiveContext`** (the `useQuery` /
  `useMutation` / `useInfiniteQuery` / `useQueries` analogues): `use_query`,
  `use_mutation`, `use_infinite_query`, `use_queries`, plus `use_is_fetching`,
  `use_is_mutating`, `use_fetching_count`, `use_mutating_count`, `use_any_fetching`,
  `use_activity_feed` — every binding returns the framework's own `Signal` and stays in
  sync through lag-tolerant cache pumps.
- `QueriesObserver` with TanStack `combine` support; `InfiniteQuery` with bidirectional
  pagination, `max_pages`, `has_next_page` / `has_previous_page`, direction-tracked
  `is_fetching_next_page` / `is_fetching_previous_page`.
- **Devtools** (feature-gated) — headless state + collector: cache rows, a timestamped
  transition timeline with filters/search, and cache actions, renderable by any framework
  (or plumbed into quoin-devtools' `EventLog`).

### Persistence & hydration
- `dehydrate` / `hydrate` with TanStack semantics: `dehydrated_at`, error redaction
  (`should_redact_errors`), `should_dehydrate_query` / `should_dehydrate_mutation`
  predicates, newer-data guard on hydrate, `fetchStatus` reset, default options.
- Fine-grained **query persister** (`experimental_createQueryPersister` port):
  `QueryStorage` trait + in-memory impl, `max_age` expiry, `buster` versioning,
  GC, selective restore.
- Whole-client persistence: `ClientPersister`, `persist_query_client_restore/save/subscribe`,
  `remove_oldest_query` retry strategy.
- **Broadcast sync** — `broadcast_query_client` keeps multiple client instances in sync
  with a re-entrancy guard (remote updates are never re-broadcast).

### Utilities
- `streamed_query` with `RefetchMode::{Replace, Append, Reset}`.
- `Updater<T>` value-or-function updates (`functional_update`).
- `cancellable_fetch` helper.

## Installation

rs-query depends on `quoin-core`. Until quoin is published to crates.io, check out
[quoin](https://github.com/elcoosp/quoin) next to rs-query (sibling directory) and use
path dependencies — they resolve to `../quoin/quoin-core` and `../quoin/quoin-testkit`:

```toml
[dependencies]
rs-query = { path = "../rs-query" }

# Optional feature gates:
#   serde     — JSON persistence/hydration of query data
#   devtools  — headless devtools state + collector
```

There is **no runtime requirement**: rs-query spawns on quoin's shared worker pool with
portable timers by default. To host it inside a quoin app, build the client from the
framework context (see the quick start) so fetches and timers use the framework's own
executor.

## Quick start

### 1. Create a `QueryClient`

Hold a `QueryClient` in your application state; it is cheap to clone and shares the
cache across clones.

```rust
use rs_query::QueryClient;

struct AppState {
    query_client: QueryClient,
}
```

### 2. Run a query (headless)

```rust
use std::time::Duration;
use rs_query::{Query, QueryClient, QueryKey, spawn_query};

let client = QueryClient::new();
let key = QueryKey::new("users").with("id", 42);

let query = Query::new(key.clone(), || async { Ok(vec!["Ada", "Alan"]) })
    .stale_time(Duration::from_secs(30));

let handle = spawn_query(&client, &query);
let state = handle.await; // awaitable + abortable (drop aborts the fetch)
```

### 2b. Observe from a quoin view (framework-agnostic)

```rust,ignore
use quoin::ReactiveContext;

// Inside your app: the client rides the framework's executor + timer.
let client = QueryClient::with_context(&cx);

// Inside a component: binding.state is the framework's own signal.
let todo = rs_query::use_query(&cx, &client, &query);
// render reads: todo.state.get()  →  QueryState<Todo>
```

### 3. Invalidate with filters

```rust
use rs_query::{QueryFilters, InvalidateQueryFilters, RefetchType};

// Mark every "users" query stale; refetch only the active ones (TanStack default).
client.invalidate_queries_filtered(
    InvalidateQueryFilters::new()
        .filters(QueryFilters::with_key(QueryKey::new("users")))
        .refetch_type(RefetchType::Active),
);
```

### 4. Mutations with optimistic updates

```rust
use rs_query::{Mutation, MutationScope, execute_mutation_full, MutationCache};

let cache = MutationCache::default();
let mutation = Mutation::<User, UserPatch>::new(|patch| async move {
    save_user(patch).await
})
.mutation_key(QueryKey::new("save-user"))
.scope(MutationScope { id: "save-user".into() }) // serialize concurrent saves
.on_mutate(|client, patch| {
    // optimistic write; return a rollback closure
    let previous = client.get_query_data::<User>(&user_key(patch.id));
    client.set_query_data(&user_key(patch.id), patch.preview(), QueryOptions::default());
    Ok(Box::new(move |client| {
        if let Some(previous) = previous {
            client.set_query_data(&user_key(patch.id), previous, QueryOptions::default());
        }
    }))
})
.invalidates_key(user_key(42));
```

### 5. Offline-aware fetching

```rust
use rs_query::{NetworkMode, OnlineManager, QueryOptions};

let online = OnlineManager::new(); // starts online
let options = QueryOptions::default()
    .network_mode(NetworkMode::OfflineFirst) // attempt 1 runs offline, retries pause
    .refetch_on_reconnect(rs_query::RefetchTrigger::Always);

// Connectivity callbacks drive pause/resume:
online.set_online(false); // in-flight retries pause; resume happens on set_online(true)
```

### 6. Persistence

```rust
use std::sync::Arc;
use rs_query::{InMemoryQueryStorage, QueryPersister, QueryPersisterOptions,
               persist_query_client, PersistQueryClientOptions, InMemoryClientPersister};

// Per-query persistence
let persister = QueryPersister::new(
    QueryPersisterOptions::new(Arc::new(InMemoryQueryStorage::new()))
        .with_buster("v1".into()),
);

// Whole-client persistence (restore + throttled save loop)
let handle = persist_query_client(
    &client,
    &PersistQueryClientOptions::new(Arc::new(InMemoryClientPersister::new())),
).await?;
```

## Architecture

```text
src/
├── lib.rs               public re-exports
├── key.rs               QueryKey hierarchy + prefix matching
├── state.rs             QueryState/MutationState + status enums
├── options.rs           QueryOptions (TanStack option surface)
├── filters.rs           QueryFilters / MutationFilters + match_query/match_mutation
├── retryer.rs           retry decisions + min(1000·2^n, 30 s) backoff
├── network.rs           NetworkMode + OnlineManager
├── notify_manager.rs    batched notifications
├── timeout_manager.rs   injectable timer source
├── query.rs             Query<T> builder
├── client.rs            QueryClient: cache, dedup, cancellation, bulk operations
├── executor.rs          fetch/mutation execution (retry, pause, restore gate)
├── observer.rs          QueryObserver + result flags
├── infinite.rs          InfiniteData / InfiniteQuery
├── infinite_executor.rs paginated fetch loop (max_pages, direction)
├── mutation.rs          Mutation<T, P> + full lifecycle
├── mutation_cache.rs    MutationCache + scope serialization
├── mutation_observer.rs MutationObserver
├── queries_observer.rs  QueriesObserver (useQueries core)
├── runtime.rs           QueryRuntime seam (PoolRuntime / ExecutorRuntime)
├── bus.rs               bounded broadcast channel (runtime-free)
├── watch.rs             watch cell (online/stop signals)
├── hooks.rs             quoin signal bindings (framework-agnostic)
├── sharing.rs           structural sharing (PartialEqAny)
├── cancellation.rs      cancellable_fetch
├── hydration.rs         dehydrate/hydrate with TanStack semantics
├── persister.rs         per-query persistence
├── persist_client.rs    whole-client persistence
├── broadcast.rs         cross-instance cache sync
├── streamed_query.rs    streamedQuery port
└── devtools.rs          headless devtools state + collector (feature "devtools")
```

## Testing & quality gates

```bash
cargo test --all-features          # 713 tests, framework-free
cargo clippy --all-features --all-targets -- -D warnings   # pedantic, zero allows
cargo doc --no-deps                # every public item documented
```

CI (`.github/workflows/ci.yml`) runs formatting, clippy (pedantic, warnings-as-errors),
tests on default + all features, and coverage uploaded to Codecov. Releases
(`.github/workflows/release.yml`) verify tests, regenerate `CHANGELOG.md` with
[git-cliff](https://git-cliff.org) from Conventional Commits (`cliff.toml`) and publish
GitHub release notes on `v*` tags.

## Versioning & changelog

Releases follow SemVer; `CHANGELOG.md` is generated from Conventional Commits by
git-cliff on each `v*` tag (see `cliff.toml`).

## License

MIT — see the repository metadata.
