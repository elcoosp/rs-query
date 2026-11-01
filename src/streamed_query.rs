// src/streamed_query.rs
//! Port of `TanStack` Query's `experimental_streamedQuery`.
//!
//! A streamed query turns an async stream of chunks into a single query
//! result. On the first fetch the chunks are accumulated into the result; on a
//! refetch (data already present) the [`RefetchMode`] decides what happens to
//! the previous value:
//!
//! * [`RefetchMode::Replace`] — collect the fresh stream and replace the old
//!   data once the stream completes.
//! * [`RefetchMode::Append`] — keep the previous data and append incoming
//!   chunks onto it.
//! * [`RefetchMode::Reset`] — `TanStack`'s default: clear the old value and
//!   start from an empty accumulator.
//!
//! Two deviations from the `TanStack` implementation, both dictated by the Rust
//! port's `Query` shape, are worth calling out:
//!
//! 1. **Cancellation**: `TanStack` wires an abort signal into the stream loop.
//!    Our `Query::fetch_fn` is a plain `Fn() -> Fut` without a token, so
//!    `streamed_query` itself is not cancel-aware; Query-level cancellation
//!    (see [`crate::cancellation::cancellable_fetch`] and the executor) wraps
//!    the produced future instead — aborting the wrapping task drops the
//!    future and with it the stream.
//! 2. **Previous data**: `TanStack` reads `query.state.data` inside the queryFn
//!    via its fetch context. Our fetch function receives no context, so the
//!    append/refetch base is the configured
//!    [`StreamedQueryConfig::initial_value`]. To append onto *live* cache
//!    data, compose [`collect_stream`] directly inside a custom fetch closure
//!    that reads the client cache first.
//!
//! Chunk combination: `TanStack` relies on a JS default reducer
//! (`(items, chunk) => [...items, chunk]`). In Rust the equivalent default is
//! expressed by the [`CombineChunks`] trait (implemented for `Vec` and
//! `String`); a custom reducer ([`StreamedQueryConfig::reducer`]) always takes
//! precedence. An empty stream that never accumulates a value yields
//! [`QueryError::UndefinedData`].

use crate::error::QueryError;
use futures::StreamExt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// How a refetch treats previously accumulated data, mirroring `TanStack`
/// `streamedQuery`'s `refetchMode` (default: [`RefetchMode::Reset`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RefetchMode {
    /// Collect the fresh stream locally and replace the previous data once the
    /// stream completes. At the data level the previous value never
    /// participates in the accumulation.
    Replace,
    /// Keep the previous data and append incoming chunks onto it.
    Append,
    /// `TanStack` default: clear the old value (start from an empty
    /// accumulator) before streaming. Resetting the *visible query state*
    /// before the stream starts is a Query-level concern outside of
    /// [`collect_stream`]; at the data level `Reset` and `Replace` accumulate
    /// identically.
    #[default]
    Reset,
}

/// Strategy for combining an accumulated value with a new stream chunk when no
/// custom reducer is configured (the Rust equivalent of `TanStack`'s default
/// `(items, chunk) => [...items, chunk]` reducer).
pub trait CombineChunks: Clone + Send + Sync + 'static {
    /// Combine the value accumulated so far with a new chunk, returning the
    /// new accumulator.
    fn combine(self, chunk: Self) -> Self;
}

impl<E: Clone + Send + Sync + 'static> CombineChunks for Vec<E> {
    fn combine(mut self, chunk: Self) -> Self {
        self.extend(chunk);
        self
    }
}

impl CombineChunks for String {
    fn combine(mut self, chunk: Self) -> Self {
        self.push_str(&chunk);
        self
    }
}

/// Reducer folding a chunk into the accumulated value: `(accumulator, chunk) -> new_accumulator`.
pub type StreamReducer<T> = Arc<dyn Fn(T, T) -> T + Send + Sync>;

/// Configuration for [`streamed_query`], mirroring the options object accepted
/// by `TanStack`'s `streamedQuery`.
#[derive(Clone)]
pub struct StreamedQueryConfig<T> {
    /// What happens to previous data on a refetch.
    pub refetch_mode: RefetchMode,
    /// Custom chunk reducer; overrides the [`CombineChunks`] default.
    pub reducer: Option<StreamReducer<T>>,
    /// Initial accumulator value. In `Append` mode this acts as the base onto
    /// which chunks are appended when no previous data is available (see the
    /// module docs for why this stands in for the live cache value).
    pub initial_value: Option<T>,
}

