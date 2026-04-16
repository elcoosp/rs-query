I have generated a comprehensive implementation plan for the rewritten `rs-query` library. The plan is structured into manageable chunks, each covering a specific feature area and containing actionable tasks.

---

## 📋 rs‑query – Complete Implementation Plan

### Current State Assessment

The rewritten `rs-query` library provides a solid foundation:

- **QueryClient** uses `DashMap` for concurrent cache access and a background thread for garbage collection.
- **QueryKey** pre‑computes its cache key string for efficient lookups and hashing.
- **Executor** uses GPUI’s `background_executor()` and `futures_timer::Delay` for retry backoff, avoiding a separate Tokio runtime.
- **Core features** implemented: cache, stale‑while‑revalidate, automatic retries, request deduplication, hierarchical invalidation.

The following features from TanStack Query are **not yet implemented**:

- Query / mutation observers
- Optimistic updates
- Infinite queries / pagination
- `placeholderData` / `initialData` / `select` transformation
- `refetchInterval` / window focus refetch
- Hydration / persistence
- Devtools
- Structural sharing
- Full query cancellation (partial abort support exists)

---

## 🧱 Implementation Chunks

### Chunk 1: Observers & Reactive Query State

**Objective:** Enable components to subscribe to query state changes and react to background refetches or cache updates.

**Tasks:**

- [ ] **1.1** Define a `QueryObserver<T>` struct that holds a `QueryKey`, a reference to the `QueryClient`, and a cached `QueryState<T>`.
  - The observer will be cloneable and can be stored in a GPUI `Model` to drive reactive UI updates.
- [ ] **1.2** Implement a subscription system within `QueryClient`.
  - Add a `broadcast::Sender<QueryStateUpdate>` (or a callback registry) to `QueryClient` that notifies subscribers when a query’s state changes.
  - Changes include: cache updates (`set_query_data`), invalidation, fetch start/success/error, garbage collection.
  - Since `DashMap` does not natively support watching, we will add a `tokio::sync::broadcast` channel per `QueryKey` lazily created on first subscription.
- [ ] **1.3** Update `QueryObserver` to subscribe to the channel for its key on creation and unsubscribe on drop.
- [ ] **1.4** Provide a GPUI‑friendly helper:
  - `fn use_query<V: 'static, T: Clone + Send + Sync + 'static>(cx: &mut Context<V>, query: &Query<T>) -> Model<QueryObserver<T>>`
  - This creates an observer and wraps it in a `Model` that updates automatically when the observer’s state changes.
- [ ] **1.5** Refactor `spawn_query` to integrate with observers:
  - Instead of a one‑time callback, the function now returns (or provides) an observer that the caller can hold.
  - (Optional) Deprecate the callback‑based API in favor of the observer pattern.
- [ ] **1.6** Write unit tests verifying subscription notifications and proper cleanup.

**Expected Outcome:** Components can hold a reactive query state that updates on cache changes, enabling UI to reflect background refetches and manual cache updates.

---

### Chunk 2: Optimistic Updates & Mutation Enhancements

**Objective:** Support optimistic cache updates with automatic rollback on mutation failure.

**Tasks:**

- [ ] **2.1** Extend the `Mutation` struct with an optional `on_mutate` callback.
  - Signature: `Fn(&mut QueryClient, &P) -> Result<RollbackContext, QueryError>`
  - The callback receives mutable access to the `QueryClient` to perform optimistic updates (e.g., via `set_query_data`).
  - Returns a `RollbackContext` that contains enough information to revert the changes (e.g., a closure or snapshot).
- [ ] **2.2** Modify `spawn_mutation` to:
  - Call `on_mutate` (if provided) **before** executing the mutation function.
  - If `on_mutate` succeeds, store the rollback context and proceed with the mutation.
  - On mutation error, invoke the rollback logic using the stored context.
- [ ] **2.3** Ensure that cache invalidations triggered after a successful mutation do not interfere with the optimistic update.
- [ ] **2.4** Expose mutation state (e.g., `is_pending`, `variables`) via a `MutationObserver` similar to queries, allowing UI to render temporary items.
- [ ] **2.5** Write tests for successful and failed optimistic updates, verifying that the cache is correctly rolled back on error.

**Expected Outcome:** Mutations can modify the cache optimistically, improving perceived performance, with safe rollback on errors.

---

### Chunk 3: Infinite Queries

