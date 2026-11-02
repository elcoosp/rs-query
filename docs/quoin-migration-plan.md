# rs-query → quoin Migration Plan

> **Status:** implementation contract. Every section below is either already
> implemented (`[x]`) or explicitly rejected with a reason. Breaking changes
> are **allowed and expected**; there is **no backward compatibility**.
> Baseline audited at rs-query `ee416b8` (gpui-hardcoded, tokio-coupled) and
> quoin `e563c04` (core + 5 adapters + testkit + conformance + devtools).

---

## 0. Why this migration exists

rs-query is a TanStack Query-inspired async state manager whose **core**
(`client`, `observer`, `mutation*`, `persister*`, `hydration`, `broadcast`,
`filters`, `retryer`, managers, `streamed_query`, `updater`, …) was already
framework-free — but only in terms of *UI* frameworks. Its runtime layer was
hard-wired to **two** specific stacks:

1. **tokio** as the async runtime: ambient-runtime `tokio::spawn`,
   `tokio::sync::{broadcast, watch, mpsc}`, `tokio::select!`,
   `tokio::time::sleep`, `tokio_util::sync::CancellationToken`,
   `tokio::task::{AbortHandle, JoinHandle, yield_now}`.
2. **gpui** as the reactive layer: `spawn_query`/`spawn_mutation` took
   `&mut gpui::Context<V>` and delivered results via `WeakEntity::update`;
   `hooks::*` returned `gpui::Entity<T>` models fed by `AsyncApp` loops;
   `devtools` rendered a `gpui-component` panel.

