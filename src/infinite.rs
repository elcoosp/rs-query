// src/infinite.rs
//! Infinite query support for paginated data

use crate::options::QueryOptions;
use crate::{QueryError, QueryKey};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Data structure for infinite query results.
#[derive(Debug)]
pub struct InfiniteData<T, P> {
    /// All fetched pages.
    pub pages: Vec<T>,
    /// Page parameters for each page.
    pub page_params: Vec<P>,
}

impl<T: Clone, P: Clone> Clone for InfiniteData<T, P> {
    fn clone(&self) -> Self {
        Self {
            pages: self.pages.clone(),
            page_params: self.page_params.clone(),
        }
    }
}

impl<T: PartialEq, P: PartialEq> PartialEq for InfiniteData<T, P> {
    fn eq(&self, other: &Self) -> bool {
        self.pages == other.pages && self.page_params == other.page_params
    }
}

impl<T, P> InfiniteData<T, P> {
    /// Create new empty infinite data.
    pub fn new() -> Self {
        Self {
            pages: Vec::new(),
            page_params: Vec::new(),
        }
    }

    /// Create infinite data with the first page.
    pub fn with_first_page(page: T, param: P) -> Self {
        Self {
            pages: vec![page],
            page_params: vec![param],
        }
    }

    /// Add a page to the data.
    pub fn push_page(&mut self, page: T, param: P) {
        self.pages.push(page);
        self.page_params.push(param);
    }

    /// Get the number of pages.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Get the last page if available.
    pub fn last_page(&self) -> Option<&T> {
        self.pages.last()
    }

    /// Get the first page if available.
    pub fn first_page(&self) -> Option<&T> {
        self.pages.first()
    }
}

impl<T, P> Default for InfiniteData<T, P> {
    fn default() -> Self {
        Self::new()
    }
}

/// Definition of an infinite query.
pub struct InfiniteQuery<T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static> {
    /// Query key.
    pub key: QueryKey,
    /// Function to fetch a page.
    pub fetch_fn: Arc<
        dyn Fn(P) -> Pin<Box<dyn Future<Output = Result<T, QueryError>> + Send + Sync>>
            + Send
            + Sync,
    >,
    /// Initial page parameter.
    pub initial_page_param: P,
    /// Function to get the next page parameter.
    pub get_next_page_param: Option<Arc<dyn Fn(&T, &[T]) -> Option<P> + Send + Sync>>,
    /// Function to get the previous page parameter.
    pub get_previous_page_param: Option<Arc<dyn Fn(&T, &[T]) -> Option<P> + Send + Sync>>,
    /// Maximum number of pages to keep.
    pub max_pages: Option<usize>,
    /// Query options.
    pub options: QueryOptions,
}

impl<T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static> Clone
    for InfiniteQuery<T, P>
{
    fn clone(&self) -> Self {
        Self {
            key: self.key.clone(),
            fetch_fn: Arc::clone(&self.fetch_fn),
            initial_page_param: self.initial_page_param.clone(),
            get_next_page_param: self.get_next_page_param.clone(),
            get_previous_page_param: self.get_previous_page_param.clone(),
            max_pages: self.max_pages,
            options: self.options.clone(),
        }
    }
}

impl<T: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static> InfiniteQuery<T, P> {
    /// Create a new infinite query.
    pub fn new<F, Fut>(key: QueryKey, fetch_fn: F, initial_page_param: P) -> Self
    where
        F: Fn(P) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, QueryError>> + Send + Sync + 'static,
    {
        Self {
            key,
            fetch_fn: Arc::new(move |p: P| Box::pin(fetch_fn(p))),
            initial_page_param,
            get_next_page_param: None,
            get_previous_page_param: None,
            max_pages: None,
            options: QueryOptions::default(),
        }
    }

    /// Set the function to get the next page parameter.
    pub fn get_next_page_param<F>(mut self, f: F) -> Self
    where
        F: Fn(&T, &[T]) -> Option<P> + Send + Sync + 'static,
    {
        self.get_next_page_param = Some(Arc::new(f));
        self
    }

    /// Set the function to get the previous page parameter.
    pub fn get_previous_page_param<F>(mut self, f: F) -> Self
    where
        F: Fn(&T, &[T]) -> Option<P> + Send + Sync + 'static,
    {
        self.get_previous_page_param = Some(Arc::new(f));
        self
    }

    /// Set maximum number of pages to keep.
    pub fn max_pages(mut self, max: usize) -> Self {
        self.max_pages = Some(max);
        self
    }

