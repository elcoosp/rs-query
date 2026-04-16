// src/infinite_executor.rs
//! Infinite query execution

use crate::infinite::{InfiniteData, InfiniteQuery};
use crate::observer::{QueryObserver, QueryStateVariant};
use crate::{QueryClient, QueryState};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Observer for an infinite query with fetch-next/previous capabilities.
pub struct InfiniteQueryObserver<T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static>
{
    pub observer: QueryObserver<InfiniteData<T, P>>,
    query: Arc<InfiniteQuery<T, P>>,
    next_page_tx: mpsc::Sender<()>,
    prev_page_tx: mpsc::Sender<()>,
}

impl<T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static>
    InfiniteQueryObserver<T, P>
{
    pub fn state(&self) -> &QueryState<InfiniteData<T, P>> {
        self.observer.state()
    }

    pub fn has_next_page(&self) -> bool {
        if let Some(data) = self.observer.state().data() {
            if let Some(ref get_next) = self.query.get_next_page_param {
                if let Some(last_page) = data.last_page() {
                    return get_next(last_page, &data.pages).is_some();
                }
            }
        }
        false
    }

    pub fn has_previous_page(&self) -> bool {
        if let Some(data) = self.observer.state().data() {
            if let Some(ref get_prev) = self.query.get_previous_page_param {
                if let Some(first_page) = data.first_page() {
                    return get_prev(first_page, &data.pages).is_some();
                }
            }
        }
        false
    }

    /// Request fetching the next page.
    /// If a fetch is already in progress or the channel is full, the request is ignored.
    pub fn fetch_next_page(&self) {
        let _ = self.next_page_tx.try_send(());
    }

    /// Request fetching the previous page.
    /// If a fetch is already in progress or the channel is full, the request is ignored.
    pub fn fetch_previous_page(&self) {
        let _ = self.prev_page_tx.try_send(());
    }
}

/// Execute an infinite query.
pub fn spawn_infinite_query<
    T: Clone + Send + Sync + 'static,
    P: Clone + Send + Sync + 'static,
    V: 'static,
