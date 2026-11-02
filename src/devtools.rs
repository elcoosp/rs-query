// src/devtools.rs
//! Headless devtools for rs-query.
//!
//! This is the portable replacement for the previous gpui-component panel:
//! everything the panel displayed (cache rows with variant/age/actions, a
//! timestamped timeline of state transitions, type-filtered search) is
//! exposed as plain Rust state plus client actions, so any UI framework —
//! gpui, leptos, dioxus, xilem, floem, or a terminal — can render it.
//!
//! Wire a client once with [`DevtoolsState::record_from_client`]; the
//! collector task runs on the client's [`QueryRuntime`](crate::QueryRuntime)
//! and appends a [`TimelineEvent`] for every cache notification and activity
//! change (wired in production now — the old panel's timeline poll was never
//! enabled).

use crate::QueryClient;
use crate::client::ActivityEvent;
use crate::observer::QueryStateVariant;
use crate::state::{FetchStatus, QueryStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// A cache-row snapshot as displayed by devtools.
#[derive(Clone, Debug)]
pub struct CacheRow {
    /// Cache key (segments joined with `::`).
    pub cache_key: String,
    /// Whether the entry is marked stale.
    pub is_stale: bool,
    /// When the data was last written.
    pub fetched_at: Instant,
    /// Aggregate status (`success` while data exists — the type-erased cache
    /// stores no error states).
    pub status: QueryStatus,
    /// Whether a fetch for this key is in flight.
    pub in_flight: bool,
}

/// Kind of a [`TimelineEvent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineKind {
    /// The key entered the loading state.
    Loading,
    /// A background refetch started for the key.
    Refetching,
    /// The key resolved with fresh data.
    Success,
    /// The key was marked stale (invalidation/focus/reconnect).
    Stale,
    /// The key surfaced an error or cancellation.
    Error,
    /// Fetching/mutating activity count changed.
    Activity,
}

impl TimelineKind {
    /// Stable label for filters and rendering.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::Refetching => "refetching",
            Self::Success => "success",
            Self::Stale => "stale",
            Self::Error => "error",
            Self::Activity => "activity",
        }
    }

    /// Every kind, in display order (devtools type filters).
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::Loading,
            Self::Refetching,
            Self::Success,
            Self::Stale,
            Self::Error,
            Self::Activity,
        ]
    }
}

/// One timeline row: what happened, to which key, when.
#[derive(Clone, Debug)]
pub struct TimelineEvent {
    /// Epoch milliseconds when the event was recorded.
    pub timestamp_ms: u64,
    /// Affected cache key (empty for activity events).
    pub cache_key: String,
    /// What happened.
    pub kind: TimelineKind,
    /// For activity events: the new global fetching count.
    pub fetching_count: Option<usize>,
    /// For activity events: the new global mutating count.
    pub mutating_count: Option<usize>,
}

/// Timeline type filter: keep everything, or only the selected kinds, with an
/// optional case-insensitive key substring search.
#[derive(Clone, Debug, Default)]
pub struct TimelineFilter {
    /// When `None`, all kinds pass; otherwise only listed kinds pass.
    pub kinds: Option<Vec<TimelineKind>>,
    /// Case-insensitive substring match on the cache key.
    pub search: Option<String>,
}

impl TimelineFilter {
    /// Whether `event` passes this filter.
    #[must_use]
    pub fn matches(&self, event: &TimelineEvent) -> bool {
        if let Some(kinds) = &self.kinds
            && !kinds.contains(&event.kind)
        {
            return false;
        }
        if let Some(search) = &self.search
            && !event
                .cache_key
                .to_lowercase()
                .contains(&search.to_lowercase())
        {
            return false;
        }
        true
    }
}

/// Headless devtools state over one [`QueryClient`].
pub struct DevtoolsState {
    client: QueryClient,
    timeline: Arc<Mutex<Vec<TimelineEvent>>>,
    filter: Mutex<TimelineFilter>,
    capacity: usize,
    /// Whether a collector is currently running.
    recording: Arc<AtomicBool>,
    /// Cooperatively stops the running collector.
    stop: Arc<AtomicBool>,
}

impl std::fmt::Debug for DevtoolsState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevtoolsState")
            .field("timeline_len", &self.timeline_len())
            .finish_non_exhaustive()
    }
}

impl DevtoolsState {
    /// Devtools over `client` with the default timeline capacity (512).
    #[must_use]
    pub fn new(client: QueryClient) -> Self {
        Self::with_capacity(client, 512)
    }

