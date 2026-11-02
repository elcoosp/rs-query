// src/runtime.rs
//! Runtime seam: the spawn + timer capability rs-query needs from the host.
//!
//! rs-query's core is deliberately **non-generic**: [`QueryClient`] and the
//! executors keep their concrete shapes so the crate stays ergonomic. Every
//! place that previously did `tokio::spawn` or `tokio::time::sleep` now goes
//! through a [`QueryRuntime`] handle owned by the client:
//!
//! * [`PoolRuntime`] — the default. Spawns on quoin's shared worker pool and
//!   sleeps with quoin's portable timer. Needs **no** UI framework and **no**
//!   ambient async runtime.
//! * [`ExecutorRuntime`] — wraps any [`quoin_core::Executor`] (in practice
//!   `C::Executor` for some `C: quoin_core::ReactiveContext`), so fetches and
//!   timers run on the hosting framework's executor (gpui executor, tokio for
//!   xilem, wasm main thread on the web, …).
//!
//! Build a client for a quoin app with
//! [`QueryClient::with_context`](crate::QueryClient::with_context).

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A boxed, `Send`, owned future — the currency of [`QueryRuntime`].
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

struct SpawnHandleInner {
    abort_fn: Box<dyn Fn() + Send + Sync>,
    rx: Mutex<Option<futures::channel::oneshot::Receiver<()>>>,
    detached: AtomicBool,
    /// Sticky completion flag: `try_recv` consumes the value, so once it has
    /// been observed, `is_finished` stays `true` without polling again.
    finished: AtomicBool,
}

/// Handle to a task spawned through [`QueryRuntime`].
///
/// * [`SpawnHandle::abort`] cancels the task at its next suspension point.
/// * [`SpawnHandle::detach`] lets it run to completion without observing it.
///
/// The handle is cheaply clonable (all clones abort the same task); the
/// abort-on-drop rule is enforced by the owners of the handle (e.g.
/// [`SpawnedTask`]), not by the handle itself.
#[derive(Clone)]
pub struct SpawnHandle {
    inner: Arc<SpawnHandleInner>,
}

impl SpawnHandle {
    pub(crate) fn new(
        abort_fn: Box<dyn Fn() + Send + Sync>,
        rx: futures::channel::oneshot::Receiver<()>,
    ) -> Self {
        Self {
            inner: Arc::new(SpawnHandleInner {
                abort_fn,
                rx: Mutex::new(Some(rx)),
                detached: AtomicBool::new(false),
                finished: AtomicBool::new(false),
            }),
        }
    }

    /// Cancels the task at its next suspension point (idempotent).
    pub fn abort(&self) {
        (self.inner.abort_fn)();
    }

    /// Lets the task run to completion without observing it (idempotent).
    pub fn detach(&self) {
        self.inner.detached.store(true, Ordering::SeqCst);
    }

    /// Whether the handle was detached.
    #[must_use]
    pub fn is_detached(&self) -> bool {
        self.inner.detached.load(Ordering::SeqCst)
    }

    /// Whether the task ran to completion (as opposed to pending/aborted).
    #[must_use]
    pub fn is_finished(&self) -> bool {
        if self.inner.finished.load(Ordering::SeqCst) {
            return true;
        }
        let mut guard = self
            .inner
            .rx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(rx) = &mut *guard
            && matches!(rx.try_recv(), Ok(Some(())))
        {
            self.inner.finished.store(true, Ordering::SeqCst);
            return true;
        }
        false
    }
}

impl Future for SpawnHandle {
    type Output = Result<(), futures::channel::oneshot::Canceled>;

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        use futures::future::FutureExt;
        let mut guard = self
            .inner
            .rx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &mut *guard {
            Some(rx) => rx.poll_unpin(cx),
            None => std::task::Poll::Ready(Err(futures::channel::oneshot::Canceled)),
        }
    }
}

impl std::fmt::Debug for SpawnHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnHandle")
            .field("detached", &self.is_detached())
            .finish()
    }
}

/// The spawn + timer capability rs-query needs from the host environment.
///
/// Object-safe on purpose: [`QueryClient`](crate::QueryClient) stores
/// `Arc<dyn QueryRuntime>` so the entire core stays non-generic. Library
/// users rarely touch this trait — use [`PoolRuntime`] or
/// [`ExecutorRuntime`], or implement it to plug a custom scheduler in.
pub trait QueryRuntime: Send + Sync + 'static {
    /// Spawns a fire-and-forget task; abort it via the returned handle.
    fn spawn(&self, future: BoxFuture<()>) -> SpawnHandle;

    /// Waits for `duration` of wall-clock time (retry backoff, persist
    /// save-loop intervals, throttles).
    fn sleep(&self, duration: Duration) -> BoxFuture<()>;
}

/// The default runtime: quoin's shared worker pool + quoin's portable timer.
///
/// Works on every target with no framework and no ambient async runtime;
/// on `wasm32` tasks run on the browser main thread via quoin's pool glue.
#[derive(Clone, Copy, Debug, Default)]
pub struct PoolRuntime;

