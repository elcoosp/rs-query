# rs-query — TanStack Query 100% Parity Port Plan

> **Status of source audit:** TanStack Query `@tanstack/query-core` **v5.105.0**, audited
> file-by-file from the official repository (`TanStack/query`, packages `query-core`,
> `react-query`, `query-persist-client-core`, `query-sync-storage-persister`,
> `query-async-storage-persister`, `query-broadcast-client-experiment`).
> Raw evidence: [`research/01-query-core.md`](research/01-query-core.md) (public API inventory),
> [`research/02-framework-and-persistence.md`](research/02-framework-and-persistence.md)
> (framework hooks + persistence), [`research/03-behavioral-spec.md`](research/03-behavioral-spec.md)
> (exact behavioral semantics). This document is the **implementation contract**: everything
> below must exist in rs-query and be covered by tests.

---

## 1. Target architecture mapping

TanStack Query is a framework-agnostic core (`query-core`) plus thin framework adapters
(`react-query`). rs-query is the Rust/GPUI equivalent of the **whole stack**:

| TanStack (JS)             | rs-query (Rust/GPUI)                                   |
|---------------------------|--------------------------------------------------------|
| `QueryClient`             | `QueryClient` (Clone, shared handle)                    |
| `QueryCache`              | `QueryCache` (event-emitting store inside the client)   |
| `MutationCache`           | `MutationCache` (scope-aware store inside the client)   |
| `Query` / `Mutation`      | `Query<T>` / `Mutation<T, P>`                           |
| `QueryObserver`           | `QueryObserver<T>` + `QueryObserverHandle` (gpui Model) |
| `InfiniteQueryObserver`   | `InfiniteQuery<T, P>` + `spawn_infinite_query`          |
| `MutationObserver`        | `MutationObserver<T, P>` (new)                          |
| `QueriesObserver`         | `QueriesObserver<T>` (new)                              |
| `useQuery`                | `use_query` / `spawn_query`                             |
| `useInfiniteQuery`        | `use_infinite_query`                                    |
| `useMutation`             | `use_mutation` / `spawn_mutation`                       |
| `useQueries`              | `use_queries` (new)                                     |
| `useIsFetching(filters)`  | `use_is_fetching` (+ filter variants)                   |
| `useIsMutating(filters)`  | `use_is_mutating` (+ filter variants)                   |
| `useMutationState`        | `use_mutation_state` (new)                              |
| suspense/prefetch family  | `use_suspense_query`, `use_prefetch_query`, … (new)     |
| `focusManager`            | `FocusManager`                                          |
| `onlineManager`           | `OnlineManager` (new)                                   |
| `notifyManager`           | `NotifyManager` (new)                                   |
| `timeoutManager`          | `TimeoutManager` (new, injectable timers)               |
| `dehydrate` / `hydrate`   | `client.dehydrate()` / `client.hydrate()`               |
| `persistQueryClient*`     | `persist_client.rs` (new)                               |
| `experimental_createQueryPersister` | `persister.rs` (new)                          |
| `broadcastQueryClient`    | `broadcast.rs` (new)                                    |
| `streamedQuery`           | `streamed_query.rs` (new)                               |
| `skipToken`               | `QueryFn::Skip` / `SKIP` sentinel (new)                 |
| `QueryFilters`/`matchQuery` | `filters.rs` (new)                                    |

Everything that is React-renderer-specific (render-blocking suspense, `<HydrationBoundary>`
lifecycle) is ported as the GPUI-idiomatic equivalent (awaitable data handles, init-time
hydration); the underlying **semantics** are identical.

## 2. Current state of rs-query (audit at start of the port)

Already present and kept: `QueryKey` (string segments), `QueryClient` (DashMap cache +
broadcast subscribers + activity counters + in-flight cancellation), `Query<T>` builder,
`QueryObserver<T>` (`new`/`from_query`/`with_options`), `spawn_query`/`spawn_mutation`
(retry loop, dedup, invalidation), `InfiniteQuery`/`spawn_infinite_query`,
`Mutation<T, P>` (on_mutate/on_success/on_error/invalidates), `dehydrate`/`hydrate`
(basic), `FocusManager`, `structural sharing` (`replace_equal_deep`),
`PlaceholderData`, `RetryConfig`, devtools, GPUI hooks for fetching/mutating counts.