Consequence: rs-query only runs where an ambient **tokio** runtime exists and
can only be observed from **gpui**. That is exactly the coupling quoin exists
to remove (quoin's ROADMAP M3.1 names rs-query as an intended adopter).

## 1. Target architecture

```
                ┌────────────────────────────────────────────────┐
                │  application crate (pick ONE framework)        │
                │  quoin = { features = ["gpui"|"leptos"|...] }  │
                └───────────────┬────────────────────────────────┘
                                │ C: ReactiveContext
                ┌───────────────▼────────────────────────────────┐
                │  rs-query (this crate)                         │
                │  • core: runtime-agnostic (no tokio, no gpui)  │
                │  • bindings: generic over C: ReactiveContext   │
                │  • QueryClient owns an Arc<dyn QueryRuntime>   │
                │    (spawn + sleep), built from C::Executor     │
                └───────────────┬────────────────────────────────┘
                                │ depends on
                ┌───────────────▼────────────────────────────────┐
                │  quoin-core (Signal / ReactiveContext /        │
                │  Executor+JoinHandle / CancellationToken /     │
                │  batch / pool / sleep)                         │
                └────────────────────────────────────────────────┘
```

Layering rules after the migration:

- `rs-query` core modules **must not** import `tokio` (production), `gpui`,
  or any framework adapter. The only allowed async substrate is
  `futures`-ecosystem primitives plus the `quoin_core` contracts.
- The **headless core stays non-generic**: `QueryClient`, `Query<T>`,
  `QueryObserver<T>`, executors and caches keep their current shapes so the
  crate remains ergonomic. Portability is achieved by owning a **runtime
  handle** (see §3), not by making every type generic over a context.
- The **binding layer** (`hooks.rs`, rewritten) is generic over
  `C: ReactiveContext` and returns `C::Signal<T>` values, following the exact
  pattern quoin's own `examples/fetch-core` establishes.

## 2. Framework coupling audit (baseline)

| Module | gpui usage | tokio usage | Action |
|---|---|---|---|
| `executor.rs` | `spawn_query`/`spawn_mutation` (`WeakEntity`, `AsyncApp`, foreground spawn) | `tokio::spawn` fetch task, `select!`, sleeps | rewrite (§6.1) |
| `hooks.rs` | 7 hooks returning `Entity<T>` with `AsyncApp` pump loops | `broadcast` receivers | rewrite as quoin bindings (§6.2) |
| `devtools.rs` | full `gpui-component` panel | (none, timeline wiring commented out) | rewrite headless (§6.3) |
| `infinite_executor.rs` | vestigial `_cx`/`_callback` params | own current-thread runtime on a private `std::thread`, `mpsc`, `select!` | rewrite spawn (§6.4) |
| `client.rs` | — | `broadcast`/`watch`, `tokio::spawn` (mount), `AbortHandle` | migrate (§6.5) |
| `mutation.rs` | — | `yield_now`+`sleep` scope gate, retry sleeps | migrate (§6.6) |
| `mutation_cache.rs` | — | `broadcast` events | migrate (§6.6) |
| `mutation_observer.rs` | — | `tokio::spawn` + `JoinHandle` | migrate (§6.6) |
| `queries_observer.rs` | — | `tokio::spawn` + `JoinHandle` | migrate (§6.6) |
| `retryer.rs` | — | `CancellationToken`, `TimeoutManager` sleep | migrate (§6.7) |
| `timeout_manager.rs` | — | `TokioSleepProvider` | re-provider (§6.7) |
| `network.rs` | — | `watch` channel | migrate (§6.8) |
| `cancellation.rs` | — | `select!` around `CancellationToken` | migrate (§6.8) |
| `broadcast.rs` (sync) | — | global `broadcast` registry, task + watch stop | migrate (§6.9) |
| `persist_client.rs` | — | 1 s save loop (`spawn`/`select!`/`sleep`), watch stop | migrate (§6.9) |
| `observer.rs` | — | `broadcast::Receiver` in `try_recv` | type swap only |
| `notify_manager.rs` | — | none | keep |
| everything else (`state`, `options`, `filters`, `key`, `error`, `query`, `sharing`, `streamed_query`, `updater`, `hydration`, `persister`, `focus_manager`) | — | — | keep, minor import fixes |

## 3. The runtime seam: `QueryRuntime`

rs-query keeps a single, **object-safe** runtime abstraction instead of
generifying the whole core:

```rust
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Spawn + timer capability rs-query needs from the host environment.
pub trait QueryRuntime: Send + Sync + 'static {
    /// Spawn a fire-and-forget task; abort at the next suspension point.
    fn spawn(&self, future: BoxFuture<()>) -> SpawnHandle;
    /// Sleep for `duration` (retry backoff, GC/persist/save intervals).
    fn sleep(&self, duration: Duration) -> BoxFuture<()>;
}

/// Awaitable, abortable handle to a spawned task
/// (`await → Result<(), Canceled>`, `abort()`, drop-cancels).
pub struct SpawnHandle { /* quoin SharedJoinHandle semantics */ }
```

Provided implementations:

| Runtime | spawn backed by | sleep backed by | Use case |
|---|---|---|---|
| `PoolRuntime` (default) | `quoin_core::pool` worker pool | `quoin_core::sleep` (futures-timer native / wasm timers) | no framework present |
| `ExecutorRuntime<E>` | any `quoin_core::Executor` | `Executor::sleep` (**added to quoin-core**, §4) | hosting inside a quoin app |

- `QueryClient::new()` keeps existing behavior but now uses `PoolRuntime`
  (no ambient tokio required anywhere).
- `QueryClient::with_runtime(Arc<dyn QueryRuntime>)` and
  `QueryClient::with_context<C: ReactiveContext>(&C)` (uses `C::Executor`)
  are the new constructors.
- All internal `tokio::spawn` sites (`spawn_query`, `MutationObserver::mutate`,
  `QueriesObserver::run_indices`, `QueryClient::mount`,
  `broadcast_query_client`, `persist_query_client`,
  `run_infinite_query_loop`) go through `client.runtime().spawn(...)`; all
  internal sleeps go through `client.runtime().sleep(...)`.
- `QueryClient::new()`'s GC sweeper keeps its `std::thread` (portable, no
  runtime needed).

## 4. Required quoin-core changes (all-frameworks)

Quoin is extended **only** with capabilities that are useful to every adapter,
never with rs-query-specific hooks:

1. **`Executor::sleep`** — timers are required by rs-query (retry backoff,
   intervals, GC/persist loops) and missing from the `Executor` contract
   (`GpuiExecutor::sleep` was an inherent method outside the trait).
   - `quoin-core`: provided method returning a boxed `Send` future, default
     implemented via a new `quoin_core::sleep(duration)` free fn
     (native: `futures-timer`; `wasm32`: `gloo-timers`).
   - Adapters may override (gpui scheduler timer, tokio for xilem); the
     quoin-testkit `TokioExecutor` overrides with `tokio::time::sleep` so
     paused-time tests (`start_paused` + `advance`) drive rs-query's backoff
     deterministically.
   - Conformance: new async check `executor_sleep_resolves` added to the
     shared suite so every adapter proves its timer capability.
