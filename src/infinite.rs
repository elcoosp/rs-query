// src/infinite.rs
//! Infinite query support for paginated data

use crate::options::QueryOptions;
use crate::query::FetchFuture;
use crate::{QueryError, QueryKey};
use std::future::Future;
use std::sync::Arc;

/// Thread-safe, cloneable function fetching one page for an
/// [`InfiniteQuery`], parameterized by the page parameter `P`.
///
/// # `TanStack` parity: `useInfiniteQuery`'s `queryFn`.
pub type PageFetchFn<T, P> = Arc<dyn Fn(P) -> FetchFuture<T> + Send + Sync>;

/// Thread-safe, cloneable function deriving the next/previous page parameter
/// from the last fetched page and all pages fetched so far.
///
/// # `TanStack` parity: `useInfiniteQuery`'s `getNextPageParam` /
/// `getPreviousPageParam`.
pub type PageParamFn<T, P> = Arc<dyn Fn(&T, &[T]) -> Option<P> + Send + Sync>;

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
    #[must_use]
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
    #[must_use]
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Get the last page if available.
    #[must_use]
    pub fn last_page(&self) -> Option<&T> {
        self.pages.last()
    }

    /// Get the first page if available.
    #[must_use]
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
    pub fetch_fn: PageFetchFn<T, P>,
    /// Initial page parameter.
    pub initial_page_param: P,
    /// Function to get the next page parameter.
    pub get_next_page_param: Option<PageParamFn<T, P>>,
    /// Function to get the previous page parameter.
    pub get_previous_page_param: Option<PageParamFn<T, P>>,
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
    #[must_use = "builder methods do nothing unless the result is used"]
    pub fn get_next_page_param<F>(mut self, f: F) -> Self
    where
        F: Fn(&T, &[T]) -> Option<P> + Send + Sync + 'static,
    {
        self.get_next_page_param = Some(Arc::new(f));
        self
    }

    /// Set the function to get the previous page parameter.
    #[must_use = "builder methods do nothing unless the result is used"]
    pub fn get_previous_page_param<F>(mut self, f: F) -> Self
    where
        F: Fn(&T, &[T]) -> Option<P> + Send + Sync + 'static,
    {
        self.get_previous_page_param = Some(Arc::new(f));
        self
    }

    /// Set maximum number of pages to keep.
    #[must_use = "builder methods do nothing unless the result is used"]
    pub fn max_pages(mut self, max: usize) -> Self {
        self.max_pages = Some(max);
        self
    }

    /// Set query options.
    #[must_use = "builder methods do nothing unless the result is used"]
    pub fn options(mut self, options: QueryOptions) -> Self {
        self.options = options;
        self
    }

    /// `TanStack` `hasNextPage`: whether `get_next_page_param` yields a
    /// parameter for the **last** cached page. `None` data, a missing
    /// `get_next_page_param`, or a `None` param all mean "no next page".
    pub fn has_next_page(&self, data: Option<&InfiniteData<T, P>>) -> bool {
        let Some(data) = data else {
            return false;
        };
        let Some(get_next) = self.get_next_page_param.as_ref() else {
            return false;
        };
        let Some(last_page) = data.last_page() else {
            return false;
        };
        get_next(last_page, &data.pages).is_some()
    }

    /// `TanStack` `hasPreviousPage`: like [`InfiniteQuery::has_next_page`] for
    /// the **first** cached page, additionally requiring
    /// `get_previous_page_param` to be configured.
    pub fn has_previous_page(&self, data: Option<&InfiniteData<T, P>>) -> bool {
        let Some(data) = data else {
            return false;
        };
        let Some(get_prev) = self.get_previous_page_param.as_ref() else {
            return false;
        };
        let Some(first_page) = data.first_page() else {
            return false;
        };
        get_prev(first_page, &data.pages).is_some()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Await the query's fetch closure once on a throwaway runtime so
    /// test-defined page fetchers are exercised even when the unit under test
    /// never drives them.
    fn fetch_once<T, P>(query: &InfiniteQuery<T, P>)
    where
        T: Clone + Send + Sync + 'static,
        P: Clone + Send + Sync + 'static,
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        assert!(
            rt.block_on((query.fetch_fn)(query.initial_page_param.clone()))
                .is_ok()
        );
    }

    #[test]
    fn test_infinite_data_new() {
        let data: InfiniteData<Vec<i32>, i32> = InfiniteData::new();
        assert_eq!(data.pages, [] as [Vec<i32>; 0]);
        assert_eq!(data.page_params, [] as [i32; 0]);
        assert_eq!(data.page_count(), 0);
    }

    #[test]
    fn test_infinite_data_default() {
        let data: InfiniteData<Vec<i32>, i32> = InfiniteData::default();
        assert_eq!(data.pages, [] as [Vec<i32>; 0]);
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
        let debug = format!("{data:?}");
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
        fetch_once(&query);

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
        fetch_once(&query);

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
        fetch_once(&query);

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
        fetch_once(&query);

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
        fetch_once(&query);

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

    // ----- has_next_page / has_previous_page -----

    #[test]
    fn test_has_next_page_true_then_false_at_boundary() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |param: i32| async move { Ok(vec![param]) },
            0,
        )
        .get_next_page_param(
            |last: &Vec<i32>, _all: &[Vec<i32>]| {
                if last[0] < 3 { Some(last[0] + 1) } else { None }
            },
        );
        fetch_once(&query);

        let mut data = InfiniteData::new();
        data.push_page(vec![1], 0);
        assert!(query.has_next_page(Some(&data)));

        data.push_page(vec![3], 3);
        assert!(
            !query.has_next_page(Some(&data)),
            "param is None => no next page"
        );
    }

    #[test]
    fn test_has_next_page_none_data_or_no_fn() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |param: i32| async move { Ok(vec![param]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(2));
        fetch_once(&query);

        assert!(!query.has_next_page(None), "no data => no next page");

        let bare: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |param: i32| async move { Ok(vec![param]) },
            0,
        );
        fetch_once(&bare);
        let data = InfiniteData::with_first_page(vec![1], 0);
        assert!(!bare.has_next_page(Some(&data)), "no get_next_page_param");
    }

    #[test]
    fn test_has_next_page_empty_pages() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |param: i32| async move { Ok(vec![param]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(2));
        fetch_once(&query);

        let data = InfiniteData::<Vec<i32>, i32>::new();
        assert!(!query.has_next_page(Some(&data)), "no last page => false");
    }

    #[test]
    fn test_has_previous_page_true_then_false() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |param: i32| async move { Ok(vec![param]) },
            0,
        )
        .get_previous_page_param(|first: &Vec<i32>, _all: &[Vec<i32>]| {
            if first[0] > 0 {
                Some(first[0] - 1)
            } else {
                None
            }
        });
        fetch_once(&query);

        let mut data = InfiniteData::new();
        data.push_page(vec![2], 1);
        assert!(query.has_previous_page(Some(&data)));

        data.pages.insert(0, vec![0]);
        data.page_params.insert(0, 0);
        assert!(
            !query.has_previous_page(Some(&data)),
            "param is None => no previous page"
        );
    }

    #[test]
    fn test_has_previous_page_requires_previous_fn_and_data() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |param: i32| async move { Ok(vec![param]) },
            0,
        )
        .get_next_page_param(|_last: &Vec<i32>, _all: &[Vec<i32>]| Some(2));
        fetch_once(&query);

        let data = InfiniteData::with_first_page(vec![1], 0);
        assert!(
            !query.has_previous_page(Some(&data)),
            "has_next_page_param alone is not enough"
        );
        assert!(!query.has_previous_page(None));
    }

    #[test]
    fn test_has_previous_page_with_empty_pages_returns_false() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |param: i32| async move { Ok(vec![param]) },
            0,
        )
        .get_previous_page_param(|_first: &Vec<i32>, _all: &[Vec<i32>]| Some(-1));

        let data = InfiniteData::<Vec<i32>, i32>::new();
        assert!(
            !query.has_previous_page(Some(&data)),
            "no first page => false"
        );

        // Drive the stored fetcher once so the async block behind it is
        // instantiated (the page logic itself is exercised elsewhere).
        let page =
            futures::executor::block_on((query.fetch_fn)(0)).expect("fetcher should succeed");
        assert_eq!(page, vec![0]);
    }

    #[test]
    fn test_infinite_data_partial_eq_compares_pages_and_params() {
        let base = InfiniteData::with_first_page(vec![1, 2], 0);
        assert_eq!(base, InfiniteData::with_first_page(vec![1, 2], 0));

        let different_pages = InfiniteData::with_first_page(vec![9, 9], 0);
        let different_params = InfiniteData::with_first_page(vec![1, 2], 7);
        assert_ne!(base, different_pages);
        assert_ne!(base, different_params);

        let mut longer = InfiniteData::with_first_page(vec![1, 2], 0);
        longer.push_page(vec![3], 1);
        assert_ne!(base, longer);
    }
}
