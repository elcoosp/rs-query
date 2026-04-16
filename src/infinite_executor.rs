// src/infinite_executor.rs
//! Infinite query execution

use crate::infinite::{InfiniteData, InfiniteQuery};
use crate::observer::{QueryObserver, QueryStateVariant};
use crate::{QueryClient, QueryOptions, QueryState};
use tokio::sync::oneshot;

/// Observer for an infinite query with fetch-next/previous capabilities.
pub struct InfiniteQueryObserver<T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static>
{
    pub observer: QueryObserver<InfiniteData<T, P>>,
    query: std::sync::Arc<InfiniteQuery<T, P>>,
    next_page_tx: oneshot::Sender<()>,
    prev_page_tx: Option<oneshot::Sender<()>>,
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
    callback: impl FnOnce(&mut V, QueryState<InfiniteData<T, P>>, &mut gpui::Context<V>)
        + Send
        + 'static,
) -> InfiniteQueryObserver<T, P> {
    let key = query.key.clone();
    let options = query.options.clone();
    let initial_param = query.initial_page_param.clone();
    let fetch_fn = query.fetch_fn.clone();
    let get_next_fn = query.get_next_page_param.clone();
    let get_prev_fn = query.get_previous_page_param.clone();
    let max_pages = query.max_pages;
    let client = client.clone();

    let mut observer = QueryObserver::with_options(&client, key.clone(), options.clone());

    let (next_page_tx, mut next_page_rx) = oneshot::channel::<()>();
    let (prev_page_tx, mut prev_page_rx) = oneshot::channel::<()>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            observer.set_loading();
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
            observer.update();

            loop {
                tokio::select! {
                    _ = &mut next_page_rx => {
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
                                                observer.update();
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
                    _ = &mut prev_page_rx => {
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
                                                observer.update();
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
        query: std::sync::Arc::new(query.clone()),
        next_page_tx,
        prev_page_tx: Some(prev_page_tx),
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
            next_page_tx: oneshot::channel().0,
            prev_page_tx: None,
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
            next_page_tx: oneshot::channel().0,
            prev_page_tx: None,
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
            next_page_tx: oneshot::channel().0,
            prev_page_tx: None,
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
            next_page_tx: oneshot::channel().0,
            prev_page_tx: None,
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
            next_page_tx: oneshot::channel().0,
            prev_page_tx: None,
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
            next_page_tx: oneshot::channel().0,
            prev_page_tx: None,
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
            next_page_tx: oneshot::channel().0,
            prev_page_tx: None,
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
            next_page_tx: oneshot::channel().0,
            prev_page_tx: None,
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
            next_page_tx: oneshot::channel().0,
            prev_page_tx: None,
        };

        assert!(infinite_observer.state().is_idle());
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
    async fn test_oneshot_channel_next_page() {
        let (tx, rx) = oneshot::channel::<()>();
        tx.send(()).unwrap();
        let result = rx.await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_oneshot_channel_dropped() {
        let (tx, rx) = oneshot::channel::<()>();
        drop(tx);
        let result = rx.await;
        assert!(result.is_err());
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
}
