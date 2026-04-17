// src/hooks.rs
//! Reactive GPUI hooks for querying global activity state.
//!
//! These mirror TanStack Query's `useIsFetching` and `useIsMutating` hooks.
//! They return a [`gpui::Model<bool>`] that automatically updates when the
//! corresponding activity count changes.

use crate::client::{ActivityEvent, QueryClient};
use gpui::{Context, Model};

/// Returns a reactive [`Model<bool>`] that is `true` when at least one query
/// fetch is in flight.
///
/// # Example
///
/// ```ignore
/// let is_fetching = use_is_fetching(cx, &self.query_client);
/// // In your render method:
/// if *is_fetching.read(cx) {
///     // show spinner
/// }
/// ```
pub fn use_is_fetching<V: 'static>(cx: &mut Context<V>, client: &QueryClient) -> Model<bool> {
    let initial = client.is_fetching();
    let model = cx.new_model(|_| initial);
    let mut rx = client.subscribe_activity();
    let client = client.clone();

    cx.spawn(|model, mut cx| async move {
        while let Ok(event) = rx.recv().await {
            if let ActivityEvent::FetchingCountChanged(_) = event {
                let _ = model.update(&mut cx, |value, cx| {
                    *value = client.is_fetching();
                    cx.notify();
                });
            }
        }
    })
    .detach();

    model
}

/// Returns a reactive [`Model<bool>`] that is `true` when at least one
/// mutation is in flight.
///
/// # Example
///
/// ```ignore
/// let is_mutating = use_is_mutating(cx, &self.query_client);
/// // In your render method:
/// if *is_mutating.read(cx) {
///     // show mutation indicator
/// }
/// ```
pub fn use_is_mutating<V: 'static>(cx: &mut Context<V>, client: &QueryClient) -> Model<bool> {
    let initial = client.is_mutating();
    let model = cx.new_model(|_| initial);
    let mut rx = client.subscribe_activity();
    let client = client.clone();

    cx.spawn(|model, mut cx| async move {
        while let Ok(event) = rx.recv().await {
            if let ActivityEvent::MutatingCountChanged(_) = event {
                let _ = model.update(&mut cx, |value, cx| {
                    *value = client.is_mutating();
                    cx.notify();
                });
            }
        }
    })
    .detach();

    model
}

/// Returns a reactive [`Model<usize>`> with the exact count of in-flight fetches.
///
/// Use this instead of [`use_is_fetching`] when you need to display a count
/// (e.g., "3 queries loading…").
pub fn use_fetching_count<V: 'static>(cx: &mut Context<V>, client: &QueryClient) -> Model<usize> {
    let initial = client.fetching_count();
    let model = cx.new_model(|_| initial);
    let mut rx = client.subscribe_activity();
    let client = client.clone();

    cx.spawn(|model, mut cx| async move {
        while let Ok(event) = rx.recv().await {
            if let ActivityEvent::FetchingCountChanged(count) = event {
                let _ = model.update(&mut cx, |value, cx| {
                    *value = count;
                    cx.notify();
                });
            }
        }
    })
    .detach();

    model
}

/// Returns a reactive [`Model<usize>`> with the exact count of in-flight mutations.
pub fn use_mutating_count<V: 'static>(cx: &mut Context<V>, client: &QueryClient) -> Model<usize> {
    let initial = client.mutating_count();
    let model = cx.new_model(|_| initial);
    let mut rx = client.subscribe_activity();
    let client = client.clone();

    cx.spawn(|model, mut cx| async move {
        while let Ok(event) = rx.recv().await {
            if let ActivityEvent::MutatingCountChanged(count) = event {
                let _ = model.update(&mut cx, |value, cx| {
                    *value = count;
                    cx.notify();
                });
            }
        }
    })
    .detach();

    model
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QueryClient;

    // Note: Testing GPUI hooks requires a GPUI test environment.
    // These are basic sanity checks that verify the client-side counters
    // which the hooks depend on.

    #[test]
    fn test_is_fetching_reflects_counter() {
        let client = QueryClient::new();
        assert!(!client.is_fetching());
        client.inc_fetching();
        assert!(client.is_fetching());
        client.dec_fetching();
        assert!(!client.is_fetching());
    }

    #[test]
    fn test_is_mutating_reflects_counter() {
        let client = QueryClient::new();
        assert!(!client.is_mutating());
        client.inc_mutating();
        assert!(client.is_mutating());
        client.dec_mutating();
        assert!(!client.is_mutating());
    }

    #[test]
    fn test_fetching_count_exact() {
        let client = QueryClient::new();
        assert_eq!(client.fetching_count(), 0);
        client.inc_fetching();
        assert_eq!(client.fetching_count(), 1);
        client.inc_fetching();
        assert_eq!(client.fetching_count(), 2);
        client.dec_fetching();
        assert_eq!(client.fetching_count(), 1);
    }

    #[test]
    fn test_activity_events_emitted() {
        let client = QueryClient::new();
        let mut rx = client.subscribe_activity();

        client.inc_fetching();
        assert_eq!(
            rx.try_recv().unwrap(),
            ActivityEvent::FetchingCountChanged(1)
        );

        client.inc_mutating();
        assert_eq!(
            rx.try_recv().unwrap(),
            ActivityEvent::MutatingCountChanged(1)
        );

        client.dec_fetching();
        assert_eq!(
            rx.try_recv().unwrap(),
            ActivityEvent::FetchingCountChanged(0)
        );

        client.dec_mutating();
        assert_eq!(
            rx.try_recv().unwrap(),
            ActivityEvent::MutatingCountChanged(0)
        );
    }

    #[test]
    fn test_multiple_subscribers_receive_events() {
        let client = QueryClient::new();
        let mut rx1 = client.subscribe_activity();
        let mut rx2 = client.subscribe_activity();

        client.inc_fetching();

        assert_eq!(
            rx1.try_recv().unwrap(),
            ActivityEvent::FetchingCountChanged(1)
        );
        assert_eq!(
            rx2.try_recv().unwrap(),
            ActivityEvent::FetchingCountChanged(1)
        );
    }
}
