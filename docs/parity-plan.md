I'll create a revised, comprehensive implementation plan that addresses the review feedback and aligns with the current codebase structure.

---

# Complete rs-query TanStack Query Port Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Achieve feature parity with TanStack Query core functionality, completing remaining items: `placeholderData`/`initialData` integration, full query cancellation with abort signals, structural sharing with opt‑in `PartialEq` requirement, and `useIsFetching`/`useIsMutating` reactive helpers.

**Architecture:** Extend `Query<T>` with type‑specific options; use a `QueryObserver::from_query` factory to apply initial/placeholder data uniformly; integrate `CancellationToken` for cancellation; implement structural sharing behind a `PartialEq`‑required builder method; add activity counters with a broadcast channel for reactive hooks.

**Tech Stack:** Rust, GPUI framework, Tokio (with `sync`, `time`, `rt`), DashMap, existing `rs-query` codebase.

---

## Chunk 1: PlaceholderData and InitialData Integration

### Task 1: Extend Query<T> with initial/placeholder data and builder methods

**Files:**
- Modify: `src/query.rs`
- Modify: `src/options.rs` (no changes needed here; options remain type‑agnostic)
- Modify: `src/observer.rs`
- Modify: `src/executor.rs`
- Test: Add tests in modified files

- [ ] **Step 1: Add fields to `Query<T>` for initial and placeholder data**

In `src/query.rs`, update the `Query` struct definition:

```rust
pub struct Query<T: Clone + Send + Sync + 'static> {
    pub key: QueryKey,
    pub fetch_fn: Arc<...>,
    pub select: Option<Arc<dyn Fn(&T) -> T + Send + Sync>>,
    pub options: QueryOptions,
    // New fields
    pub initial_data: Option<T>,
    pub initial_data_updated_at: Option<std::time::Instant>,
    pub placeholder_data: Option<PlaceholderData<T>>,
}
```

Update the `Clone` impl accordingly.

- [ ] **Step 2: Add builder methods to `Query<T>`**

```rust
pub fn initial_data(mut self, data: T) -> Self {
    self.initial_data = Some(data);
    self.initial_data_updated_at = Some(std::time::Instant::now());
    self
}

pub fn placeholder_data(mut self, data: PlaceholderData<T>) -> Self {
    self.placeholder_data = Some(data);
    self
}
```

- [ ] **Step 3: Add a `QueryObserver::from_query` factory method**

In `src/observer.rs`, add:

```rust
impl<T: Clone + Send + Sync + 'static> QueryObserver<T> {
    pub fn from_query(client: &QueryClient, query: &Query<T>) -> Self {
        let key = query.key.clone();
        let cache_key = key.cache_key().to_string();
        let receiver = client.subscribe(&cache_key);

        let mut state = if let Some(data) = client.get_query_data::<T>(&key) {
            if client.is_stale(&key) {
                QueryState::Stale(data)
            } else {
                QueryState::Success(data)
            }
        } else if let Some(initial) = &query.initial_data {
            // Insert initial data into cache
            client.set_query_data(&key, initial.clone(), query.options.clone());
            QueryState::Success(initial.clone())
        } else {
            QueryState::Idle
        };

        // If placeholder is set and we're in Idle, we could show placeholder,
        // but placeholder is typically applied during loading state.
        // We'll handle that in `set_loading`.

        Self {
            key,
            state,
            client: client.clone(),
            receiver,
            options: query.options.clone(),
            fetch_started_at: None,
            _marker: PhantomData,
            // Store query reference? Not needed for now.
        }
    }

    // Existing `new` remains for backward compat, but may not apply initial data.
}
```

- [ ] **Step 4: Modify `QueryObserver::set_loading` to apply placeholder data**

Add a field to `QueryObserver` to store the `Query` reference or the `PlaceholderData` directly. Simpler: pass `placeholder_data` to `set_loading` as an optional parameter, or store it in the observer during construction.

Update `QueryObserver` struct:

```rust
pub struct QueryObserver<T> {
    // ... existing fields
    placeholder_data: Option<PlaceholderData<T>>,
}
```