impl<T> Default for StreamedQueryConfig<T> {
    fn default() -> Self {
        Self {
            refetch_mode: RefetchMode::default(),
            reducer: None,
            initial_value: None,
        }
    }
}

impl<T> StreamedQueryConfig<T> {
    /// Configuration with `TanStack` defaults (`refetchMode: 'reset'`, default
    /// Vec-style reducer, no initial value).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the refetch mode.
    #[must_use]
    pub fn refetch_mode(mut self, mode: RefetchMode) -> Self {
        self.refetch_mode = mode;
        self
    }

    /// Set a custom chunk reducer `(accumulator, chunk) -> new_accumulator`.
    #[must_use]
    pub fn reducer<F>(mut self, reducer: F) -> Self
    where
        F: Fn(T, T) -> T + Send + Sync + 'static,
    {
        self.reducer = Some(Arc::new(reducer));
        self
    }

    /// Set the initial accumulator value (`TanStack` `initialValue`).
    #[must_use]
    pub fn initial_value(mut self, value: T) -> Self {
        self.initial_value = Some(value);
        self
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for StreamedQueryConfig<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("StreamedQueryConfig");
        debug
            .field("refetch_mode", &self.refetch_mode)
            .field("initial_value", &self.initial_value);
        if self.reducer.is_some() {
            debug.field("reducer", &"<fn>");
        }
        debug.finish()
    }
}

/// Accumulate a stream of chunks into a single result according to `mode`.
///
/// * `Append` starts from `previous` (keeping old data) and folds chunks in;
///   `Replace`/`Reset` start from an empty accumulator, ignoring `previous`.
/// * Each chunk is folded in via `reducer` when provided, else via the
///   [`CombineChunks`] default (Vec-style append).
/// * An error chunk is propagated immediately, stopping accumulation.
/// * A stream that produces no accumulated value (empty stream without
///   previous data) yields [`QueryError::UndefinedData`], mirroring the
///   "query data cannot be undefined" rule.
pub async fn collect_stream<T, S>(
    stream: S,
    mode: RefetchMode,
    previous: Option<T>,
    reducer: Option<StreamReducer<T>>,
) -> Result<T, QueryError>
where
    T: CombineChunks,
    S: futures::Stream<Item = Result<T, QueryError>>,
{
    let mut stream = std::pin::pin!(stream);
    let mut acc: Option<T> = match mode {
        RefetchMode::Append => previous,
        RefetchMode::Replace | RefetchMode::Reset => None,
    };

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        acc = Some(match acc {
            Some(current) => match &reducer {
                Some(reduce) => reduce(current, chunk),
                None => current.combine(chunk),
            },
            None => chunk,
        });
    }

    acc.ok_or(QueryError::UndefinedData)
}

/// Boxed future produced by [`streamed_query`], shaped to plug directly into
/// [`crate::query::Query::new`] (whose fetch futures must be `Send + Sync`).
pub type StreamedQueryFuture<T> =
    Pin<Box<dyn Future<Output = Result<T, QueryError>> + Send + Sync + 'static>>;

