// src/queries_observer.rs
//! Port of `TanStack` Query's `QueriesObserver` — the non-GPUI core behind
//! `useQueries`.
//!
//! A [`QueriesObserver`] tracks a list of query specs ([`crate::query::Query`]
//! values bundling key + `fetch_fn` + options), materialises a
//! [`QueryState`](crate::state::QueryState) per spec and exposes `TanStack`'s
//! `combine` escape hatch for deriving a single value from all results.
//!
//! Execution model: `run_all`/`refresh` spawn [`crate::executor::execute_query`]
//! tasks and return their [`JoinHandle`]s. Every completed task deposits its
//! final state into a shared per-index buffer; [`QueriesObserver::sync`] folds
//! those buffered states back into the observer's results so they can be read
//! with [`QueriesObserver::results`]. Set-pruning and additions are handled by
//! [`QueriesObserver::set_specs`], which reconciles by cache key: states for
//! unchanged keys are preserved, newly added specs are executed immediately
//! and their handles returned.

use crate::executor::execute_query;
use crate::query::Query;
use crate::{QueryClient, QueryState};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Multi-query observer mirroring `TanStack`'s `QueriesObserver`.
pub struct QueriesObserver<T: Clone + Send + Sync + PartialEq + 'static> {
    specs: Vec<Query<T>>,
    results: Vec<QueryState<T>>,
    /// Per-index slot for the latest completed execution, written from the
    /// spawned tasks and pulled into `results` by [`QueriesObserver::sync`].
    completed: Arc<Mutex<Vec<Option<QueryState<T>>>>>,
    client: QueryClient,
}

impl<T: Clone + Send + Sync + PartialEq + 'static> QueriesObserver<T> {
    /// Create an observer for the given specs. Initial results are read from
    /// the client cache (seeding `initial_data` like
    /// [`crate::observer::QueryObserver::from_query`] when the cache is
    /// empty).
    pub fn new(client: &QueryClient, specs: Vec<Query<T>>) -> Self {
        let results: Vec<QueryState<T>> = specs.iter().map(|q| initial_result(client, q)).collect();
        let completed = Arc::new(Mutex::new(vec![None; specs.len()]));
        Self {
            specs,
            results,
            completed,
            client: client.clone(),
        }
    }

    /// Replace the observed specs, reconciling by cache key.
    ///
    /// * Specs whose cache key already existed keep their current result and
    ///   completion slot.
    /// * Newly added specs get an initial (cache/`initial_data`-backed) state
    ///   and are executed immediately; their [`JoinHandle`]s are returned so
    ///   callers can await them (dropping the handles detaches the tasks).
    /// * Removed specs simply disappear together with their state.
    pub fn set_specs(&mut self, specs: Vec<Query<T>>) -> Vec<JoinHandle<QueryState<T>>> {
        let mut old_results: Vec<Option<QueryState<T>>> = std::mem::take(&mut self.results)
            .into_iter()
            .map(Some)
            .collect();
        let old_completed = Arc::clone(&self.completed);
        let previous = self.specs.clone();

        let mut results = Vec::with_capacity(specs.len());
        let mut completed = Vec::with_capacity(specs.len());
        let mut new_indices = Vec::new();

        for (idx, spec) in specs.iter().enumerate() {
            let cache_key = spec.key.cache_key().to_string();
            match previous.iter().position(|q| q.key.cache_key() == cache_key) {
                Some(old_idx) if old_idx < old_results.len() => {
                    results.push(old_results[old_idx].take().unwrap_or_default());
                    let slot = old_completed
                        .lock()
                        .ok()
                        .and_then(|slots| slots.get(old_idx).cloned())
                        .unwrap_or_default();
                    completed.push(slot);
                }
                _ => {
                    results.push(initial_result(&self.client, spec));
                    completed.push(None);
                    new_indices.push(idx);
                }
            }
        }

        self.specs = specs;
        self.results = results;
        self.completed = Arc::new(Mutex::new(completed));

        self.run_indices(&new_indices)
    }

    /// Execute every spec, returning one [`JoinHandle`] per executed query
    /// (in spec order).
    ///
    /// Before spawning, each result is optimistically marked
    /// `Loading`/`Refetching` (`TanStack`'s optimistic results). Disabled specs
    /// (`options.enabled`) are skipped. Completed states only become visible
    /// through [`QueriesObserver::results`] after the handles are awaited and
    /// [`QueriesObserver::sync`] is called.
    pub fn run_all(&mut self) -> Vec<JoinHandle<QueryState<T>>> {
        let indices: Vec<usize> = (0..self.specs.len()).collect();
        self.run_indices(&indices)
    }

    /// Execute a single spec by index, returning its [`JoinHandle`].
    ///
    /// Returns `None` when the index is out of bounds or the spec is
    /// disabled.
    pub fn refresh(&mut self, idx: usize) -> Option<JoinHandle<QueryState<T>>> {
        if idx >= self.specs.len() {
            return None;
        }
        self.run_indices(&[idx]).into_iter().next()
    }

    /// Fold the latest completed execution per index into [`QueriesObserver::results`].
    pub fn sync(&mut self) {
        if let Ok(slots) = self.completed.lock() {
            for (idx, slot) in slots.iter().enumerate() {
                if let (Some(state), Some(result)) = (slot, self.results.get_mut(idx)) {
                    *result = state.clone();
                }
            }
        }
    }

    /// Current per-spec results.
    #[must_use]
    pub fn results(&self) -> &[QueryState<T>] {
        &self.results
    }

    /// `TanStack` `combine`: derive a single value from all results.
    #[must_use]
    pub fn combine<R>(&self, combine_fn: impl Fn(&[QueryState<T>]) -> R) -> R {
        combine_fn(&self.results)
    }

    /// Number of observed specs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.specs.len()
    }

    /// Whether no specs are observed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// The observed query specs.
    #[must_use]
    pub fn specs(&self) -> &[Query<T>] {
        &self.specs
    }

    /// Mark the given indices optimistically and spawn their executions.
    fn run_indices(&mut self, indices: &[usize]) -> Vec<JoinHandle<QueryState<T>>> {
        let mut handles = Vec::with_capacity(indices.len());
        for &idx in indices {
            let Some(spec) = self.specs.get(idx) else {
                continue;
            };
            if !spec.options.is_enabled() {
                continue;
            }
            // Optimistic loading mark (TanStack optimistic results).
            if let Some(result) = self.results.get_mut(idx) {
                *result = match result.data_cloned() {
                    Some(data) => QueryState::Refetching(data),
                    None => QueryState::Loading,
                };
            }
            let query = spec.clone();
            let client = self.client.clone();
            let completed = Arc::clone(&self.completed);
            handles.push(tokio::spawn(async move {
                let state = execute_query(&client, &query, CancellationToken::new()).await;
                if let Ok(mut slots) = completed.lock()
                    && let Some(slot) = slots.get_mut(idx)
                {
                    *slot = Some(state.clone());
                }
                state
            }));
        }
        handles
    }
}