Gaps found (this port closes all of them):

1. **No filter model** — `invalidate_queries(pattern)` only; no `QueryFilters`,
   `MutationFilters`, `match_query`/`match_mutation`, `refetch_type`.
2. **No network-mode layer** — offline/paused states, `NetworkMode`, `OnlineManager`,
   `resume_paused_mutations`, refetch-on-reconnect.
3. **Retry semantics diverge** — rs-query uses `base_delay` 500 ms; TanStack uses
   `min(1000·2^failures, 30_000)` ms, `retry: 3` default (0 for mutations/imperative
   fetches), `retryOnMount`, `failure_count`/`failure_reason` surfaced in results.
4. **No managers** — `NotifyManager` (batched notifications), `TimeoutManager`
   (injectable timers), `OnlineManager`.
5. **Cache is a bare DashMap** — no `QueryCache`/`MutationCache` event model
   (`added/removed/updated/observerAdded/…`), no `find`/`find_all` with filters, no
   cache-level callbacks (`on_success`/`on_error`/`on_settled`), no GC events.
6. **Observer result is a bare `QueryState`** — missing TanStack result flags
   (`is_pending`, `is_fetching`, `data_updated_at`, `error_updated_at`,
   `error_update_count`, `failure_count`, `failure_reason`, `is_fetched`,
   `is_fetched_after_mount`, `is_loading_error`, `is_refetch_error`,
   `is_placeholder_data`, `is_paused`, `is_enabled`, `is_stale`, `is_refetching`,
   `is_loading`), tracked-props notification, `select` memoization, placeholder
   resolution inside the observer, `refetch({cancel_refetch, throw_on_error})`.
7. **Mutation layer incomplete** — no `on_settled`, no per-call callbacks
   (`mutate(vars, {onSuccess, onError, onSettled})`), no `retry` (default 0), no
   `scope {id}` serialization, no paused-mutation queue, no `MutationCache`, no
   `MutationObserver`, no `use_mutation_state`, `submitted_at`/`context`/`variables`
   not exposed, no mutation GC.
8. **Hydration is lossy** — no `dehydrated_at`, no error redaction, no
   `should_dehydrate_query/mutation` predicates, no newer-data guard on hydrate, no
   fetchStatus reset, no pending→success promotion, no serialization callbacks.
9. **No persistence** — nothing like `persistQueryClient*`, no `Persister` trait, no
   fine-grained query persister, no `remove_oldest_query`, no broadcast sync.
10. **No `use_queries`** (dynamic multi-query observer with `combine`).
11. **Client method surface incomplete** — no `fetch_query`/`prefetch_query`/
    `ensure_query_data`/`query()`, no `cancel_queries` (revert), no `reset_queries`,
    no `remove_queries`, no `get_query_state`, no `get/set_default_options`, no
    query/mutation defaults registration, no `get_queries_data`/`set_queries_data`,
    no `clear`, no `mount`/`unmount` semantics, no `is_fetching`/`is_mutating` filter
    variants.
12. **Options surface incomplete** — no `network_mode`, `retry_on_mount`,
    `refetch_on_reconnect`, `refetch_interval` fn form / background flag,
    `notify_on_change_props`, `meta`, `query_key_hash_fn`, `persister` hook,
    `subscribed`, `throw_on_error`, `initial_data_updated_at`, `enabled`/`stale_time`
    fn forms, `max_pages` on client-level infinite fetch, `skip_token`.
13. **Infinite query gaps** — no `fetch_next_page`/`fetch_previous_page` options
    (`cancel_refetch`, `throw_on_error`), no `is_fetching_next/previous_page`,
    no `is_fetch_next_page_error`, no bidirectional max_pages trimming parity,
    no refetch-replays-pageParams semantics.