In `from_query`, set `placeholder_data: query.placeholder_data.clone()`.

In `set_loading(&mut self)`:

```rust
pub fn set_loading(&mut self) {
    self.fetch_started_at = Some(Instant::now());
    if let Some(data) = self.state.data_cloned() {
        self.state = QueryState::Refetching(data);
    } else if let Some(placeholder) = &self.placeholder_data {
        let resolved = placeholder.resolve(None);
        self.state = QueryState::LoadingWithPlaceholder(resolved);
    } else {
        self.state = QueryState::Loading;
    }
}
```

We need a new `QueryState` variant `LoadingWithPlaceholder(T)` or add a flag to `Loading`. To minimize changes, add a variant:

```rust
pub enum QueryState<T> {
    // ...
    LoadingWithPlaceholder(T),
}
```

Update all methods (`data()`, `is_loading()`, etc.) to handle this variant correctly (return the placeholder data, consider it loading).

- [ ] **Step 5: Update `spawn_query` to use `from_query`**

In `src/executor.rs`, replace the inline observer creation with `QueryObserver::from_query(&client, &query)`. This ensures initial data is applied automatically.

- [ ] **Step 6: Write tests for initial data**

In `src/query.rs` test module:

```rust
#[test]
fn test_query_with_initial_data() {
    let client = QueryClient::new();
    let key = QueryKey::new("test");
    let query: Query<String> = Query::new(key.clone(), || async { Ok("fetched".to_string()) })
        .initial_data("initial".to_string());

    let observer = QueryObserver::from_query(&client, &query);
    assert_eq!(observer.state().data(), Some(&"initial".to_string()));
}
```

- [ ] **Step 7: Write tests for placeholder data**

```rust
#[tokio::test]
async fn test_placeholder_data_shown_during_loading() {
    let client = QueryClient::new();
    let key = QueryKey::new("test");
    let query: Query<String> = Query::new(key.clone(), || async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok("fetched".to_string())
    })
    .placeholder_data(PlaceholderData::value("placeholder".to_string()));

    let mut observer = QueryObserver::from_query(&client, &query);
    observer.set_loading();
    assert!(matches!(observer.state(), QueryState::LoadingWithPlaceholder(ref s) if s == "placeholder"));
}
```

- [ ] **Step 8: Run all tests and verify they pass**

```bash
cargo test --lib
```

- [ ] **Step 9: Commit**

```bash
git add src/query.rs src/observer.rs src/executor.rs src/state.rs
git commit -m "feat: add initialData and placeholderData support with QueryObserver::from_query"
```

---

## Chunk 2: Full Query Cancellation with CancellationToken

### Task 1: Integrate CancellationToken into query execution

**Files:**
- Modify: `src/client.rs`
- Modify: `src/executor.rs`
- Create: `src/cancellation.rs`
- Test: Add tests in `src/executor.rs`

- [ ] **Step 1: Add `CancellationToken` storage per query key in `QueryClient`**

In `src/client.rs`, add:

```rust
use tokio_util::sync::CancellationToken;

pub(crate) struct InFlightTask {
    pub abort_handle: AbortHandle,
    pub cancel_token: CancellationToken,
}

pub struct QueryClient {
    // ...
    in_flight: Arc<DashMap<String, InFlightTask>>,
}
```

Update methods:
- `is_in_flight` checks `in_flight.contains_key`.
- `set_abort_handle` replaced by `set_in_flight` that stores both handle and token.
- `clear_abort_handle` removes entry.
- `cancel_query` calls `token.cancel()` and `handle.abort()`.

- [ ] **Step 2: Create cancellation helper in `src/cancellation.rs`**

```rust
use tokio_util::sync::CancellationToken;
use crate::QueryError;

pub async fn cancellable_fetch<T, F, Fut>(
    token: CancellationToken,
    fetch_fn: F,
) -> Result<T, QueryError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, QueryError>>,
{
    tokio::select! {
        _ = token.cancelled() => Err(QueryError::Cancelled),
        result = fetch_fn() => result,
    }
}
```

- [ ] **Step 3: Modify `spawn_query` to create task and store token**

In `src/executor.rs`, inside `spawn_query`:

```rust
let token = CancellationToken::new();
let token_clone = token.clone();
let fetch_task = tokio::spawn(async move {
    execute_query(&client, &query, token_clone).await
});
client.set_in_flight(&query.key, InFlightTask {
    abort_handle: fetch_task.abort_handle(),
    cancel_token: token,
});
```

- [ ] **Step 4: Update `execute_query` signature to accept `CancellationToken`**

```rust
pub async fn execute_query<T: Clone + Send + Sync + 'static>(
    client: &QueryClient,
    query: &Query<T>,
    token: CancellationToken,
) -> QueryState<T> {
    // ...
    loop {
        if token.is_cancelled() {
            return QueryState::Error {
                error: QueryError::Cancelled,
                stale_data: client.get_query_data(&query.key),
            };
        }
        // existing retry logic, but wrap fetch_fn call with cancellable_fetch
        match cancellable_fetch(token.clone(), || (query.fetch_fn)()).await {
            Ok(data) => { ... }
            Err(QueryError::Cancelled) => { ... }
            Err(e) => { ... }
        }
    }
}
```

- [ ] **Step 5: Ensure `invalidate_queries` cancels in‑flight tasks**

Already calls `abort_handles.remove(...).abort()`. Update to use `in_flight` and call both `cancel_token.cancel()` and `abort_handle.abort()`.

- [ ] **Step 6: Write tests for cancellation**

In `src/executor.rs` test module:

```rust
#[tokio::test]
async fn test_query_cancellation() {
    let client = QueryClient::new();
    let key = QueryKey::new("test");
    let query: Query<String> = Query::new(key.clone(), || async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok("done".to_string())
    });

    let handle = tokio::spawn({
        let client = client.clone();
        let query = query.clone();
        async move {
            let token = CancellationToken::new();
            execute_query(&client, &query, token).await
        }
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    client.cancel_query(&key);
    let state = handle.await.unwrap();
    assert!(matches!(state, QueryState::Error { error: QueryError::Cancelled, .. }));
}
```

- [ ] **Step 7: Run all tests**

```bash
cargo test --lib
```

- [ ] **Step 8: Commit**

```bash
git add src/client.rs src/executor.rs src/cancellation.rs Cargo.toml  # add tokio-util if not present
git commit -m "feat: implement full query cancellation with CancellationToken"
```

---

## Chunk 3: Structural Sharing with Opt‑in PartialEq

### Task 1: Implement `replace_equal_deep` using a custom trait with fallback

**Files:**
- Modify: `src/sharing.rs`
- Modify: `src/client.rs`
- Modify: `src/query.rs` (to enforce PartialEq bound when structural sharing is enabled)
- Test: Add tests in `src/sharing.rs`

- [ ] **Step 1: Define `PartialEqAny` trait in `src/sharing.rs`**

```rust
use std::any::Any;

pub trait PartialEqAny: Any {
    fn as_any(&self) -> &dyn Any;
    fn eq_any(&self, other: &dyn Any) -> bool;
}

impl<T: PartialEq + 'static> PartialEqAny for T {
    fn as_any(&self) -> &dyn Any { self }
    fn eq_any(&self, other: &dyn Any) -> bool {
        other.downcast_ref::<T>().map_or(false, |o| self == o)
    }
}
```

- [ ] **Step 2: Change `CacheEntry` data type to `Arc<dyn PartialEqAny + Send + Sync>`**

In `src/client.rs`:

```rust
pub(crate) struct CacheEntry {
    pub(crate) data: Arc<dyn PartialEqAny + Send + Sync>,
    // ...
}
```

Update all methods that manipulate `data` to use this new type. For `set_query_data`, wrap `T` in a struct that implements `PartialEqAny`. Since `T` may not implement `PartialEq`, we need a wrapper that can store either a comparable or non‑comparable value.

Alternative: Keep `Arc<dyn Any + Send + Sync>` and in `replace_equal_deep_any`, attempt downcast to `dyn PartialEqAny` (by checking if the type implements the trait). That's safer and doesn't force all types to be comparable.

Define a helper trait `MaybePartialEqAny` with a blanket impl for all `T` that checks `TypeId` and attempts to downcast.