>(
    _cx: &mut gpui::Context<V>,
    client: &QueryClient,
    query: &InfiniteQuery<T, P>,
    _callback: impl FnOnce(&mut V, QueryState<InfiniteData<T, P>>, &mut gpui::Context<V>)
        + Send
        + 'static,
) -> InfiniteQueryObserver<T, P> {
    let key = query.key.clone();
    let options = query.options.clone();
    let initial_param = query.initial_page_param.clone();
    let fetch_fn = Arc::clone(&query.fetch_fn);
    let get_next_fn = query.get_next_page_param.clone();
    let get_prev_fn = query.get_previous_page_param.clone();
    let max_pages = query.max_pages;
    let client = client.clone();

    let observer = QueryObserver::with_options(&client, key.clone(), options.clone());
    let mut observer_thread = observer.clone();

    // Use mpsc channels to allow multiple page fetch requests
    let (next_page_tx, mut next_page_rx) = mpsc::channel::<()>(1);
    let (prev_page_tx, mut prev_page_rx) = mpsc::channel::<()>(1);

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            observer_thread.set_loading();
            let result = fetch_fn(initial_param.clone()).await;
            match result {
                Ok(page) => {
                    let mut initial_data = InfiniteData::new();
                    initial_data.push_page(page, initial_param);
                    client.set_infinite_data(&key, initial_data, options.clone());
                }
                Err(_) => {
                    client.notify_subscribers(key.cache_key(), QueryStateVariant::Error);
                }
            }
            observer_thread.update();

            loop {
                tokio::select! {
                    _ = next_page_rx.recv() => {
                        if client.is_in_flight(&key) { continue; }
                        let current_data = client.get_infinite_data::<T, P>(&key);
                        if let Some(data) = current_data {
                            if let Some(ref get_next) = get_next_fn {
                                if let Some(last_page) = data.last_page() {
                                    if let Some(next_param) = get_next(last_page, &data.pages) {
                                        let handle = tokio::spawn(async {}).abort_handle();
                                        client.set_abort_handle(&key, handle);
                                        match fetch_fn(next_param.clone()).await {
                                            Ok(page) => {
                                                client.append_infinite_page(&key, page, next_param, max_pages);
                                                observer_thread.update();
                                            }
                                            Err(_) => {
                                                client.notify_subscribers(key.cache_key(), QueryStateVariant::Error);
                                            }
                                        }
                                        client.clear_abort_handle(&key);
                                    }
                                }
                            }
                        }
                    }
                    _ = prev_page_rx.recv() => {
                        if client.is_in_flight(&key) { continue; }
                        let current_data = client.get_infinite_data::<T, P>(&key);
                        if let Some(data) = current_data {
                            if let Some(ref get_prev) = get_prev_fn {
                                if let Some(first_page) = data.first_page() {
                                    if let Some(prev_param) = get_prev(first_page, &data.pages) {
                                        let handle = tokio::spawn(async {}).abort_handle();
                                        client.set_abort_handle(&key, handle);
                                        match fetch_fn(prev_param.clone()).await {
                                            Ok(page) => {
                                                client.prepend_infinite_page(&key, page, prev_param, max_pages);
                                                observer_thread.update();
                                            }
                                            Err(_) => {
                                                client.notify_subscribers(key.cache_key(), QueryStateVariant::Error);
                                            }
                                        }
                                        client.clear_abort_handle(&key);
                                    }
                                }
                            }
                        }
                    }
                    else => break,
                }
            }
        });
    });

    InfiniteQueryObserver {
        observer,
        query: Arc::new(query.clone()),
        next_page_tx,
        prev_page_tx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QueryKey, QueryOptions};

    #[test]
    fn test_has_next_page_true() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(1));

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(infinite_observer.has_next_page());
    }

    #[test]
    fn test_has_next_page_false_when_no_fn() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_next_page());
    }

    #[test]
    fn test_has_next_page_false_when_fn_returns_none() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| None);

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_next_page());
    }

    #[test]
    fn test_has_next_page_false_when_no_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(1));

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_next_page());
    }

    #[test]
    fn test_has_previous_page_true() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_previous_page_param(|_first: &Vec<i32>, _all: &[Vec<i32>]| Some(-1));

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(infinite_observer.has_previous_page());
    }

    #[test]
    fn test_has_previous_page_false_when_no_fn() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_previous_page());
    }

    #[test]
    fn test_has_previous_page_false_when_no_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_previous_page_param(|_first: &Vec<i32>, _all: &[Vec<i32>]| Some(-1));

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_previous_page());
    }

    #[test]
    fn test_has_previous_page_false_when_fn_returns_none() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_previous_page_param(|_first: &Vec<i32>, _all: &[Vec<i32>]| None);

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_previous_page());
    }

    #[test]
    fn test_infinite_observer_state() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(infinite_observer.state().is_idle());
    }

    #[test]
    fn test_fetch_next_page_does_not_panic() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        );

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> =
            QueryObserver::new(&client, key.clone());

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        // Should not panic even without a background thread listening
        infinite_observer.fetch_next_page();
        infinite_observer.fetch_previous_page();
    }

    #[test]
    fn test_append_infinite_page() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let result = client.append_infinite_page(&key, vec![3, 4], 1, None);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![1, 2]);
        assert_eq!(result.pages[1], vec![3, 4]);
    }

    #[test]
    fn test_append_infinite_page_with_max_pages_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let mut data = InfiniteData::new();
        data.push_page(vec![1], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        client.append_infinite_page(&key, vec![2], 1, Some(2));

        let result = client.append_infinite_page(&key, vec![3], 2, Some(2));
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![2]);
        assert_eq!(result.pages[1], vec![3]);
    }

    #[test]
    fn test_append_infinite_page_with_max_pages_no_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let mut data = InfiniteData::new();
        data.push_page(vec![1], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let result = client.append_infinite_page(&key, vec![2], 1, Some(3));
        assert!(result.is_some());
        assert_eq!(result.unwrap().page_count(), 2);
    }

    #[test]
    fn test_append_infinite_page_no_existing_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let result = client.append_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_append_infinite_page_wrong_type() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "wrong_type".to_string(), QueryOptions::default());

        let result = client.append_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_prepend_infinite_page() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let mut data = InfiniteData::new();
        data.push_page(vec![3, 4], 1);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let result = client.prepend_infinite_page(&key, vec![1, 2], 0, None);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![1, 2]);
        assert_eq!(result.pages[1], vec![3, 4]);
    }

    #[test]
    fn test_prepend_infinite_page_with_max_pages_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let mut data = InfiniteData::new();
        data.push_page(vec![3], 2);
        client.set_infinite_data(&key, data, QueryOptions::default());

        client.prepend_infinite_page(&key, vec![2], 1, Some(2));
        let result = client.prepend_infinite_page(&key, vec![1], 0, Some(2));

        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.page_count(), 2);
        assert_eq!(result.pages[0], vec![1]);
        assert_eq!(result.pages[1], vec![2]);
    }

    #[test]
    fn test_prepend_infinite_page_with_max_pages_no_eviction() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let mut data = InfiniteData::new();
        data.push_page(vec![2], 1);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let result = client.prepend_infinite_page(&key, vec![1], 0, Some(3));
        assert!(result.is_some());
        assert_eq!(result.unwrap().page_count(), 2);
    }

    #[test]
    fn test_prepend_infinite_page_no_existing_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        let result = client.prepend_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_prepend_infinite_page_wrong_type() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");
        client.set_query_data(&key, "wrong_type".to_string(), QueryOptions::default());

        let result = client.prepend_infinite_page::<Vec<i32>, i32>(&key, vec![1], 0, None);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_mpsc_channel_send_recv() {
        let (tx, mut rx) = mpsc::channel::<()>(1);
        tx.send(()).await.unwrap();
        let result = rx.recv().await;
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_mpsc_channel_closed() {
        let (tx, mut rx) = mpsc::channel::<()>(1);
        drop(tx);
        let result = rx.recv().await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_mpsc_try_send() {
        let (tx, mut rx) = mpsc::channel::<()>(1);
        tx.try_send(()).unwrap();
        // Channel full, should fail
        assert!(tx.try_send(()).is_err());
        let result = rx.recv().await;
        assert!(result.is_some());
    }

    #[test]
    fn test_infinite_query_error_notification() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let mut data = InfiniteData::new();
        data.push_page(vec![1], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        client.notify_subscribers(key.cache_key(), QueryStateVariant::Error);

        let result: Option<InfiniteData<Vec<i32>, i32>> = client.get_infinite_data(&key);
        assert!(result.is_some());
    }
    #[test]
    fn test_infinite_observer_has_next_page_no_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(1));

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        // No data yet
        assert!(!infinite_observer.has_next_page());
        assert!(!infinite_observer.has_previous_page());
    }

    #[test]
    fn test_infinite_observer_has_previous_page_no_data() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_previous_page_param(|_first: &Vec<i32>, _all: &[Vec<i32>]| Some(-1));

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_previous_page());
    }

    #[test]
    fn test_infinite_observer_has_next_page_fn_returns_none() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| None);

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        // Fn exists but returns None
        assert!(!infinite_observer.has_next_page());
    }

    #[test]
    fn test_infinite_observer_has_previous_page_fn_returns_none() {
        let client = QueryClient::new();
        let key = QueryKey::new("test");

        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            key.clone(),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_previous_page_param(|_first: &Vec<i32>, _all: &[Vec<i32>]| None);

        let mut data = InfiniteData::new();
        data.push_page(vec![1, 2, 3], 0);
        client.set_infinite_data(&key, data, QueryOptions::default());

        let observer: QueryObserver<InfiniteData<Vec<i32>, i32>> = QueryObserver::new(&client, key);

        let infinite_observer = InfiniteQueryObserver {
            observer,
            query: std::sync::Arc::new(query),
            next_page_tx: mpsc::channel(1).0,
            prev_page_tx: mpsc::channel(1).0,
        };

        assert!(!infinite_observer.has_previous_page());
    }

    #[tokio::test]
    async fn test_fetch_next_page_channel_full() {
        let (tx, mut rx) = mpsc::channel::<()>(1);

        // Fill the channel
        tx.send(()).await.unwrap();

        // Should fail silently (channel full)
        assert!(tx.try_send(()).is_err());

        // Drain it
        let _ = rx.recv().await;

        // Now should succeed
        assert!(tx.try_send(()).is_ok());
    }

    #[tokio::test]
    async fn test_fetch_previous_page_channel_full() {
        let (tx, mut rx) = mpsc::channel::<()>(1);

        tx.send(()).await.unwrap();
        assert!(tx.try_send(()).is_err());

        let _ = rx.recv().await;
        assert!(tx.try_send(()).is_ok());
    }
}