2. **`LocalExecutor` for dioxus + floem adapters** — both adapters' docs
   already claimed support; implement it via the shared `spawn_local_task`
   (same as leptos/xilem) and add the conformance local-task check.
3. **Doc-drift fixes** — `ReactiveContext::use_global` GPUI paragraph updated
   to the shipped shared-cell semantics.
4. Version bump + CHANGELOG entries; no API removed from quoin.

Explicitly **rejected** quoin changes (with reasons):

- *Per-signal `subscribe`/effect primitive in core*: rs-query's eventing is
  an internal concern; a hand-rolled bounded event bus (§5) keeps the core
  surface small and is portable by construction.
- *Async-spanning batch*: rs-query never batches across `.await`; single
  logical mutations are already one notification each.
- *`spawn_on_main_thread`*: no universal main-thread concept exists across
  the 5 frameworks; quoin's established publish-from-task contract (see
  `Resource`/`fetch-core`) is the documented portability model.

## 5. Runtime primitives replacing tokio (inside rs-query)

New private modules, zero tokio:

- **`bus.rs`** — bounded async broadcast channel mirroring the exact
  `tokio::sync::broadcast` semantics rs-query relies on
  (`Sender::send` sync + fan-out, `Receiver::recv().await`,
  `try_recv()`, `RecvError::{Closed, Lagged(n)}`, lag → drop-oldest so slow
  subscribers self-heal by re-reading the cache). Backed by
  `Mutex<VecDeque>` + waker lists (no locks held across `.await`).
  Replaces: per-key subscriber channels (cap 16), activity channel (cap 16),
  `MutationCacheEvent` channel (cap 256), cross-instance broadcast registry
  (cap 64).
- **`watch.rs`** — minimal watch cell (`send`/`borrow`/`subscribe` with
  versioned change detection, async `changed()`), replacing
  `tokio::sync::watch` (online manager, mount stop-signal, persist/broadcast
  stop-signals, scope-release wakeups in §6.6).
- **`cancellation.rs`** — `CancellationToken` now re-exported from
  `quoin_core`; `cancellable_fetch` becomes
  `token.run_until_cancelled(fetch).await` (`None` → `QueryError::cancelled()`).
- **`error.rs`** — drop `From<tokio::task::JoinError>`; the spawn-handle
  cancelled case maps directly to `QueryError::cancelled()`.

## 6. Module migration contract

### 6.1 `executor.rs`
- `execute_query` / `execute_mutation` signatures unchanged; internals use
  `client.runtime().sleep(...)` and the new bus/watch/cancellation.
- `spawn_query(cx, client, query, callback)` → **`spawn_query(client, query)
  -> SpawnedQuery<T>`** (`client`-owned runtime): registers in-flight, spawns
  the fetch via `runtime.spawn`, resolves with the final `QueryState<T>`,
  honours `is_restoring` (resolves `Idle` without fetching). The
  view-callback concept is **removed** — observation is now the domain of the
  binding layer (§6.2) and the awaited handle.
- `spawn_mutation(cx, client, mutation, params, callback)` →
  **`spawn_mutation(client, mutation, params) -> SpawnedMutation<T>`**.
- `SpawnedQuery<T>`/`SpawnedMutation<T>`: `await → QueryState<T>` /
  `MutationState<T>`, `abort()`, `is_finished()`; abort → cancelled error
  state.

### 6.2 `hooks.rs` (bindings, generic over quoin)
Replaces gpui entities with quoin signals; all follow the `fetch-core`
pattern (`C::Signal<…>: Send`, pumps spawned on `cx.executor()`):

- `use_query<C, T>(cx, client, query) -> UseQuery<C::Signal<QueryState<T>>, T>`
  — seeds the signal from the cache, kicks execution, keeps the signal in
  sync via the per-key bus (lag-tolerant re-reads), exposes `refetch()` and
  cancels its pump when dropped.
- `use_mutation<C, T, P>(cx, client, mutation) -> UseMutation<…>` with
  `mutate(params) -> SpawnedMutation<T>` and a live `state` signal.
- `use_infinite_query<C, T, P>(cx, client, query) -> UseInfiniteQuery<…>`
  (state signal + `fetch_next_page`/`fetch_previous_page`).