Simpler: In `set_query_data`, if `structural_sharing` is true, we **require** that `T` implements `PartialEq`. We'll enforce this at the `Query` builder level.

- [ ] **Step 3: Enforce `PartialEq` bound on `.structural_sharing(true)` in `Query<T>`**

In `src/query.rs`:

```rust
pub fn structural_sharing(mut self, enabled: bool) -> Self
where
    T: PartialEq,
{
    self.options.structural_sharing = enabled;
    self
}
```

This ensures that only queries whose data type implements `PartialEq` can enable structural sharing. This is a compile‑time guarantee, so we can safely downcast in `set_query_data`.

- [ ] **Step 4: Implement `replace_equal_deep_any` with type‑safe comparison**

In `src/sharing.rs`:

```rust
pub(crate) fn replace_equal_deep_any<T: PartialEq + 'static>(
    old: Arc<T>,
    new: Arc<T>,
) -> Arc<T> {
    if *old == *new {
        old
    } else {
        new
    }
}
```

In `client.rs`, inside `set_query_data`:

```rust
if options.structural_sharing {
    if let Some(old_entry) = self.cache.get(&cache_key) {
        if old_entry.type_id == TypeId::of::<T>() {
            // Safe because T: PartialEq is guaranteed by structural_sharing = true
            if let (Some(old), Some(new)) = (
                old_entry.data.downcast_ref::<T>(),
                final_data.downcast_ref::<T>(),
            ) {
                let old_arc: Arc<T> = /* extract from old_entry */;
                let new_arc: Arc<T> = /* from final_data */;
                final_data = replace_equal_deep_any(old_arc, new_arc);
            }
        }
    }
}
```

This approach avoids global trait changes and uses the compiler‑enforced `PartialEq` bound.

- [ ] **Step 5: Write tests for structural sharing**

In `src/sharing.rs`:

```rust
#[test]
fn test_structural_sharing_keeps_same_arc_when_equal() {
    let old = Arc::new(vec![1, 2, 3]);
    let new = Arc::new(vec![1, 2, 3]);
    let result = replace_equal_deep_any(old.clone(), new);
    assert!(Arc::ptr_eq(&old, &result));
}

#[test]
fn test_structural_sharing_replaces_when_not_equal() {
    let old = Arc::new(vec![1, 2, 3]);
    let new = Arc::new(vec![4, 5, 6]);
    let result = replace_equal_deep_any(old.clone(), new.clone());
    assert!(!Arc::ptr_eq(&old, &result));
    assert_eq!(*result, *new);
}
```

Also add integration tests in `src/client.rs` that verify cache updates with structural sharing enabled and disabled.

- [ ] **Step 6: Run tests**

```bash
cargo test --lib
```

- [ ] **Step 7: Commit**

```bash
git add src/sharing.rs src/client.rs src/query.rs
git commit -m "feat: implement structural sharing with opt-in PartialEq requirement"
```

---

## Chunk 4: Helper Hooks – useIsFetching and useIsMutating

### Task 1: Add activity counters and broadcast channel to QueryClient

**Files:**
- Modify: `src/client.rs`
- Create: `src/hooks.rs`
- Modify: `src/lib.rs`
- Test: Add tests in `src/hooks.rs`

- [ ] **Step 1: Add atomic counters and broadcast sender to `QueryClient`**

In `src/client.rs`:

```rust
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::broadcast;

#[derive(Clone, Debug)]
pub enum ActivityEvent {
    FetchingCountChanged(usize),
    MutatingCountChanged(usize),
}

pub struct QueryClient {
    // ... existing fields
    fetching_count: Arc<AtomicUsize>,
    mutating_count: Arc<AtomicUsize>,
    activity_tx: broadcast::Sender<ActivityEvent>,
}
```

Initialize with a channel capacity (e.g., 16). Add methods:

```rust
pub fn is_fetching(&self) -> bool {
    self.fetching_count.load(Ordering::Relaxed) > 0
}

pub fn fetching_count(&self) -> usize {
    self.fetching_count.load(Ordering::Relaxed)
}

pub fn subscribe_activity(&self) -> broadcast::Receiver<ActivityEvent> {
    self.activity_tx.subscribe()
}

fn inc_fetching(&self) {
    let new = self.fetching_count.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = self.activity_tx.send(ActivityEvent::FetchingCountChanged(new));
}

fn dec_fetching(&self) {
    let new = self.fetching_count.fetch_sub(1, Ordering::Relaxed) - 1;
    let _ = self.activity_tx.send(ActivityEvent::FetchingCountChanged(new));
}
// Similar for mutations.
```

- [ ] **Step 2: Integrate counter updates in `spawn_query` and `spawn_mutation`**

In `src/executor.rs`:

For `spawn_query`, increment before spawning the Tokio task, and decrement in a `finally` block (e.g., using a guard struct that impls `Drop`).

Create a guard type:

```rust
struct FetchingGuard {
    client: QueryClient,
}
impl Drop for FetchingGuard {
    fn drop(&mut self) {
        self.client.dec_fetching();
    }
}
```

In `spawn_query`:

```rust
client.inc_fetching();
let guard = FetchingGuard { client: client.clone() };
// spawn task, move guard into the async block so it lives until completion.
```

Similarly for mutations.

- [ ] **Step 3: Implement GPUI hook `use_is_fetching`**

In `src/hooks.rs`:

```rust
use gpui::{Context, Model, Subscription};
use crate::{QueryClient, ActivityEvent};

pub fn use_is_fetching<V: 'static>(
    cx: &mut Context<V>,
    client: &QueryClient,
) -> Model<bool> {
    let model = cx.new_model(|_| client.is_fetching());
    let mut rx = client.subscribe_activity();
    let client = client.clone();
    cx.spawn(|model, mut cx| async move {
        while let Ok(event) = rx.recv().await {
            if let ActivityEvent::FetchingCountChanged(_) = event {
                model.update(&mut cx, |value, cx| {
                    *value = client.is_fetching();
                    cx.notify();
                }).ok();
            }
        }
    }).detach();
    model
}

pub fn use_is_mutating<V: 'static>(cx: &mut Context<V>, client: &QueryClient) -> Model<bool> {
    // similar
}
```

- [ ] **Step 4: Write tests for counter increments**

In `src/client.rs` test module:

```rust
#[tokio::test]
async fn test_fetching_counter() {
    let client = QueryClient::new();
    assert!(!client.is_fetching());
    // spawn a query that takes a while
    // verify counter increments and decrements.
}
```

For hooks, test that the model updates when activity changes (may require a simple GPUI test setup).

- [ ] **Step 5: Run tests**

```bash
cargo test --lib
```

- [ ] **Step 6: Commit**

```bash
git add src/client.rs src/executor.rs src/hooks.rs src/lib.rs
git commit -m "feat: add useIsFetching and useIsMutating reactive hooks"
```

---

## Chunk 5: Final Polish and Documentation

### Task 1: Update README, API docs, and ensure all public items are documented

**Files:**
- Modify: `README.md`
- Modify: `src/lib.rs`
- Modify: Any other public modules missing docs

- [ ] **Step 1: Update README with new features**

Add sections for:
- `initialData` and `placeholderData`
- Query cancellation
- Structural sharing (with `PartialEq` requirement)
- `useIsFetching` / `useIsMutating` hooks

Update the comparison table to mark these items as ✅ Implemented.

- [ ] **Step 2: Add doc examples for new APIs**

In `src/lib.rs` or individual modules, add code examples for:
- `Query::initial_data()`
- `Query::placeholder_data()`
- `QueryObserver::from_query()`
- `QueryClient::cancel_query()`
- `use_is_fetching`

- [ ] **Step 3: Run `cargo doc` to verify no broken links**

```bash
cargo doc --no-deps --open
```

- [ ] **Step 4: Run full test suite again**

```bash
cargo test --lib
```

- [ ] **Step 5: Commit final changes**

```bash
git add README.md src/lib.rs src/*.rs
git commit -m "docs: finalize documentation and README for completed TanStack Query port"
```

---

**Plan complete and saved. Ready to execute?**
