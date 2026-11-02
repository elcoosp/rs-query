// src/bus.rs
//! Bounded async broadcast channel (the `tokio::sync::broadcast` stand-in).
//!
//! Semantics mirror the tokio channel rs-query previously relied on, without
//! pulling in a runtime:
//!
//! * [`Sender::send`] is synchronous: the value is cloned and queued for
//!   every live receiver. Senders are `Clone` (clones share the stream).
//! * [`Receiver::recv`] awaits the next value; [`Receiver::try_recv`] never
//!   awaits; `Receiver` is also a [`Stream`].
//! * Each receiver has a bounded queue (`capacity`). When a receiver falls
//!   behind and the queue overflows, the **oldest** values are dropped and
//!   the next receive resolves with [`RecvError::Lagged(n)`] telling how
//!   many were skipped — observers use this to re-read authoritative state
//!   from the cache.
//! * When every sender is gone, receives resolve with [`RecvError::Closed`].
//!
//! Locks are never held across `.await`: `send`/`try_recv` take the shared
//! mutex, queue values, collect wakers, release, then wake.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use futures::FutureExt;
use futures::stream::Stream;

/// Errors produced by [`Receiver::recv`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecvError {
    /// All senders (and the channel) are dropped.
    Closed,
    /// The receiver lagged: `n` values were dropped from its queue since the
    /// last successful receive. The receiver is still usable.
    Lagged(u64),
}

/// Errors produced by [`Receiver::try_recv`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TryRecvError {
    /// No value is queued right now.
    Empty,
    /// All senders (and the channel) are dropped.
    Closed,
    /// The receiver lagged: `n` values were dropped from its queue.
    Lagged(u64),
}

struct RecvState<T> {
    queue: VecDeque<T>,
    waker: Option<Waker>,
    missed: u64,
    alive: bool,
}

struct Shared<T> {
    capacity: usize,
    next_id: u64,
    senders_alive: bool,
    sender_count: usize,
    receivers: HashMap<u64, RecvState<T>>,
}

/// Sending half of a bounded broadcast channel. Clone to add another sender.
pub struct Sender<T> {
    shared: Arc<Mutex<Shared<T>>>,
}

/// Receiving half of a bounded broadcast channel. Each
/// [`Sender::subscribe`] yields an independent stream starting "now" (no
/// backlog).
pub struct Receiver<T> {
    id: u64,
    shared: Arc<Mutex<Shared<T>>>,
}

impl<T: Clone> Sender<T> {
    pub(crate) fn channel(capacity: usize) -> (Sender<T>, Receiver<T>) {
        let shared = Arc::new(Mutex::new(Shared {
            capacity: capacity.max(1),
            next_id: 0,
            senders_alive: true,
            sender_count: 1,
            receivers: HashMap::new(),
        }));
        let rx = Receiver {
            id: 0,
            shared: Arc::clone(&shared),
        };
        // The initial receiver shares the same machinery: reserve id 0 for it.
        {
            let mut guard = shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.next_id = 1;
            guard.receivers.insert(
                0,
                RecvState {
                    queue: VecDeque::new(),
                    waker: None,
                    missed: 0,
                    alive: true,
                },
            );
        }
        (
            Sender {
                shared: Arc::clone(&shared),
            },
            rx,
        )
    }

    /// Creates an independent receiver starting "now" (no backlog).
    #[must_use]
    pub fn subscribe(&self) -> Receiver<T> {
        let mut guard = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = guard.next_id;
        guard.next_id += 1;
        guard.receivers.insert(
            id,
            RecvState {
                queue: VecDeque::new(),
                waker: None,
                missed: 0,
                alive: true,
            },
        );
        Receiver {
            id,
            shared: Arc::clone(&self.shared),
        }
    }

    /// Queues `value` (cloned per receiver) for every live receiver. The
    /// oldest queued values are dropped for receivers whose queue is full
    /// (they will observe `Lagged` on their next receive). Calling `send`
    /// with no live receivers simply drops the value.
    pub fn send(&self, value: &T) {
        let mut wakers: Vec<Waker> = Vec::new();
        {
            let mut guard = self
                .shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let capacity = guard.capacity;
            for state in guard.receivers.values_mut() {
                if !state.alive {
                    continue;
                }
                if state.queue.len() >= capacity {
                    state.queue.pop_front();
                    state.missed += 1;
                }
                state.queue.push_back(value.clone());
                if let Some(waker) = state.waker.take() {
                    wakers.push(waker);
                }
            }
        }
        for waker in wakers {
            waker.wake();
        }
    }

    /// Number of live receivers.
    #[must_use]
    pub fn receiver_count(&self) -> usize {
        self.shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .receivers
            .values()
            .filter(|state| state.alive)
            .count()
    }
}

impl<T: Clone> Clone for Sender<T> {
    fn clone(&self) -> Self {
        let mut guard = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.sender_count += 1;
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let wakers: Vec<Waker> = {
            let mut guard = self
                .shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.sender_count = guard.sender_count.saturating_sub(1);
            if guard.sender_count == 0 {
                guard.senders_alive = false;
                // Wake every parked receiver so they observe `Closed`.
                guard
                    .receivers
                    .values_mut()
                    .filter_map(|state| state.waker.take())
                    .collect()
            } else {
                Vec::new()
            }
        };
        for waker in wakers {
            waker.wake();
        }
    }
}