- `use_queries<C, T>(cx, client, specs) -> C::Signal<Vec<QueryState<T>>>`
  (per-spec pump, `set_eq`-gated writes).
- `use_is_fetching` / `use_is_mutating` / `use_fetching_count` /
  `use_mutating_count` / `use_any_fetching` / `use_activity_feed` — same
  semantics, `C::Signal`-typed.
- `bind_focus(client, focused)` helper retained for focus wiring.
- Lifecycle: pumps hold a stop token; `Drop` of the binding cancels the pump
  (replaces "view dropped → weak update fails").

### 6.3 `devtools.rs` (headless)
- The `gpui-component` panel is removed with the gpui dependency.
- New headless devtools behind the `devtools` feature:
  `DevtoolsState { client, timeline ring buffer, filters }` with
  `cache_snapshot()`, `record_from_client()` (background collector on the
  client runtime), `refetch/invalidate/remove` actions, `clear_timeline()`,
  timeline search/type filters — everything the panel displayed, exposed as
  data + actions any framework UI (or the quoin-devtools `EventLog`) can
  render. Timeline collection is **wired in production** now (the old
  commented-out poll is replaced by a real subscriber loop).

### 6.4 `infinite_executor.rs`
- `spawn_infinite_query(client, query) -> InfiniteQueryObserver<T, P>` (no
  cx, no callback): the page-request loop runs as one task on
  `client.runtime()`; `futures_channel::mpsc` for next/prev requests; dummy
  in-flight markers replaced by real task abort handles.

### 6.5 `client.rs`
- Subscribers/activity channels → `bus.rs`; online/stop watches → `watch.rs`.
- `InFlightTask { abort: Box<dyn Fn() + Send + Sync>, cancel_token }`
  (runtime-agnostic abort closure).
- `mount()`/`unmount()` watcher spawned via `runtime.spawn`; `watch.rs` stop
  signal; no "requires a running tokio runtime" caveat.
- `QueryClient::with_runtime` / `with_context` added; `new()` documented as
  `PoolRuntime`-backed.

### 6.6 Mutation stack
- `mutation_cache.rs`: event channel → `bus.rs` (cap 256); scope release now
  wakes paused mutations through `watch.rs` (polling `yield_now`+10 ms sleep
  loop deleted).
- `mutation.rs`: retry sleeps → `runtime.sleep`; scope gate parks on the
  scope watch; `execute_mutation_full*` signatures unchanged.
- `mutation_observer.rs`: `mutate` → `runtime.spawn` + oneshot;
  `tokio::task::JoinHandle<Result<T, QueryError>>` → `SpawnedMutation<T>`.

### 6.7 Retry & timers
- `timeout_manager.rs`: `TokioSleepProvider` → `QuoinSleepProvider`
  (`quoin_core::sleep`); `SleepProvider` trait and injectable-timer story
  unchanged. `TimeoutManager::default()` keeps working with **no runtime**.
- `retryer.rs`: token swap only; paused-time tests now use the testkit tokio
  runtime.

### 6.8 Network & cancellation
- `network.rs`: `OnlineManager::subscribe` → `watch.rs::Watcher<bool>`.
- `cancellation.rs`: `cancellable_fetch` via `run_until_cancelled`.

### 6.9 Sync & persistence
- `broadcast.rs`: registry stores `bus::Sender<PersistedClient>`; receive
  loop + stop flag on `runtime.spawn` + `watch.rs`.
- `persist_client.rs`: save loop `runtime.spawn` + `runtime.sleep(interval)`
  + `watch.rs` stop.

## 7. Public API surface changes (breaking)