14. **Structural sharing** — no depth cap (TanStack: 500), no `structuralSharing` fn form.
15. **No `streamedQuery`**, no `QueryErrorResetBoundary` equivalent, no
    `is_restoring` gate, no `updater` (`Updater<T>` fn-or-value) unification.
16. **`hash_key` parity** — key hashing must sort plain-object segments alphabetically
    (JSON.stringify parity) for cross-implementation stable hashes.

## 3. Implementation plan (module → contract)

New modules (all `pub` unless noted; every public item documented + tested):

### 3.1 `filters.rs`
```rust
pub struct QueryFilters { exact: bool, stale: bool, fetch_status: Option<FetchStatus>,
    status: Option<QueryStatus>, predicate: Option<Predicate>, query_key: Option<QueryKey>,
    type_: QueryType /* all|active|inactive */ }
pub struct InvalidateQueryFilters { refetch_type: RefetchType /* all|active|inactive|none */ }
pub struct RefetchQueryFilters /* QueryFilters + refetch_type */
pub struct MutationFilters { exact: bool, status: Option<MutationStatus>,
    fetch_status: Option<FetchStatus>, predicate: Option<MutationPredicate>,
    mutation_key: Option<QueryKey> }
pub fn match_query(entry: &QueryCacheEntry, filters: &QueryFilters) -> bool
pub fn match_mutation(entry: &MutationCacheEntry, filters: &MutationFilters) -> bool
```
Semantics: key match = prefix unless `exact`; `active` = has observers and enabled;
`stale` per `is_stale()`; predicate overrides.

### 3.2 `retryer.rs` (behavioral core, used by query & mutation executors)
- Backoff: `delay(k) = min(1000 * 2^k, 30_000)` ms — **no jitter**, k = 0-based failures
  so far; first retry waits exactly 1000 ms.
- `RetryDecision`: `bool | u32(count) | closure(error, attempt) -> bool`.
- `Retryer` drives: attempt loop, pause (offline / hidden), `cancel()` (abort current),
  `cancel_retry()` (finish current, stop loop), `continue_()`; exposes
  `failure_count`, `failure_reason`, `is_paused`, `is_resumed`.
- Gating: `can_fetch(OnlineState, NetworkMode)`; pause on `online` mode when offline;
  `always` never pauses on offline; `offline_first` runs attempt 1 offline but retries pause.
- Mutations default `retry = false`; `fetch_query`-style imperative fetches force no retry.

### 3.3 `network.rs` + `online_manager.rs`
```rust
pub enum NetworkMode { Online, Always, OfflineFirst }
pub fn can_fetch(mode: NetworkMode, online: bool) -> bool
pub struct OnlineManager { /* start: online=true; subscribe/set_online/is_online */ }
```

### 3.4 `notify_manager.rs` + `timeout_manager.rs`
- `NotifyManager::batch(f)` — nested-batch safe; flush once per outermost batch;
  `schedule(cb)` — deferred via scheduler (default: tokio spawn), `batch_calls`
  helper. GPUI mapping: notification storms collapse to a single notify.
- `TimeoutManager` — injectable `sleep` provider (`tokio` default; swap for tests),
  used by retry backoff, GC timers, refetch intervals, throttles.

### 3.5 `query_cache.rs` + `mutation_cache.rs`
- `QueryCache`: keyed by `query_hash`; events
  `Added, Removed, Updated { action }, ObserverAdded, ObserverRemoved,
  ObserverResultsUpdated, ObserverOptionsUpdated, Cleared, Subscribed, Unsubscribed`;
  `find/find_all` with `QueryFilters`; config callbacks
  `on_success/on_error/on_settled` (fire-and-forget).
- `MutationCache`: insertion-order store with `mutation_id` counter; `build/add/remove/
  clear/get_all/find/find_all`; **scope queues**: mutations sharing `scope.id` run
  serially (`can_run`/`run_next`); awaited cache callbacks
  `on_mutate/on_success/on_error/on_settled`; `resume_paused_mutations()`.
- GC: `schedule_gc` (runs once after `gc_time` of inactivity), `update_gc_time` is
  monotonic (max), never GCs while fetching/pending, re-armed after each fetch.