    /// Devtools over `client` with a bounded timeline (oldest dropped).
    #[must_use]
    pub fn with_capacity(client: QueryClient, capacity: usize) -> Self {
        Self {
            client,
            timeline: Arc::new(Mutex::new(Vec::new())),
            filter: Mutex::new(TimelineFilter::default()),
            capacity: capacity.max(1),
            recording: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The observed client.
    #[must_use]
    pub fn client(&self) -> &QueryClient {
        &self.client
    }

    /// Start recording cache notifications into the timeline (idempotent).
    ///
    /// The collector runs on the client's runtime and keeps running until
    /// [`DevtoolsState::stop_recording`] or the client is dropped.
    pub fn record_from_client(&self) {
        if self.recording.swap(true, Ordering::SeqCst) {
            return;
        }
        self.stop.store(false, Ordering::SeqCst);
        let mut updates = self.client.subscribe_all();
        let mut activity = self.client.subscribe_activity();
        let timeline = Arc::clone(&self.timeline);
        let stop = Arc::clone(&self.stop);
        let capacity = self.capacity;
        let task = async move {
            loop {
                let update_fut = updates.recv();
                let activity_fut = activity.recv();
                futures::pin_mut!(update_fut, activity_fut);
                let event = match futures::future::select(activity_fut, update_fut).await {
                    // Activity first: cheap, coarse events never starve.
                    futures::future::Either::Left((incoming, _)) => match incoming {
                        Ok(ActivityEvent::FetchingCountChanged(count)) => TimelineEvent {
                            timestamp_ms: now_ms(),
                            cache_key: String::new(),
                            kind: TimelineKind::Activity,
                            fetching_count: Some(count),
                            mutating_count: None,
                        },
                        Ok(ActivityEvent::MutatingCountChanged(count)) => TimelineEvent {
                            timestamp_ms: now_ms(),
                            cache_key: String::new(),
                            kind: TimelineKind::Activity,
                            fetching_count: None,
                            mutating_count: Some(count),
                        },
                        Err(crate::bus::RecvError::Lagged(_)) => continue,
                        Err(crate::bus::RecvError::Closed) => break,
                    },
                    futures::future::Either::Right((incoming, _)) => match incoming {
                        Ok(update) => TimelineEvent {
                            timestamp_ms: now_ms(),
                            cache_key: update.key.clone(),
                            kind: map_variant(update.state_variant),
                            fetching_count: None,
                            mutating_count: None,
                        },
                        Err(crate::bus::RecvError::Lagged(_)) => continue,
                        Err(crate::bus::RecvError::Closed) => break,
                    },
                };
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                push_capped_event(&timeline, event, capacity);
            }
        };
        let handle = self.client.runtime().spawn(Box::pin(task));
        handle.detach();
    }

    /// Stop the collector started by [`DevtoolsState::record_from_client`].
    pub fn stop_recording(&self) {
        self.recording.store(false, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Whether the collector is running.
    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::SeqCst)
    }

    /// Snapshot of every cached query (the Queries tab rows).
    #[must_use]
    pub fn cache_rows(&self) -> Vec<CacheRow> {
        self.client
            .cache
            .iter()
            .map(|entry| {
                let cache_key = entry.key().clone();
                let in_flight = self
                    .client
                    .is_in_flight(&crate::QueryKey::from_segments(cache_key.split("::")));
                CacheRow {
                    cache_key,
                    is_stale: entry.is_stale,
                    fetched_at: entry.fetched_at,
                    status: QueryStatus::Success,
                    in_flight,
                }
            })
            .collect()
    }

    /// Timeline rows (oldest first) after the current filter.
    #[must_use]
    pub fn timeline(&self) -> Vec<TimelineEvent> {
        let filter = self
            .filter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        self.timeline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|event| filter.matches(event))
            .cloned()
            .collect()
    }

    /// Number of recorded events (before filtering).
    #[must_use]
    pub fn timeline_len(&self) -> usize {
        self.timeline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Replace the timeline filter.
    pub fn set_filter(&self, filter: TimelineFilter) {
        *self
            .filter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = filter;
    }

    /// Current timeline filter.
    #[must_use]
    pub fn filter(&self) -> TimelineFilter {
        self.filter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Clear the timeline.
    pub fn clear_timeline(&self) {
        self.timeline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Refetch action for one cache row (the panel's per-row button).
    pub fn refetch(&self, cache_key: &str) {
        self.client
            .invalidate_queries(&crate::QueryKey::from_segments(cache_key.split("::")));
    }

    /// Invalidate action for one cache row.
    pub fn invalidate(&self, cache_key: &str) {
        self.client
            .invalidate_queries(&crate::QueryKey::from_segments(cache_key.split("::")));
    }

    /// Remove action for one cache row.
    pub fn remove(&self, cache_key: &str) {
        let _ = self
            .client
            .remove_queries(&crate::filters::QueryFilters::with_key(
                crate::QueryKey::from_segments(cache_key.split("::")),
            ));
    }
}

/// Current wall-clock time in milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn push_capped_event(timeline: &Mutex<Vec<TimelineEvent>>, event: TimelineEvent, capacity: usize) {
    let mut guard = timeline
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if guard.len() >= capacity {
        guard.remove(0);
    }
    guard.push(event);
}

fn map_variant(variant: QueryStateVariant) -> TimelineKind {
    match variant {
        QueryStateVariant::Idle | QueryStateVariant::Stale => TimelineKind::Stale,
        QueryStateVariant::Loading => TimelineKind::Loading,
        QueryStateVariant::Refetching => TimelineKind::Refetching,
        QueryStateVariant::Success => TimelineKind::Success,
        QueryStateVariant::Error => TimelineKind::Error,
    }
}

/// Aggregate fetch status for a row (the panel's badge column).
#[must_use]
pub fn row_fetch_status(in_flight: bool) -> FetchStatus {
    if in_flight {
        FetchStatus::Fetching
    } else {
        FetchStatus::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::QueryKey;
    use crate::query::Query;
    use std::time::Duration;

    fn client() -> QueryClient {
        crate::test_support::tokio_client()
    }

    #[test]
    fn filter_matches_kind_and_search() {
        let event = TimelineEvent {
            timestamp_ms: 1,
            cache_key: "todos::user=1".to_string(),
            kind: TimelineKind::Success,
            fetching_count: None,
            mutating_count: None,
        };
        let all = TimelineFilter::default();
        assert!(all.matches(&event));
        let kinds = TimelineFilter {
            kinds: Some(vec![TimelineKind::Error]),
            search: None,
        };
        assert!(!kinds.matches(&event));
        let search = TimelineFilter {
            kinds: None,
            search: Some("user=1".to_string()),
        };
        assert!(search.matches(&event));
    }

    #[tokio::test]
    async fn timeline_records_transitions() {
        let client = client();
        let key = QueryKey::new("devtools-timeline");
        let query = Query::new(key.clone(), || async { Ok("data".to_string()) });
        let devtools = DevtoolsState::new(client.clone());
        assert_eq!(devtools.timeline_len(), 0);
        devtools.record_from_client();

        let state =
            crate::executor::execute_query(&client, &query, quoin_core::CancellationToken::new())
                .await;
        assert!(state.is_success());

        // Wait for the collector to observe the notifications.
        let deadline = Instant::now() + Duration::from_secs(2);
        while devtools.timeline_len() == 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(devtools.timeline_len() > 0, "timeline must record events");
        let events = devtools.timeline();
        assert!(
            events
                .iter()
                .any(|event| event.kind == TimelineKind::Loading)
        );
        assert!(
            events
                .iter()
                .any(|event| event.kind == TimelineKind::Success)
        );

        devtools.stop_recording();
    }

    #[test]
    fn cache_rows_reflect_cache_and_actions() {
        let client = client();
        let key = QueryKey::new("devtools-rows");
        client.set_query_data(&key, "value".to_string(), crate::QueryOptions::default());
        let devtools = DevtoolsState::new(client.clone());
        let rows = devtools.cache_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cache_key, "devtools-rows");
        assert!(!rows[0].is_stale);

        devtools.invalidate("devtools-rows");
        assert!(devtools.cache_rows()[0].is_stale);

        devtools.remove("devtools-rows");
        assert!(devtools.cache_rows().is_empty());
    }

    #[test]
    fn timeline_capacity_is_bounded() {
        let client = client();
        let _ = client;
        let timeline = Arc::new(Mutex::new(Vec::new()));
        for i in 0..10 {
            push_capped_event(
                &timeline,
                TimelineEvent {
                    timestamp_ms: i,
                    cache_key: "k".to_string(),
                    kind: TimelineKind::Success,
                    fetching_count: None,
                    mutating_count: None,
                },
                4,
            );
        }
        let guard = timeline.lock().expect("timeline lock");
        assert_eq!(guard.len(), 4);
        assert_eq!(guard[0].timestamp_ms, 6, "oldest dropped");
    }

    #[tokio::test]
    async fn activity_events_are_recorded() {
        let client = client();
        let devtools = DevtoolsState::new(client.clone());
        devtools.record_from_client();
        client.inc_fetching();
        client.dec_fetching();

        let deadline = Instant::now() + Duration::from_secs(2);
        while devtools.timeline_len() < 2 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let events = devtools.timeline();
        assert!(
            events.iter().any(
                |event| event.kind == TimelineKind::Activity && event.fetching_count == Some(1)
            )
        );
        devtools.stop_recording();
    }

    #[test]
    fn kind_labels_and_list_are_stable() {
        assert_eq!(TimelineKind::Success.as_str(), "success");
        assert_eq!(TimelineKind::all().len(), 6);
    }

    #[test]
    fn row_fetch_status_maps_in_flight() {
        assert_eq!(row_fetch_status(true), FetchStatus::Fetching);
        assert_eq!(row_fetch_status(false), FetchStatus::Idle);
    }

    #[tokio::test]
    async fn record_from_client_is_idempotent() {
        let client = client();
        let devtools = DevtoolsState::new(client.clone());
        devtools.record_from_client();
        devtools.record_from_client();
        client.inc_fetching();
        let deadline = Instant::now() + Duration::from_secs(2);
        while devtools.timeline_len() == 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // Only one collector: exactly one activity event, no duplicates.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(devtools.timeline_len(), 1);
        devtools.stop_recording();
    }
}