| Before (gpui/tokio) | After (quoin) |
|---|---|
| `spawn_query(&mut gpui::Context<V>, …, callback)` | `spawn_query(&QueryClient, &Query<T>) -> SpawnedQuery<T>` |
| `spawn_mutation(&mut gpui::Context<V>, …, callback)` | `spawn_mutation(&QueryClient, &Mutation<T,P>, P) -> SpawnedMutation<T>` |
| `spawn_infinite_query(&mut gpui::Context<V>, …, _cb)` | `spawn_infinite_query(&QueryClient, &InfiniteQuery<T,P>) -> InfiniteQueryObserver<T,P>` |
| `hooks::use_*(&mut Context<V>, …) -> gpui::Entity<_>` | `hooks::use_*(cx: &C, …) -> C::Signal<_>` / binding structs |
| `tokio_util::sync::CancellationToken` | `quoin_core::CancellationToken` (re-exported) |
| `tokio::sync::broadcast::Receiver<…>` from `subscribe*()` | `bus::Receiver<…>` (`recv`/`try_recv`/`Stream`) |
| `OnlineManager::subscribe() -> tokio watch::Receiver` | `watch::Watcher<bool>` |
| `MutationObserver::mutate -> tokio JoinHandle` | `-> SpawnedMutation<T>` |
| `PersistQueryClientHandle::into_task -> tokio JoinHandle` | `-> SpawnHandle` |
| `BroadcastHandle::into_task -> tokio JoinHandle` | `-> SpawnHandle` |
| `TimeoutManager::default()` (tokio) | `QuoinSleepProvider` default (runtime-free) |
| `devtools` (gpui-component panel) | `devtools` (headless state + collector) |
| Cargo: `gpui`, `gpui-component`, `tokio`, `tokio-util`, `futures-timer` | Cargo: `quoin-core`, `futures`; `tokio` only as dev-dep via `quoin-testkit` |

Feature flags: `serde` unchanged; `devtools` no longer pulls `gpui-component`
/ `chrono` (uses std-time ring buffer; quoin-devtools glue is optional).
Deleted: nothing else — every TanStack-parity feature is preserved.

## 8. Test strategy

- **Core tests keep `#[tokio::test]`** — tokio moves to `[dev-dependencies]`
  purely as the deterministic test runtime:
  - `rs-query::test_support` (cfg(test)) exposes `client()` →
    `QueryClient::with_runtime(tokio-backed runtime)` and `token()`; paused
    time tests (`start_paused = true` + `advance`) keep working because the
    testkit tokio executor's `sleep` is tokio-time based.
  - Plain `#[test]`s use `QueryClient::new()` (`PoolRuntime`) — no runtime
    required.
- **Binding tests** run on `quoin-testkit`: `TestContext<ThreadedExecutor>`
  for pump loops, `ImmediateExecutor` for pure signal logic; update churn is
  asserted with `UpdateRecorder`.
- **All 41 `#[gpui::test]` tests are ported**: executor spawn tests become
  awaited-handle tokio tests; hook tests become testkit signal tests;
  devtools tests become headless collector tests; the single infinite-executor
  gpui test becomes a tokio test. gpui leaves the dev-deps entirely.
- **Gates (all must be green before delivery):**
  1. `cargo fmt --check`
  2. `cargo clippy --all-features --all-targets -- -D warnings`
  3. `cargo test --all-features` (unit + doc tests)
  4. quoin side: `cargo clippy -p quoin-core -p quoin-testkit -p quoin -p quoin-conformance … -- -D warnings` + their test suites (headless crates; adapter crates compile-check where the sandbox allows).

## 9. Packaging

- rs-query depends on `quoin-core` via a path dependency
  (`../quoin/quoin-core`) with a documented sibling-checkout layout — the two
  repos are delivered as zips that extract side by side.
- README rewritten (quoin quick-start, framework matrix, devtools guide);
  CHANGELOG entries added for both repos; this plan ships inside
  `rs-query/docs/`.

## 10. Feature-preservation checklist (must all survive)

- [x] dedup, retry (TanStack backoff), network gating/pause, cancellation
      (cooperative + hard abort), in-flight registry
- [x] per-key + activity + mutation-cache eventing with lag tolerance
- [x] filters/invalidate/refetch/cancel/reset/remove matrix
- [x] structural sharing, select, placeholder data, updater, streamed query
- [x] infinite queries (bidirectional pages, max_pages, direction flags)
- [x] mutation lifecycle ordering, scope serialization, per-call callbacks
- [x] hydration (newer-data guard, redaction), persister, whole-client
      persistence + retry, cross-instance broadcast sync
- [x] mount/unmount reconnect watcher, focus + online managers
- [x] injectable timers (`SleepProvider`) + NotifyManager batch primitives
- [x] devtools data surface (cache rows, timeline, actions)
- [x] `serde` feature surface
- Removed on purpose: gpui view-callback delivery (superseded by signals +
  awaited handles), gpui-component devtools panel (superseded by headless
  devtools renderable by any framework).