**Objective:** Implement `useInfiniteQuery` semantics, including paginated data and load‑more functionality.

**Tasks:**

- [ ] **3.1** Define `InfiniteQuery<T, P>` struct:
  - Contains `QueryKey`, a `fetch_fn` that receives `page_param: P`, `initial_page_param: P`, and functions `get_next_page_param` and `get_previous_page_param`.
- [ ] **3.2** Extend `QueryClient` cache to store `InfiniteData<T>`:
  - `InfiniteData` holds `pages: Vec<T>` and `page_params: Vec<P>`.
- [ ] **3.3** Implement `spawn_infinite_query` executor:
  - Similar to `spawn_query`, but manages page fetching.
  - Provides `fetch_next_page` and `fetch_previous_page` methods on the observer.
  - Maintains `has_next_page` / `has_previous_page` flags derived from the page param functions.
- [ ] **3.4** Add `max_pages` option to limit stored pages (evict oldest pages when limit exceeded).
- [ ] **3.5** Support `select` transformation on infinite query data (apply to the entire `pages` vector).
- [ ] **3.6** Write tests for basic pagination, bi‑directional fetching, and `max_pages`.

**Expected Outcome:** Applications can implement infinite scroll / pagination with automatic caching of pages.

---

### Chunk 4: Query Options (`placeholderData`, `initialData`, `select`)

**Objective:** Enhance `Query` configuration with options that mirror TanStack Query.

**Tasks:**

- [ ] **4.1** Add `initial_data: Option<T>` and `initial_data_updated_at: Option<Instant>` to `QueryOptions`.
  - On query creation, if cache is empty and `initial_data` is present, insert it as the initial cached value.
  - The `initial_data_updated_at` timestamp is used to calculate staleness correctly.
- [ ] **4.2** Add `placeholder_data: Option<PlaceholderData<T>>` (an enum that can be a value or a function `(prev: Option<T>) -> T`).
  - When a query is in `pending` state, return placeholder data to the observer while fetching.
  - Mark `is_placeholder_data` flag in `QueryState`.
- [ ] **4.3** Add `select: Option<Box<dyn Fn(&T) -> R + Send + Sync>>` to `QueryOptions`.
  - Apply transformation to the data before returning it to observers.
  - Ensure the transformed data is memoized and only recomputed when source data changes (e.g., using a cached hash of the input).
- [ ] **4.4** Update `get_query_data` and observer to respect these options.
- [ ] **4.5** Write tests for each option.

**Expected Outcome:** Queries can be initialized with existing data, show placeholder content while loading, and transform data for UI.

---

### Chunk 5: Refetch Interval & Focus Refetch (Optional)

**Objective:** Implement periodic background refetching and (optionally) refetch on window/app focus.

**Tasks:**

- [ ] **5.1** Add `refetch_interval: Option<Duration>` to `QueryOptions`.
  - When a query has active observers, spawn a timer that calls `refetch` at the given interval.
  - Use `tokio::time::interval` for the timer; ensure it is cancelled when the last observer unsubscribes.
- [ ] **5.2** Add `refetch_interval_in_background: bool` to control whether the timer continues when the app is not focused (if focus detection is available).
- [ ] **5.3** (Optional) Implement a platform‑agnostic `FocusManager` trait.
  - For GPUI, expose a way for the application to notify the `QueryClient` of focus changes (e.g., via `cx.on_app_focus`).
  - Add `refetch_on_window_focus: bool` option that triggers a refetch of all active stale queries when focus is regained.
- [ ] **5.4** Write tests for interval refetch (using fake time via `tokio::time::pause`).

**Expected Outcome:** Data stays fresh automatically with periodic polling and optional focus‑based refresh.

---

### Chunk 6: Structural Sharing

**Objective:** Reduce unnecessary clones and improve cache efficiency by sharing unchanged parts of data structures.

**Tasks:**

- [ ] **6.1** Implement a `replace_equal_deep` function similar to TanStack Query’s.
  - Recursively compare old and new data (using `PartialEq` where possible) and keep references to unchanged portions.
- [ ] **6.2** Integrate structural sharing into `set_query_data`.
  - When updating cache with new data, use `replace_equal_deep(old, new)` to produce a value that shares as much as possible.
- [ ] **6.3** Add `structural_sharing: bool` option to `QueryOptions` (default `true`).
- [ ] **6.4** Write tests to verify that unchanged nested objects retain the same pointer/reference (e.g., via `Arc` reference equality).