    /// Set query options.
    pub fn options(mut self, options: QueryOptions) -> Self {
        self.options = options;
        self
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_infinite_data_new() {
        let data: InfiniteData<Vec<i32>, i32> = InfiniteData::new();
        assert!(data.pages.is_empty());
        assert!(data.page_params.is_empty());
        assert_eq!(data.page_count(), 0);
    }

    #[test]
    fn test_infinite_data_default() {
        let data: InfiniteData<Vec<i32>, i32> = InfiniteData::default();
        assert!(data.pages.is_empty());
    }

    #[test]
    fn test_infinite_data_with_first_page() {
        let data = InfiniteData::with_first_page(vec![1, 2, 3], 0);
        assert_eq!(data.page_count(), 1);
        assert_eq!(data.pages[0], vec![1, 2, 3]);
        assert_eq!(data.page_params[0], 0);
    }

    #[test]
    fn test_infinite_data_push_page() {
        let mut data: InfiniteData<Vec<i32>, i32> = InfiniteData::new();
        data.push_page(vec![1, 2], 0);
        data.push_page(vec![3, 4], 1);

        assert_eq!(data.page_count(), 2);
        assert_eq!(data.pages[0], vec![1, 2]);
        assert_eq!(data.pages[1], vec![3, 4]);
        assert_eq!(data.page_params, vec![0, 1]);
    }

    #[test]
    fn test_infinite_data_last_page() {
        let mut data: InfiniteData<Vec<i32>, i32> = InfiniteData::new();
        assert!(data.last_page().is_none());

        data.push_page(vec![1], 0);
        assert_eq!(data.last_page(), Some(&vec![1]));

        data.push_page(vec![2], 1);
        assert_eq!(data.last_page(), Some(&vec![2]));
    }

    #[test]
    fn test_infinite_data_first_page() {
        let mut data: InfiniteData<Vec<i32>, i32> = InfiniteData::new();
        assert!(data.first_page().is_none());

        data.push_page(vec![1], 0);
        assert_eq!(data.first_page(), Some(&vec![1]));

        data.push_page(vec![2], 1);
        assert_eq!(data.first_page(), Some(&vec![1]));
    }

    #[test]
    fn test_infinite_data_clone() {
        let data = InfiniteData::with_first_page(vec![1, 2, 3], 0);
        let cloned = data.clone();
        assert_eq!(cloned.page_count(), 1);
        assert_eq!(cloned.pages[0], vec![1, 2, 3]);
    }

    #[test]
    fn test_infinite_data_debug() {
        let data = InfiniteData::with_first_page(vec![1, 2, 3], 0);
        let debug = format!("{:?}", data);
        assert!(debug.contains("pages"));
    }

    #[tokio::test]
    async fn test_infinite_query_new() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |param: i32| async move { Ok(vec![param, param + 1]) },
            0,
        );
        let result = (query.fetch_fn)(0).await.unwrap();
        assert_eq!(result, vec![0, 1]);
    }

    #[tokio::test]
    async fn test_infinite_query_error() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |_param: i32| async move { Err(QueryError::network("fail")) },
            0,
        );
        let result = (query.fetch_fn)(0).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_infinite_query_clone() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |param: i32| async move { Ok(vec![param]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(1))
        .max_pages(10);

        let cloned = query.clone();
        assert_eq!(cloned.key.cache_key(), "test");
        assert_eq!(cloned.initial_page_param, 0);
        assert!(cloned.get_next_page_param.is_some());
        assert_eq!(cloned.max_pages, Some(10));
    }

    #[test]
    fn test_infinite_query_get_next_page_param() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |param: i32| async move { Ok(vec![param]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(1));

        assert!(query.get_next_page_param.is_some());
        let fn_get = query.get_next_page_param.as_ref().unwrap();
        let result = fn_get(&vec![1, 2, 3], &[vec![1, 2, 3]]);
        assert_eq!(result, Some(1));
    }

    #[test]
    fn test_infinite_query_get_previous_page_param() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |param: i32| async move { Ok(vec![param]) },
            0,
        )
        .get_previous_page_param(|_first: &Vec<i32>, _all: &[Vec<i32>]| Some(-1));

        assert!(query.get_previous_page_param.is_some());
        let fn_get = query.get_previous_page_param.as_ref().unwrap();
        let result = fn_get(&vec![1, 2, 3], &[vec![1, 2, 3]]);
        assert_eq!(result, Some(-1));
    }

    #[test]
    fn test_infinite_query_builder() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |param: i32| async move { Ok(vec![param]) },
            0,
        )
        .max_pages(5)
        .options(QueryOptions {
            stale_time: std::time::Duration::from_secs(60),
            ..Default::default()
        });

        assert_eq!(query.max_pages, Some(5));
        assert_eq!(query.options.stale_time, std::time::Duration::from_secs(60));
    }

    #[test]
    fn test_infinite_query_get_next_returns_none() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |param: i32| async move { Ok(vec![param]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| None);

        let fn_get = query.get_next_page_param.as_ref().unwrap();
        let result = fn_get(&vec![1], &[vec![1]]);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_infinite_query_fetch_fn_cloned_works() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);

        let query: InfiniteQuery<i32, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            move |param: i32| {
                let counter = Arc::clone(&counter_clone);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok(param)
                }
            },
            0,
        );

        let cloned = query.clone();

        (query.fetch_fn)(1).await.unwrap();
        (cloned.fetch_fn)(2).await.unwrap();

        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}