### 3.6 `observer.rs` (extended) — TanStack `QueryObserverResult` parity
Result flags added: `data_updated_at, error_updated_at, error_update_count,
failure_count, failure_reason, is_fetched, is_fetched_after_mount, is_loading_error,
is_refetch_error, is_placeholder_data, is_paused, is_enabled, is_stale, is_refetching,
is_loading (is_pending && is_fetching)`. Plus:
- `should_fetch_on_mount` predicates (`refetch_on_mount: bool | "always"`, `retry_on_mount`,
  static-stale short-circuit),
- `refetch(RefetchOptions { cancel_refetch = true, throw_on_error = false })`,
- `fetch_optimistic()` (never rejects),
- `select` memo (recompute only when data or select identity changes, errors captured),
- placeholder resolution (`keep_previous_data` helper = `PlaceholderData::previous`),
- tracked-props notify filtering (`notify_on_change_props`).
- `InfiniteQueryObserver` additions: `has_next_page/has_previous_page`,
  `is_fetching_next_page/is_fetching_previous_page`, `is_fetch_next_page_error`,
  direction from `fetch_meta`, `fetch_next_page({cancel_refetch, throw_on_error, page_param})`.

### 3.7 `mutation.rs` + `mutation_observer.rs`
- Full option surface: `on_settled`, `retry (default 0)`, `retry_delay`, `network_mode`,
  `gc_time`, `scope: MutationScope { id }`, `meta`, `throw_on_error`.
- `mutate(vars)` + `mutate_with(vars, MutateOptions { on_success, on_error, on_settled })`
  — per-call callbacks run **last** and only while the observer is subscribed;
  `mutate_async(vars) -> Result`.
- Exact side-effect order: `cache.on_mutate → options.on_mutate → dispatch pending →
  mutation_fn → cache.on_success → options.on_success → cache.on_settled →
  options.on_settled → dispatch success` (error path mirrors with per-callback try/propagation).
- `MutationState` gains `context`, `variables`, `submitted_at`, `failure_count`,
  `failure_reason`, `is_paused`, `mutation_key`, `scope`.
- Paused mutations queue and resume on reconnect (`resume_paused_mutations`).
- `MutationObserver`: subscribe, `mutate`, `reset`, live state updates.
- `use_mutation_state(filters, select)` hook.

### 3.8 `hydration.rs` (upgraded)
- `DehydratedState { dehydrated_at, mutations, queries }`; `DehydratedQuery` gains
  `query_hash`, `query_key` payload, `promise: Option<...>` (pending only),
  `meta`, `query_type`.
- `DehydrateOptions { should_redact_errors = true, should_dehydrate_mutation = paused-only,
  should_dehydrate_query = success-only, serialize_data, default_options }`.
- `hydrate`: mutations first; never overwrite newer `data_updated_at`; force
  `fetch_status = idle`; promote `pending + data` → success with `data_updated_at =
  dehydrated_at`; create queries without fetch_fn as hydrate-only; error redaction
  (`QueryError::redacted`).

### 3.9 `persister.rs` + `persist_client.rs` + `broadcast.rs`
- `trait QueryStorage` (async get/set/remove) — impls for in-memory (tests) and any
  user backend; `create_query_persister(QueryPersisterOptions { storage, buster = "",
  max_age = 24h, prefix = "rs-query", serialize/deserialize (serde feature),
  refetch_on_restore = true, filters })` with
  `persister_fn / persist_query / persist_query_by_key / retrieve_query /
  persister_gc / restore_queries / remove_queries`.
- Whole-client: `trait ClientPersister`, `PersistedClient { timestamp, buster, client_state }`,
  `persist_query_client_restore/save/subscribe/persist_query_client`,
  `remove_oldest_query` retry (drop oldest by `data_updated_at`, keep mutations).
- `broadcast_query_client(client, channel_name)` — cross-instance cache sync with
  re-entrancy guard (remote-applied updates are not re-broadcast).