/// Materialise the observer-visible state of one spec, mirroring
/// [`crate::observer::QueryObserver::from_query`]: prefer cached data
/// (fresh/stale), otherwise seed `initial_data` into the cache, otherwise
/// idle.
fn initial_result<T>(client: &QueryClient, query: &Query<T>) -> QueryState<T>
where
    T: Clone + Send + Sync + PartialEq + 'static,
{
    if client.get_query_data::<T>(&query.key).is_none()
        && let Some(initial) = &query.initial_data
    {
        client.set_query_data(&query.key, initial.clone(), query.options.clone());
    }
    match client.get_query_data::<T>(&query.key) {
        Some(data) if client.is_in_flight(&query.key) => QueryState::Refetching(data),
        Some(data) if client.is_stale(&query.key) => QueryState::Stale(data),
        Some(data) => QueryState::Success(data),
        None if client.is_in_flight(&query.key) => QueryState::Loading,
        None => QueryState::Idle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QueryError;
    use crate::QueryKey;
    use crate::QueryOptions;

    fn ok_query(key: &QueryKey, data: &'static str) -> Query<String> {
        Query::new(key.clone(), move || async move { Ok(data.to_string()) })
    }

    fn failing_query(key: &QueryKey) -> Query<String> {
        Query::new(key.clone(), || async {
            Err(QueryError::custom("unrecoverable"))
        })
    }

    async fn await_all(handles: Vec<JoinHandle<QueryState<String>>>) {
        for handle in handles {
            let _ = handle.await;
        }
    }

    #[tokio::test]
    async fn test_run_all_tracks_success_and_error_mix() {
        let client = QueryClient::new();
        let ok_key = QueryKey::new("ok");
        let err_key = QueryKey::new("err");
        let mut observer = QueriesObserver::new(
            &client,
            vec![ok_query(&ok_key, "good"), failing_query(&err_key)],
        );

        let handles = observer.run_all();
        await_all(handles).await;
        observer.sync();

        let results = observer.results();
        assert!(results[0].is_success());
        assert_eq!(results[0].data(), Some(&"good".to_string()));
        assert!(results[1].is_error());
        assert_eq!(
            results[1].error(),
            Some(&QueryError::custom("unrecoverable"))
        );
    }

    #[tokio::test]
    async fn test_run_all_marks_optimistic_loading() {
        let client = QueryClient::new();
        let key = QueryKey::new("slow");
        let mut observer = QueriesObserver::new(&client, vec![ok_query(&key, "later")]);

        let handles = observer.run_all();
        assert!(observer.results()[0].is_loading());

        await_all(handles).await;
        observer.sync();
        assert!(observer.results()[0].is_success());
    }

    #[tokio::test]
    async fn test_run_all_refetch_marks_refetching() {
        let client = QueryClient::new();
        let key = QueryKey::new("cached");
        client.set_query_data(&key, "cached".to_string(), QueryOptions::default());
        let mut observer = QueriesObserver::new(&client, vec![ok_query(&key, "fresh")]);

        let handles = observer.run_all();
        assert!(observer.results()[0].is_refetching());

        await_all(handles).await;
        observer.sync();
        assert_eq!(observer.results()[0].data(), Some(&"fresh".to_string()));
    }

    #[tokio::test]
    async fn test_set_specs_reconciles_by_cache_key() {
        let client = QueryClient::new();
        let a_key = QueryKey::new("a");
        let b_key = QueryKey::new("b");
        let c_key = QueryKey::new("c");
        let mut observer =
            QueriesObserver::new(&client, vec![ok_query(&a_key, "A"), ok_query(&b_key, "B")]);
        await_all(observer.run_all()).await;
        observer.sync();
        assert!(observer.results()[1].is_success());

        // Replace A with C; B keeps its state, C starts fresh and runs.
        let handles = observer.set_specs(vec![ok_query(&b_key, "B"), ok_query(&c_key, "C")]);
        assert_eq!(handles.len(), 1, "only the newly added spec is executed");
        await_all(handles).await;
        observer.sync();

        assert_eq!(observer.len(), 2);
        assert_eq!(observer.specs()[0].key.cache_key(), "b");
        assert_eq!(observer.specs()[1].key.cache_key(), "c");
        assert!(observer.results()[0].is_success(), "B state is preserved");
        assert_eq!(observer.results()[0].data(), Some(&"B".to_string()));
        assert!(observer.results()[1].is_success());
        assert_eq!(observer.results()[1].data(), Some(&"C".to_string()));
    }

    #[tokio::test]
    async fn test_set_specs_removes_specs() {
        let client = QueryClient::new();
        let a_key = QueryKey::new("a");
        let b_key = QueryKey::new("b");
        let mut observer =
            QueriesObserver::new(&client, vec![ok_query(&a_key, "A"), ok_query(&b_key, "B")]);
        await_all(observer.run_all()).await;
        observer.sync();

        observer.set_specs(vec![ok_query(&a_key, "A")]);
        assert_eq!(observer.len(), 1);
        assert_eq!(observer.results().len(), 1);
        assert!(observer.results()[0].is_success());
    }

    #[tokio::test]
    async fn test_combine_derives_from_results() {
        let client = QueryClient::new();
        let ok_key = QueryKey::new("ok");
        let err_key = QueryKey::new("err");
        let mut observer = QueriesObserver::new(
            &client,
            vec![ok_query(&ok_key, "good"), failing_query(&err_key)],
        );
        await_all(observer.run_all()).await;
        observer.sync();

        let successes = observer.combine(|states| {
            states
                .iter()
                .filter(|state| state.is_success())
                .cloned()
                .collect::<Vec<_>>()
        });
        assert_eq!(successes.len(), 1);
        assert_eq!(successes[0].data(), Some(&"good".to_string()));

        let total = observer.combine(<[QueryState<String>]>::len);
        assert_eq!(total, 2);
    }

    #[tokio::test]
    async fn test_results_length_matches_specs() {
        let client = QueryClient::new();
        let mut observer = QueriesObserver::<String>::new(&client, Vec::new());
        assert!(observer.is_empty());
        assert_eq!(observer.results().len(), 0);

        observer.set_specs(vec![
            ok_query(&QueryKey::new("a"), "A"),
            ok_query(&QueryKey::new("b"), "B"),
            ok_query(&QueryKey::new("c"), "C"),
        ]);
        assert_eq!(observer.len(), 3);
        assert_eq!(observer.results().len(), 3);
        assert!(!observer.is_empty());
    }

    #[tokio::test]
    async fn test_initial_result_seeds_initial_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("seeded");
        let observer = QueriesObserver::new(
            &client,
            vec![ok_query(&key, "seed").initial_data("seed".to_string())],
        );
        assert!(observer.results()[0].is_success());
        assert_eq!(observer.results()[0].data(), Some(&"seed".to_string()));
        // initial_data is persisted into the cache.
        assert_eq!(
            client.get_query_data::<String>(&key),
            Some("seed".to_string())
        );
    }

    #[tokio::test]
    async fn test_initial_result_reads_cached_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("cached");
        client.set_query_data(&key, "cached".to_string(), QueryOptions::default());
        let observer = QueriesObserver::new(&client, vec![ok_query(&key, "unused")]);
        assert!(observer.results()[0].is_success());
        assert_eq!(observer.results()[0].data(), Some(&"cached".to_string()));
    }

    #[tokio::test]
    async fn test_initial_result_reads_stale_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("stale");
        let options = crate::QueryOptions {
            stale_time: std::time::Duration::from_millis(1),
            ..crate::QueryOptions::default()
        };
        client.set_query_data(&key, "old".to_string(), options);
        std::thread::sleep(std::time::Duration::from_millis(5));

        let observer = QueriesObserver::new(&client, vec![ok_query(&key, "unused")]);
        assert!(observer.results()[0].is_stale());
    }

    #[tokio::test]
    async fn test_refresh_single_spec() {
        let client = QueryClient::new();
        let a_key = QueryKey::new("a");
        let b_key = QueryKey::new("b");
        let mut observer =
            QueriesObserver::new(&client, vec![ok_query(&a_key, "A"), failing_query(&b_key)]);

        let handle = observer.refresh(0).expect("index in bounds");
        let _ = handle.await;
        observer.sync();
        assert!(observer.results()[0].is_success());
        assert!(observer.results()[1].is_idle(), "only index 0 ran");

        assert!(observer.refresh(9).is_none(), "out of bounds");
    }

    #[tokio::test]
    async fn test_disabled_specs_are_skipped() {
        let client = QueryClient::new();
        let key = QueryKey::new("disabled");
        let disabled = ok_query(&key, "nope").enabled(false);
        let mut observer = QueriesObserver::new(&client, vec![disabled]);

        let handles = observer.run_all();
        assert!(handles.is_empty(), "disabled specs are not executed");
        assert!(observer.results()[0].is_idle(), "state left untouched");

        assert!(observer.refresh(0).is_none());
    }

    #[tokio::test]
    async fn test_specs_returns_specs_in_order() {
        let client = QueryClient::new();
        let keys = [QueryKey::new("x"), QueryKey::new("y")];
        let observer = QueriesObserver::new(
            &client,
            vec![ok_query(&keys[0], "X"), ok_query(&keys[1], "Y")],
        );
        let specs = observer.specs();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].key.cache_key(), "x");
        assert_eq!(specs[1].key.cache_key(), "y");
    }

    #[tokio::test]
    async fn test_run_indices_skips_out_of_bounds_index() {
        let client = QueryClient::new();
        let key = QueryKey::new("bounds");
        let mut observer = QueriesObserver::new(&client, vec![ok_query(&key, "A")]);

        let handles = observer.run_indices(&[7]);
        assert!(handles.is_empty(), "out-of-bounds indices are skipped");
        assert!(observer.results()[0].is_idle(), "state left untouched");
    }

    #[tokio::test]
    async fn test_run_indices_tolerates_missing_result_slot() {
        let client = QueryClient::new();
        let key = QueryKey::new("no-slot");
        let mut observer = QueriesObserver::new(&client, vec![ok_query(&key, "A")]);

        // Desynchronise results from specs: without a slot the optimistic
        // loading mark is skipped, but the execution still runs and `sync`
        // folds its outcome back in once a slot exists again.
        observer.results.clear();
        let handles = observer.run_indices(&[0]);
        assert_eq!(handles.len(), 1, "the spec is still executed");
        await_all(handles).await;

        observer.results.push(QueryState::Idle);
        observer.sync();
        assert!(observer.results()[0].is_success());
        assert_eq!(observer.results()[0].data(), Some(&"A".to_string()));
    }

    #[tokio::test]
    async fn test_sync_skips_poisoned_completion_slots() {
        let client = QueryClient::new();
        let key = QueryKey::new("poisoned");
        let mut observer = QueriesObserver::new(&client, vec![ok_query(&key, "A")]);

        // Poison the completion-slot lock: `sync` must skip the fold instead
        // of panicking.
        let completed = Arc::clone(&observer.completed);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = completed.lock().expect("lock before poisoning");
            panic!("poison the completed lock");
        }));

        observer.sync();
        assert!(
            observer.results()[0].is_idle(),
            "a poisoned lock leaves the results untouched"
        );
    }
}
