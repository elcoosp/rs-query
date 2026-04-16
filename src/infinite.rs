// src/infinite.rs
//! Infinite query types for pagination

use crate::{QueryError, QueryKey, QueryOptions};
use std::pin::Pin;

/// Data structure for infinite/paginated queries.
#[derive(Clone, Debug)]
pub struct InfiniteData<T, P> {
    pub pages: Vec<T>,
    pub page_params: Vec<P>,
}

impl<T, P> InfiniteData<T, P> {
    pub fn new() -> Self {
        Self {
            pages: Vec::new(),
            page_params: Vec::new(),
        }
    }

    pub fn last_page(&self) -> Option<&T> {
        self.pages.last()
    }

    pub fn first_page(&self) -> Option<&T> {
        self.pages.first()
    }

    pub fn last_page_param(&self) -> Option<&P> {
        self.page_params.last()
    }

    pub fn first_page_param(&self) -> Option<&P> {
        self.page_params.first()
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    pub fn push_page(&mut self, page: T, param: P) {
        self.pages.push(page);
        self.page_params.push(param);
    }

    pub fn prepend_page(&mut self, page: T, param: P) {
        self.pages.insert(0, page);
        self.page_params.insert(0, param);
    }

    pub fn pop_first_page(&mut self) -> Option<(T, P)> {
        if self.pages.is_empty() {
            return None;
        }
        let page = self.pages.remove(0);
        let param = self.page_params.remove(0);
        Some((page, param))
    }

    pub fn pop_last_page(&mut self) -> Option<(T, P)> {
        if self.pages.is_empty() {
            return None;
        }
        let page = self.pages.pop().unwrap();
        let param = self.page_params.pop().unwrap();
        Some((page, param))
    }
}

impl<T, P> Default for InfiniteData<T, P> {
    fn default() -> Self {
        Self::new()
    }
}

/// Definition of an infinite query.
pub struct InfiniteQuery<T, P> {
    pub key: QueryKey,
    pub fetch_fn: Box<
        dyn Fn(P) -> Pin<Box<dyn std::future::Future<Output = Result<T, QueryError>> + Send>>
            + Send
            + Sync,
    >,
    pub initial_page_param: P,
    pub get_next_page_param: Option<Box<dyn Fn(&T, &[T]) -> Option<P> + Send + Sync>>,
    pub get_previous_page_param: Option<Box<dyn Fn(&T, &[T]) -> Option<P> + Send + Sync>>,
    pub max_pages: Option<usize>,
    pub options: QueryOptions,
}

impl<T: Send + Sync + 'static, P: Clone + Send + Sync + 'static> InfiniteQuery<T, P> {
    pub fn new<F, Fut>(key: QueryKey, fetch_fn: F, initial_page_param: P) -> Self
    where
        F: Fn(P) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<T, QueryError>> + Send + 'static,
    {
        Self {
            key,
            fetch_fn: Box::new(move |param| Box::pin(fetch_fn(param))),
            initial_page_param,
            get_next_page_param: None,
            get_previous_page_param: None,
            max_pages: None,
            options: QueryOptions::default(),
        }
    }

    pub fn get_next_page_param<F>(mut self, f: F) -> Self
    where
        F: Fn(&T, &[T]) -> Option<P> + Send + Sync + 'static,
    {
        self.get_next_page_param = Some(Box::new(f));
        self
    }

    pub fn get_previous_page_param<F>(mut self, f: F) -> Self
    where
        F: Fn(&T, &[T]) -> Option<P> + Send + Sync + 'static,
    {
        self.get_previous_page_param = Some(Box::new(f));
        self
    }

    pub fn max_pages(mut self, max: usize) -> Self {
        self.max_pages = Some(max);
        self
    }