impl<T> Receiver<T> {
    /// Awaits the next value; see [`RecvError`] for lag/close semantics.
    pub fn recv(&mut self) -> Recv<'_, T> {
        Recv { rx: self }
    }

    /// Non-blocking receive.
    pub fn try_recv(&mut self) -> Result<T, TryRecvError> {
        let mut guard = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::poll_state(&mut guard, self.id, None)
    }

    fn poll_state(
        guard: &mut Shared<T>,
        id: u64,
        waker: Option<&Waker>,
    ) -> Result<T, TryRecvError> {
        let Some(state) = guard.receivers.get_mut(&id) else {
            return Err(TryRecvError::Closed);
        };
        if state.missed > 0 {
            let missed = state.missed;
            state.missed = 0;
            // Tokio-compatible: the lag notice is delivered before any
            // queued value so observers can re-read authoritative state.
            return Err(TryRecvError::Lagged(missed));
        }
        if let Some(value) = state.queue.pop_front() {
            return Ok(value);
        }
        if !guard.senders_alive {
            state.alive = false;
            return Err(TryRecvError::Closed);
        }
        if let Some(waker) = waker {
            state.waker = Some(waker.clone());
        }
        Err(TryRecvError::Empty)
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.shared.lock() {
            guard.receivers.remove(&self.id);
        }
    }
}

/// Future returned by [`Receiver::recv`].
pub struct Recv<'a, T> {
    rx: &'a mut Receiver<T>,
}

impl<T> Future for Recv<'_, T> {
    type Output = Result<T, RecvError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut guard = this
            .rx
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match Receiver::poll_state(&mut guard, this.rx.id, Some(cx.waker())) {
            Ok(value) => Poll::Ready(Ok(value)),
            Err(TryRecvError::Empty) => Poll::Pending,
            Err(TryRecvError::Closed) => Poll::Ready(Err(RecvError::Closed)),
            Err(TryRecvError::Lagged(n)) => Poll::Ready(Err(RecvError::Lagged(n))),
        }
    }
}

impl<T> Stream for Receiver<T> {
    type Item = Result<T, RecvError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.recv().poll_unpin(cx) {
            Poll::Ready(Ok(value)) => Poll::Ready(Some(Ok(value))),
            Poll::Ready(Err(RecvError::Lagged(n))) => Poll::Ready(Some(Err(RecvError::Lagged(n)))),
            Poll::Ready(Err(RecvError::Closed)) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T: Clone> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        // A clone is a fresh subscription: it starts at "now" like any other
        // receiver instead of sharing queue position (documented deviation
        // from tokio, whose clones share the stream).
        Sender {
            shared: Arc::clone(&self.shared),
        }
        .subscribe()
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sender").finish_non_exhaustive()
    }
}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Receiver").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn send_recv_round_trip() {
        let (tx, mut rx) = Sender::channel(4);
        tx.send(&1);
        tx.send(&2);
        assert_eq!(rx.try_recv(), Ok(1));
        assert_eq!(rx.try_recv(), Ok(2));
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn fan_out_is_independent() {
        let (tx, mut a) = Sender::channel(4);
        let mut b = tx.subscribe();
        tx.send(&"x");
        assert_eq!(a.try_recv(), Ok("x"));
        assert_eq!(b.try_recv(), Ok("x"));
        // Subscriber starting later sees nothing of the past.
        let mut c = tx.subscribe();
        assert_eq!(c.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn lag_drops_oldest_and_reports() {
        let (tx, mut rx) = Sender::channel(2);
        tx.send(&1);
        tx.send(&2);
        tx.send(&3);
        assert_eq!(rx.try_recv(), Err(TryRecvError::Lagged(1)));
        assert_eq!(rx.try_recv(), Ok(2));
        assert_eq!(rx.try_recv(), Ok(3));
    }

    #[test]
    fn close_when_senders_drop() {
        let (tx, mut rx) = Sender::channel(2);
        let tx2 = tx.clone();
        drop(tx);
        tx2.send(&7);
        drop(tx2);
        assert_eq!(futures::executor::block_on(rx.recv()), Ok(7));
        assert_eq!(
            futures::executor::block_on(rx.recv()),
            Err(RecvError::Closed)
        );
    }

    #[test]
    fn recv_wakes_on_send() {
        let (tx, mut rx) = Sender::channel(2);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            tx.send(&42);
        });
        // recv must park until the send arrives (waker exercised).
        let value = futures::executor::block_on(rx.recv());
        assert_eq!(value, Ok(42));
        handle.join().expect("sender thread must not panic");
    }

    #[test]
    fn stream_trait_works() {
        let (tx, rx) = Sender::<u32>::channel(2);
        futures::executor::block_on(async move {
            let mut rx = rx;
            let send_task = {
                let tx = tx.clone();
                async move {
                    tx.send(&1);
                    tx.send(&2);
                }
            };
            send_task.await;
            let first = futures::StreamExt::next(&mut rx).await;
            assert_eq!(first, Some(Ok(1)));
            let second = futures::StreamExt::next(&mut rx).await;
            assert_eq!(second, Some(Ok(2)));
        });
    }
}