**Expected Outcome:** Cache updates are more memory‑efficient and components relying on `PartialEq` re‑render less often.

---

### Chunk 7: Query Cancellation Improvements

**Objective:** Fully support cancelling in‑flight queries when they are invalidated or no longer needed.

**Tasks:**

- [ ] **7.1** Replace the current `in_flight` tracking with `AbortHandle` storage.
  - When spawning a query task, obtain an `AbortHandle` from the task and store it keyed by `QueryKey`.
- [ ] **7.2** On `invalidate_queries` or when the last observer unsubscribes, call `abort()` on the handle to cancel the in‑flight fetch.
- [ ] **7.3** Ensure the `query_fn` receives an `AbortSignal` equivalent (e.g., a `tokio::sync::watch::Receiver` that indicates cancellation).
  - Provide a helper `async fn with_cancellation<F, T>(cancel: watch::Receiver<bool>, fut: F) -> Result<T, Cancelled>`.
- [ ] **7.4** Update retry logic to respect cancellation and not retry aborted requests.
- [ ] **7.5** Write tests for cancellation during fetch and retry.

**Expected Outcome:** Queries can be cleanly aborted, freeing resources and preventing stale data from overwriting cache.

---

### Chunk 8: Hydration & Persistence (Dehydrate/Hydrate)

**Objective:** Allow the query cache to be serialized and restored, enabling SSR‑like patterns and offline persistence.

**Tasks:**

- [ ] **8.1** Define `DehydratedState` and `HydrateOptions` structs.
  - `DehydratedState` contains serializable representations of cached queries and mutations (e.g., key, data, timestamps).
- [ ] **8.2** Implement `QueryClient::dehydrate(&self) -> DehydratedState`.
  - Only include successful queries by default (configurable via `should_dehydrate_query` closure).
- [ ] **8.3** Implement `QueryClient::hydrate(&mut self, state: DehydratedState)`.
  - Merge dehydrated state into existing cache, respecting timestamps (only overwrite if dehydrated data is newer).
- [ ] **8.4** Provide a `HydrationBoundary` equivalent for GPUI (e.g., a component that hydrates a client before rendering children).
- [ ] **8.5** Add support for custom serialization via a `Serializer` trait (allowing `serde` integration) and error redaction.
- [ ] **8.6** Write tests for hydration and merging logic.

**Expected Outcome:** Cache can be persisted to disk or transferred from server to client, enabling fast startup and offline support.

---

### Chunk 9: Devtools (Optional)

**Objective:** Provide a developer tool to inspect and manipulate the query cache during development.

**Tasks:**

- [ ] **9.1** Create a separate crate `rs-query-devtools` that integrates with GPUI.
- [ ] **9.2** Implement a `QueryDevtools` component that displays:
  - List of active queries with their keys, status, data, and timestamps.
  - Buttons to refetch, invalidate, or remove queries.
  - A toggle to simulate offline mode (by pausing network requests via a custom `NetworkManager`).
- [ ] **9.3** Use the observer system (Chunk 1) to subscribe to all cache changes.
- [ ] **9.4** Provide a way to attach the devtools to a `QueryClient` instance (e.g., `cx.spawn(|mut cx| { ... })`).
- [ ] **9.5** (Optional) Add a browser‑extension style floating button to toggle the panel.

**Expected Outcome:** Developers can debug cache behavior visually, accelerating development.

---

## 🔄 Implementation Order & Dependencies

1. **Chunk 1 (Observers)** – foundational for reactive UI and later features.
2. **Chunk 2 (Optimistic Updates)** – can be done in parallel with Chunk 3.
3. **Chunk 3 (Infinite Queries)** – requires observer system for state updates.
4. **Chunk 4 (Query Options)** – enhances existing query functionality.
5. **Chunk 5 (Refetch Interval)** – independent, can be done anytime.
6. **Chunk 6 (Structural Sharing)** – optimizes cache performance.
7. **Chunk 7 (Cancellation)** – improves resource management.
8. **Chunk 8 (Hydration)** – useful for SSR/persistence.
9. **Chunk 9 (Devtools)** – final polish, depends on observers.

Each chunk should be implemented in a separate pull request, and the plan chunk should be reviewed before coding begins. This ensures focused, testable increments and minimizes integration issues.