### 3.10 `client.rs` (surface completion)
New/extended methods (signatures follow TanStack; `Vec<QueryKey>`-style filters):
`fetch_query, prefetch_query, ensure_query_data, query, infinite_query,
fetch_infinite_query, prefetch_infinite_query, ensure_infinite_query_data,
get_query_state, get_queries_data, set_queries_data, invalidate_queries(filters),
refetch_queries(filters, {cancel_refetch, throw_on_error}), cancel_queries(filters,
{revert = true}), reset_queries(filters), remove_queries(filters),
resume_paused_mutations, is_fetching(filters), is_mutating(filters),
get_default_options/set_default_options, set_query_defaults/get_query_defaults,
set_mutation_defaults/get_mutation_defaults, clear, mount/unmount,
dehydrate/hydrate (with options), cache() → &QueryCache, mutation_cache() →
&MutationCache, is_restoring flag, online/focus integration, `query()` applies
`select` and forces `retry = false`.

### 3.11 `streamed_query.rs`, `updater.rs`, `skip_token`, misc
- `streamed_query(fn, { refetch_mode: Replace|Append|Reset, reducer, initial_value })`.
- `Updater<T>` (`Value(T) | Fn(prev) -> T`) + `functional_update` (undefined ⇒ no-op).
- `SKIP` sentinel queryFn ⇒ `enabled: false`; manual refetch of skip-queries errors.
- `hash_key` parity: object segments serialized with **alphabetically sorted keys**
  (JSON.stringify parity) + `partial_match_key` recursion for filters.
- `replace_equal_deep` depth cap 500; plain arrays/objects only; identity reuse.
- `QueryError` gains `Cancelled { revert: bool, silent: bool }`, `MissingQueryFn`,
  `Redacted`, `Paused`.

## 4. Toolchain & quality gates (part of the port contract)

1. **Deps**: all versions bumped to latest available (majors included); git deps
   (`gpui`, `gpui-component`) re-resolved to current HEAD; `edition = "2024"`.
2. **Lint**: `[lints.rust]` + `[lints.clippy]` (pedantic + nursery-selected) warnings
   configured in `Cargo.toml`; CI runs `cargo clippy --all-features --all-targets --
   -D warnings`. **Zero `#[allow]`/`#[expect]` suppressions in src/**.
3. **Coverage**: `cargo-llvm-cov` line coverage target 100% (`--fail-under-lines 100`
   attempted; CI reports coverage on every run).
4. **CI/CD**: GitHub Actions pinned to latest majors (checkout@v6, rust-toolchain@v9
   or dtolnay, Swatinem/rust-cache@v2, codecov/codecov-action@v5, softprops/action-gh-release@v2,
   wimpysworld/nothing-but-nail@v13 not used; cargo-dirty check), jobs: lint → test
   (with coverage) → build; release workflow runs `git-cliff` (cliff.toml, Conventional
   Commits) to generate `CHANGELOG.md` + GitHub Release notes on `v*` tags.
5. **README**: regenerated to document every module & feature in the new state.

## 5. Test matrix (behavioral invariants → tests)

Each research doc ends with a checklist; the implementation adds tests keyed to it:
retry backoff math (1s/2s/4s, cap 30s), retry counts & failure surfacing, mutation
callback ordering (incl. per-call callbacks), dedup of concurrent fetches, cancel
revert semantics, invalidation refetch-type matrix, stale/fresh time math, GC after
gc_time, structural sharing identity preservation + depth cap, hydration newer-data
guard + redaction + fetchStatus reset, pause/resume offline flow for queries &
mutations + resume_paused_mutations, scope serialization, persister save/restore/
expiry/buster/removeOldest, broadcast re-entrancy guard, filters matching matrix
(exact/prefix/stale/active/inactive/predicate/status), observer flags truth table,
infinite max_pages trimming + bidirectional fetch + hasNext/PreviousPage,
streamedQuery refetch modes, updater no-op semantics, skipToken, hash_key stability.

---

*End of contract. Implementation proceeds module by module; `docs/research/*` hold the
evidence and exact JS semantics for every row above.*