impl QueryRuntime for PoolRuntime {
    fn spawn(&self, future: BoxFuture<()>) -> SpawnHandle {
        let (tx, rx) = futures::channel::oneshot::channel();
        // The abort capability is owned by an `AbortHandle` (drop-neutral) so
        // dropping the `SpawnHandle` never aborts through a framework
        // handle's own abort-on-drop `Drop`. Detaching the pooled handle
        // releases its structured-concurrency guard: abort ownership moves
        // to the `SpawnHandle`.
        let (abort_handle, abort_registration) = futures::future::AbortHandle::new_pair();
        quoin_core::pool::spawn_pooled(futures::future::Abortable::new(
            async move {
                future.await;
                let _ = tx.send(());
            },
            abort_registration,
        ));
        SpawnHandle::new(Box::new(move || abort_handle.abort()), rx)
    }

    fn sleep(&self, duration: Duration) -> BoxFuture<()> {
        quoin_core::sleep(duration)
    }
}

/// Wraps any quoin [`quoin_core::Executor`] into a [`QueryRuntime`].
///
/// This is how a quoin application hosts rs-query: the executor comes from
/// the framework's [`quoin_core::ReactiveContext`], so fetches spawn on the
/// framework's own scheduler and sleeps use the framework's timer.
#[derive(Clone, Debug)]
pub struct ExecutorRuntime<E> {
    executor: E,
}

impl<E> ExecutorRuntime<E> {
    /// Wraps `executor`.
    pub fn new(executor: E) -> Self {
        Self { executor }
    }

    /// The wrapped executor.
    #[must_use]
    pub fn executor(&self) -> &E {
        &self.executor
    }
}

impl<E> QueryRuntime for ExecutorRuntime<E>
where
    E: quoin_core::Executor,
{
    fn spawn(&self, future: BoxFuture<()>) -> SpawnHandle {
        let (tx, rx) = futures::channel::oneshot::channel();
        // See `PoolRuntime::spawn`: the abort capability is an `AbortHandle`,
        // and the framework handle is detached immediately so its own
        // abort-on-drop rule cannot fire when the `SpawnHandle` is dropped.
        let (abort_handle, abort_registration) = futures::future::AbortHandle::new_pair();
        let join = self.executor.spawn(futures::future::Abortable::new(
            async move {
                future.await;
                let _ = tx.send(());
            },
            abort_registration,
        ));
        quoin_core::executor::JoinHandle::detach(join);
        SpawnHandle::new(Box::new(move || abort_handle.abort()), rx)
    }

    fn sleep(&self, duration: Duration) -> BoxFuture<()> {
        self.executor.sleep(duration)
    }
}

/// Generic awaitable/abortable handle to a task that produces a value.
///
/// Await it to observe the result, [`SpawnedTask::abort`] to cancel, or
/// [`SpawnedTask::detach`] for fire-and-forget. **Dropping without awaiting
/// or detaching aborts the task** (structured concurrency); awaiting once
/// marks the task detached.
///
/// If the task is aborted before producing a value, awaiting resolves with
/// the `fallback` the task was built with (e.g. a cancelled [`QueryState`]).
pub struct SpawnedTask<O> {
    handle: SpawnHandle,
    rx: Option<futures::channel::oneshot::Receiver<O>>,
    fallback: Arc<dyn Fn() -> O + Send + Sync>,
}

impl<O> SpawnedTask<O> {
    pub(crate) fn new(
        handle: SpawnHandle,
        rx: futures::channel::oneshot::Receiver<O>,
        fallback: Arc<dyn Fn() -> O + Send + Sync>,
    ) -> Self {
        Self {
            handle,
            rx: Some(rx),
            fallback,
        }
    }

    /// Aborts the task at its next suspension point; a later await resolves
    /// with the task's fallback value.
    pub fn abort(&self) {
        self.handle.abort();
    }

    /// Lets the task run to completion without observing its result. Any
    /// later poll of the future resolves with the fallback value.
    pub fn detach(&self) {
        self.handle.detach();
    }
}

impl<O> Future for SpawnedTask<O> {
    type Output = O;

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<O> {
        use futures::future::FutureExt;
        // Awaiting resolves the handle: the task must not be cancelled by
        // the drop that follows.
        self.handle.detach();
        match self.rx.as_mut() {
            Some(rx) => match rx.poll_unpin(cx) {
                std::task::Poll::Ready(Ok(value)) => std::task::Poll::Ready(value),
                // The task was aborted before producing a value.
                std::task::Poll::Ready(Err(_)) => {
                    self.rx.take();
                    std::task::Poll::Ready((self.fallback)())
                }
                std::task::Poll::Pending => std::task::Poll::Pending,
            },
            // Detached: the result is no longer observable.
            None => std::task::Poll::Ready((self.fallback)()),
        }
    }
}

impl<O> Drop for SpawnedTask<O> {
    fn drop(&mut self) {
        // Structured concurrency: dropping without awaiting/detaching
        // cancels the underlying task.
        if !self.handle.is_detached() {
            self.handle.abort();
        }
    }
}

impl<O> std::fmt::Debug for SpawnedTask<O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnedTask").finish_non_exhaustive()
    }
}

/// Builds a [`SpawnHandle`] from an abort closure and a completion receiver.
pub(crate) fn spawn_handle(
    abort_fn: Box<dyn Fn() + Send + Sync>,
    rx: futures::channel::oneshot::Receiver<()>,
) -> SpawnHandle {
    SpawnHandle::new(abort_fn, rx)
}