    pub fn options(mut self, options: QueryOptions) -> Self {
        self.options = options;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_infinite_data_new() {
        let data: InfiniteData<String, i32> = InfiniteData::new();
        assert!(data.is_empty());
        assert_eq!(data.page_count(), 0);
    }

    #[test]
    fn test_infinite_data_default() {
        let data: InfiniteData<String, i32> = InfiniteData::default();
        assert!(data.is_empty());
    }

    #[test]
    fn test_infinite_data_push_and_accessors() {
        let mut data: InfiniteData<String, i32> = InfiniteData::new();
        data.push_page("page1".to_string(), 0);
        data.push_page("page2".to_string(), 1);
        data.push_page("page3".to_string(), 2);
        assert_eq!(data.page_count(), 3);
        assert_eq!(data.first_page(), Some(&"page1".to_string()));
        assert_eq!(data.last_page(), Some(&"page3".to_string()));
        assert_eq!(data.first_page_param(), Some(&0));
        assert_eq!(data.last_page_param(), Some(&2));
    }

    #[test]
    fn test_infinite_data_prepend() {
        let mut data: InfiniteData<String, i32> = InfiniteData::new();
        data.push_page("page2".to_string(), 1);
        data.prepend_page("page1".to_string(), 0);
        assert_eq!(data.page_count(), 2);
        assert_eq!(data.first_page(), Some(&"page1".to_string()));
        assert_eq!(data.last_page(), Some(&"page2".to_string()));
    }

    #[test]
    fn test_infinite_data_pop_first() {
        let mut data: InfiniteData<String, i32> = InfiniteData::new();
        data.push_page("page1".to_string(), 0);
        data.push_page("page2".to_string(), 1);
        let (page, param) = data.pop_first_page().unwrap();
        assert_eq!(page, "page1".to_string());
        assert_eq!(param, 0);
        assert_eq!(data.page_count(), 1);
    }

    #[test]
    fn test_infinite_data_pop_last() {
        let mut data: InfiniteData<String, i32> = InfiniteData::new();
        data.push_page("page1".to_string(), 0);
        data.push_page("page2".to_string(), 1);
        let (page, param) = data.pop_last_page().unwrap();
        assert_eq!(page, "page2".to_string());
        assert_eq!(param, 1);
        assert_eq!(data.page_count(), 1);
    }

    #[test]
    fn test_infinite_data_pop_first_empty() {
        let mut data: InfiniteData<String, i32> = InfiniteData::new();
        assert!(data.pop_first_page().is_none());
    }

    #[test]
    fn test_infinite_data_pop_last_empty() {
        let mut data: InfiniteData<String, i32> = InfiniteData::new();
        assert!(data.pop_last_page().is_none());
    }

    #[test]
    fn test_infinite_data_accessors_empty() {
        let data: InfiniteData<String, i32> = InfiniteData::new();
        assert!(data.last_page().is_none());
        assert!(data.first_page().is_none());
        assert!(data.last_page_param().is_none());
        assert!(data.first_page_param().is_none());
    }

    #[test]
    fn test_infinite_data_clone() {
        let mut data = InfiniteData::new();
        data.push_page("page1".to_string(), 0);
        data.push_page("page2".to_string(), 1);
        let cloned = data.clone();
        assert_eq!(cloned.page_count(), 2);
        assert_eq!(cloned.first_page(), Some(&"page1".to_string()));
    }

    #[test]
    fn test_infinite_data_debug() {
        let data = InfiniteData::<String, i32>::new();
        let debug = format!("{:?}", data);
        assert!(debug.contains("pages"));
    }

    #[test]
    fn test_infinite_query_new() {
        let query: InfiniteQuery<String, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |_param: i32| async move { Ok("result".to_string()) },
            0,
        );
        assert_eq!(query.key.cache_key(), "test");
        assert_eq!(query.initial_page_param, 0);
        assert!(query.get_next_page_param.is_none());
        assert!(query.get_previous_page_param.is_none());
        assert!(query.max_pages.is_none());
    }

    #[test]
    fn test_infinite_query_builder() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |_param: i32| async move { Ok(vec![1, 2, 3]) },
            0,
        )
        .get_next_page_param(|_last, _all| Some(1))
        .get_previous_page_param(|_first, _all| Some(-1))
        .max_pages(5);

        assert!(query.get_next_page_param.is_some());
        assert!(query.get_previous_page_param.is_some());
        assert_eq!(query.max_pages, Some(5));
    }

    #[test]
    fn test_infinite_query_with_options() {
        let opts = QueryOptions {
            stale_time: Duration::from_secs(60),
            ..Default::default()
        };
        let query: InfiniteQuery<String, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |_param: i32| async move { Ok("data".to_string()) },
            0,
        )
        .options(opts.clone());

        assert_eq!(query.options.stale_time, opts.stale_time);
    }

    #[tokio::test]
    async fn test_infinite_query_fetch_fn() {
        let query: InfiniteQuery<String, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |param: i32| async move { Ok(format!("page_{}", param)) },
            0,
        );

        let result = (query.fetch_fn)(0).await.unwrap();
        assert_eq!(result, "page_0");

        let result = (query.fetch_fn)(1).await.unwrap();
        assert_eq!(result, "page_1");
    }

    #[test]
    fn test_infinite_query_get_next_page_param_some() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |_param: i32| async move { Ok(vec![1]) },
            0,
        )
        .get_next_page_param(|_last, _all| Some(42));

        let f = query.get_next_page_param.as_ref().unwrap();
        assert_eq!(f(&vec![1], &[vec![1]]), Some(42));
    }

    #[test]
    fn test_infinite_query_get_next_page_param_none_result() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |_param: i32| async move { Ok(vec![1]) },
            0,
        )
        .get_next_page_param(|_last, _all| None);

        let f = query.get_next_page_param.as_ref().unwrap();
        assert_eq!(f(&vec![1], &[vec![1]]), None);
    }

    #[test]
    fn test_infinite_query_get_previous_page_param_some() {
        let query: InfiniteQuery<Vec<i32>, i32> = InfiniteQuery::new(
            QueryKey::new("test"),
            |_param: i32| async move { Ok(vec![1]) },
            0,
        )
        .get_previous_page_param(|_first, _all| Some(-1));

        let f = query.get_previous_page_param.as_ref().unwrap();
        assert_eq!(f(&vec![1], &[vec![1]]), Some(-1));
    }
}