/// Build a `TanStack` `streamedQuery`-style query function from a stream factory.
///
/// The returned closure is a drop-in `fetch_fn` for
/// [`crate::query::Query::new`]: each invocation creates a fresh stream via
/// `stream_fn`, accumulates it with [`collect_stream`] under the configured
/// [`StreamedQueryConfig`], and yields the final value.
///
/// Cancellation is *not* handled inside the produced future; Query-level
/// cancellation (executor + `CancellationToken`) wraps it. See the module docs
/// for the previous-data caveat in `Append` mode.
pub fn streamed_query<T, F, S>(
    stream_fn: F,
    config: StreamedQueryConfig<T>,
) -> impl Fn() -> StreamedQueryFuture<T> + Clone + Send + Sync + 'static
where
    F: Fn() -> S + Clone + Send + Sync + 'static,
    S: futures::Stream<Item = Result<T, QueryError>> + Send + Sync + 'static,
    T: CombineChunks,
{
    move || {
        let stream_fn = stream_fn.clone();
        let mode = config.refetch_mode;
        let reducer = config.reducer.clone();
        let initial_value = config.initial_value.clone();
        Box::pin(async move {
            let stream = stream_fn();
            collect_stream(stream, mode, initial_value, reducer).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::execute_query;
    use crate::query::Query;
    use crate::{QueryClient, QueryOptions};
    use futures::stream;
    use tokio_util::sync::CancellationToken;

    fn stream_of(
        chunks: Vec<Result<Vec<i32>, QueryError>>,
    ) -> impl futures::Stream<Item = Result<Vec<i32>, QueryError>> + Send + Sync {
        stream::iter(chunks)
    }

    #[test]
    fn test_refetch_mode_default_is_reset() {
        assert_eq!(RefetchMode::default(), RefetchMode::Reset);
    }

    #[test]
    fn test_combine_chunks_vec_extends() {
        let acc = vec![1, 2];
        let combined = acc.combine(vec![3, 4]);
        assert_eq!(combined, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_combine_chunks_string_concatenates() {
        let acc = "hello ".to_string();
        let combined = acc.combine("world".to_string());
        assert_eq!(combined, "hello world");
    }

    #[tokio::test]
    async fn test_collect_stream_first_fetch_accumulates() {
        let result = collect_stream(
            stream_of(vec![Ok(vec![1]), Ok(vec![2]), Ok(vec![3])]),
            RefetchMode::Replace,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_collect_stream_empty_stream_errors_undefined_data() {
        let err = collect_stream(stream_of(Vec::new()), RefetchMode::Replace, None, None)
            .await
            .unwrap_err();
        assert_eq!(err, QueryError::UndefinedData);
    }

    #[tokio::test]
    async fn test_collect_stream_empty_append_keeps_previous() {
        let result = collect_stream(
            stream_of(Vec::new()),
            RefetchMode::Append,
            Some(vec![7, 8]),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result, vec![7, 8]);
    }

    #[tokio::test]
    async fn test_collect_stream_replace_ignores_previous() {
        let result = collect_stream(
            stream_of(vec![Ok(vec![3])]),
            RefetchMode::Replace,
            Some(vec![1, 2]),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result, vec![3]);
    }

    #[tokio::test]
    async fn test_collect_stream_reset_ignores_previous() {
        let result = collect_stream(
            stream_of(vec![Ok(vec![9])]),
            RefetchMode::Reset,
            Some(vec![1, 2]),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result, vec![9]);
    }

    #[tokio::test]
    async fn test_collect_stream_append_keeps_previous() {
        let result = collect_stream(
            stream_of(vec![Ok(vec![3]), Ok(vec![4])]),
            RefetchMode::Append,
            Some(vec![1, 2]),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result, vec![1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn test_collect_stream_reducer_applied_on_append() {
        // A reducer that ten-folds every incoming chunk proves it overrides
        // the Vec-style default combine.
        let result = collect_stream(
            stream_of(vec![Ok(vec![3]), Ok(vec![4])]),
            RefetchMode::Append,
            Some(vec![1, 2]),
            Some(Arc::new(|acc: Vec<i32>, chunk: Vec<i32>| {
                acc.into_iter()
                    .chain(chunk.into_iter().map(|x| x * 10))
                    .collect::<Vec<i32>>()
            })),
        )
        .await
        .unwrap();
        assert_eq!(result, vec![1, 2, 30, 40]);
    }

    #[tokio::test]
    async fn test_collect_stream_reducer_applies_between_chunks() {
        let result = collect_stream(
            stream_of(vec![Ok(vec![1]), Ok(vec![2]), Ok(vec![3])]),
            RefetchMode::Replace,
            None,
            Some(Arc::new(|acc: Vec<i32>, chunk: Vec<i32>| {
                acc.into_iter()
                    .chain(chunk.into_iter().map(|x| x * 10))
                    .collect::<Vec<i32>>()
            })),
        )
        .await
        .unwrap();
        assert_eq!(result, vec![1, 20, 30]);
    }

    #[tokio::test]
    async fn test_collect_stream_error_chunk_propagates_and_stops() {
        let err = collect_stream(
            stream_of(vec![
                Ok(vec![1]),
                Err(QueryError::network("stream broke")),
                Ok(vec![3]),
            ]),
            RefetchMode::Append,
            Some(vec![0]),
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(err, QueryError::network("stream broke"));
    }

    #[tokio::test]
    async fn test_streamed_query_first_fetch_accumulates() {
        let fetch_fn = streamed_query(
            || stream_of(vec![Ok(vec![1, 2]), Ok(vec![3])]),
            StreamedQueryConfig::new(),
        );
        let result = fetch_fn().await.unwrap();
        assert_eq!(result, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_streamed_query_empty_stream_errors() {
        let fetch_fn = streamed_query(
            || stream_of(Vec::new()),
            StreamedQueryConfig::<Vec<i32>>::new(),
        );
        let err = fetch_fn().await.unwrap_err();
        assert_eq!(err, QueryError::UndefinedData);
    }

    #[tokio::test]
    async fn test_streamed_query_append_uses_initial_value_as_base() {
        let config = StreamedQueryConfig::<Vec<i32>>::new()
            .refetch_mode(RefetchMode::Append)
            .initial_value(vec![0]);
        let fetch_fn = streamed_query(|| stream_of(vec![Ok(vec![3])]), config);
        // Every invocation appends onto the configured initial value.
        assert_eq!(fetch_fn().await.unwrap(), vec![0, 3]);
        assert_eq!(fetch_fn().await.unwrap(), vec![0, 3]);
    }

    #[tokio::test]
    async fn test_streamed_query_plugs_into_executor() {
        let client = QueryClient::new();
        let key = crate::QueryKey::new("streamed");
        let query: Query<Vec<i32>> = Query::new(
            key.clone(),
            streamed_query(
                || stream_of(vec![Ok(vec![1]), Ok(vec![2])]),
                StreamedQueryConfig::new(),
            ),
        );

        let state = execute_query(&client, &query, CancellationToken::new()).await;
        assert!(state.is_success());
        assert_eq!(state.data(), Some(&vec![1, 2]));
        // Result is cached.
        assert_eq!(client.get_query_data::<Vec<i32>>(&key), Some(vec![1, 2]));
    }

    #[tokio::test]
    async fn test_streamed_query_refetch_replace_and_append_through_executor() {
        let client = QueryClient::new();
        let key = crate::QueryKey::new("streamed-refetch");

        // Replace: the second fetch replaces the cached value entirely.
        let replace_query: Query<Vec<i32>> = Query::new(
            key.clone(),
            streamed_query(
                || stream_of(vec![Ok(vec![100])]),
                StreamedQueryConfig::<Vec<i32>>::new().refetch_mode(RefetchMode::Replace),
            ),
        )
        .options(QueryOptions::default());
        let _ = execute_query(&client, &replace_query, CancellationToken::new()).await;
        let _ = execute_query(&client, &replace_query, CancellationToken::new()).await;
        assert_eq!(client.get_query_data::<Vec<i32>>(&key), Some(vec![100]));

        // Append: the next fetch appends onto the configured initial base.
        client.clear();
        let append_query: Query<Vec<i32>> = Query::new(
            key.clone(),
            streamed_query(
                || stream_of(vec![Ok(vec![5])]),
                StreamedQueryConfig::<Vec<i32>>::new()
                    .refetch_mode(RefetchMode::Append)
                    .initial_value(vec![1]),
            ),
        );
        let _ = execute_query(&client, &append_query, CancellationToken::new()).await;
        let _ = execute_query(&client, &append_query, CancellationToken::new()).await;
        assert_eq!(client.get_query_data::<Vec<i32>>(&key), Some(vec![1, 5]));
    }

    #[tokio::test]
    async fn test_streamed_query_undefined_data_errors_through_executor() {
        let client = QueryClient::new();
        let key = crate::QueryKey::new("streamed-empty");
        let query: Query<Vec<i32>> = Query::new(
            key,
            streamed_query(|| stream_of(Vec::new()), StreamedQueryConfig::new()),
        );

        let state = execute_query(&client, &query, CancellationToken::new()).await;
        assert!(state.is_error());
        assert_eq!(state.error(), Some(&QueryError::UndefinedData));
    }

    #[test]
    fn test_config_debug_renders_fn_placeholder() {
        let config = StreamedQueryConfig::<Vec<i32>>::new()
            .refetch_mode(RefetchMode::Append)
            .reducer(|acc: Vec<i32>, chunk: Vec<i32>| acc.combine(chunk))
            .initial_value(vec![0]);
        let debug = format!("{config:?}");
        assert!(debug.contains("refetch_mode"));
        assert!(debug.contains("Append"));
        assert!(debug.contains("<fn>"));
    }

    #[test]
    fn test_config_clone_and_builders() {
        let config = StreamedQueryConfig::<String>::new()
            .refetch_mode(RefetchMode::Append)
            .initial_value("seed".to_string());
        let cloned = config.clone();
        assert_eq!(cloned.refetch_mode, RefetchMode::Append);
        assert_eq!(cloned.initial_value, Some("seed".to_string()));
        assert!(cloned.reducer.is_none());
    }
}
