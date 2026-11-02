// src/broadcast.rs
//! In-process cache broadcast, porting `TanStack` Query's
//! `broadcastQueryClient`.
//!
//! `TanStack` uses the `broadcast-channel` npm package for cross-tab transport.
//! quoin apps sync windows/instances inside one process, so this module ships
//! an **in-process hub**: a global registry of bounded broadcast buses
//! ([`crate::bus`]) keyed by channel name. Registered clients apply remote
//! snapshots with [`QueryClient::hydrate`]. The loop runs on the client's
//! [`QueryRuntime`](crate::QueryRuntime) (framework executor or the shared
//! pool).
//!
//! Re-entrancy guard: while a remote snapshot is being applied, the receiving
//! task raises a local flag and never publishes of its own — remote updates
//! are never echoed back (`TanStack`'s `transaction` guard).

use crate::QueryClient;
use crate::hydration::HydrateOptions;
use crate::persist_client::PersistedClient;
use crate::persister::now_ms;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use crate::bus;
use crate::runtime::{SpawnHandle, spawn_handle};
use crate::watch::Watch;

/// Default channel name (`TanStack`: `tanstack-query`).
pub const DEFAULT_CHANNEL: &str = "rs-query";

/// Maximum number of queued snapshots per channel before a slow receiver lags.
const CHANNEL_CAPACITY: usize = 64;

/// Global registry of in-process broadcast channels.
static CHANNEL_REGISTRY: LazyLock<Mutex<HashMap<String, bus::Sender<PersistedClient>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Look up (or create) the channel and return the sender plus a fresh
/// receiver for a new subscriber.
fn channel_entry(channel: &str) -> (bus::Sender<PersistedClient>, bus::Receiver<PersistedClient>) {
    let mut registry = CHANNEL_REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let sender = registry
        .entry(channel.to_string())
        .or_insert_with(|| bus::Sender::channel(CHANNEL_CAPACITY).0);
    (sender.clone(), sender.subscribe())
}

/// Drop the channel from the registry once its last subscriber is gone, so a
/// later re-subscribe starts from a clean queue.
fn maybe_remove_channel(channel: &str) {
    let mut registry = CHANNEL_REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let empty = registry
        .get(channel)
        .is_some_and(|sender| sender.receiver_count() == 0);
    if empty {
        registry.remove(channel);
    }
}

/// Options for [`broadcast_query_client_with_options`].
#[derive(Clone, Debug)]
pub struct BroadcastQueryClientOptions {
    channel: String,
}

impl Default for BroadcastQueryClientOptions {
    fn default() -> Self {
        Self {
            channel: DEFAULT_CHANNEL.to_string(),
        }
    }
}

impl BroadcastQueryClientOptions {
    /// Options for the default channel.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The channel name.
    #[must_use]
    pub fn channel(&self) -> &str {
        &self.channel
    }

    /// Set the channel name.
    #[must_use]
    pub fn with_channel(mut self, channel: impl Into<String>) -> Self {
        self.channel = channel.into();
        self
    }
}

/// Handle for a [`broadcast_query_client`] subscription.
///
/// Dropping the handle stops the subscription (the `Drop` impl unsubscribes);
/// use [`BroadcastHandle::into_task`] when you need to await the loop.
pub struct BroadcastHandle {
    /// Cooperative stop flag; also set by [`BroadcastHandle::unsubscribe`].
    pub unsubscribe: Arc<AtomicBool>,
    /// The receiving task; aborting it also stops the subscription. Dropping
    /// the handle stops the loop as well (the `Drop` impl unsubscribes).
    pub task: Option<SpawnHandle>,
    stop_signal: Watch<bool>,
    /// Re-entrancy guard, raised while a remote snapshot is being applied.
    applying_remote: Arc<AtomicBool>,
}

impl BroadcastHandle {
    /// Stop the subscription and release the channel (idempotent).
    pub fn unsubscribe(&self) {
        self.unsubscribe.store(true, Ordering::SeqCst);
        self.stop_signal.send(true);
    }

    /// Stop the subscription and return the completion task.
    ///
    /// Consumes the handle: this is the reliable way to both stop the
    /// subscription and await the loop, because unmatched fields of a
    /// destructured struct are only dropped at the end of the enclosing
    /// block (a partially moved `Drop` type cannot stop itself).
    ///
    /// # Panics
    ///
    /// Panics when the task was already taken via a previous `into_task`
    /// call; a handle yields its task exactly once.
    #[must_use]
    pub fn into_task(mut self) -> SpawnHandle {
        let task = self
            .task
            .take()
            .expect("into_task consumes a handle whose task was never taken");
        self.unsubscribe();
        task
    }

    /// Whether a remote snapshot is currently being applied (the re-entrancy
    /// guard; useful for tests and diagnostics).
    #[must_use]
    pub fn is_applying_remote(&self) -> bool {
        self.applying_remote.load(Ordering::SeqCst)
    }
}

impl Drop for BroadcastHandle {
    fn drop(&mut self) {
        self.unsubscribe();
    }
}

/// Subscribe `client` to a broadcast channel (`TanStack` `broadcastQueryClient`).
///
/// Every remote snapshot published on `channel` is applied to the client with
/// [`QueryClient::hydrate`]; the re-entrancy guard guarantees applying never
/// re-publishes. The client's own publishes are also delivered back to it
/// (there is no sender identity on a `tokio::sync::broadcast` channel);
/// re-applying its own state is a no-op in practice.
///
/// The returned handle unsubscribes and releases the channel; dropping it
/// stops the subscription too.
///
/// # Errors
///
/// Currently no code path produces `Err`: the `Result` is reserved so the
/// subscription can become fallible (e.g. transport setup) without breaking
/// callers.
#[must_use]
pub fn broadcast_query_client(client: &QueryClient, channel: &str) -> BroadcastHandle {
    broadcast_query_client_with_options(
        client,
        &BroadcastQueryClientOptions::new().with_channel(channel),
    )
}

/// Like [`broadcast_query_client`] but taking full options.
pub fn broadcast_query_client_with_options(
    client: &QueryClient,
    options: &BroadcastQueryClientOptions,
) -> BroadcastHandle {
    let (_sender, mut receiver) = channel_entry(&options.channel);
    let unsubscribe = Arc::new(AtomicBool::new(false));
    let (stop_tx, mut stop_rx) = Watch::new(false);
    let applying_remote = Arc::new(AtomicBool::new(false));

    let task_client = client.clone();
    let task_flag = Arc::clone(&unsubscribe);
    let task_guard = Arc::clone(&applying_remote);
    let task_channel = options.channel.clone();
    let (done_tx, done_rx) = futures::channel::oneshot::channel::<()>();
    let task = async move {
        loop {
            // Race the stop signal against the next incoming snapshot.
            let stop_fut = stop_rx.changed();
            let msg_fut = receiver.recv();
            futures::pin_mut!(stop_fut, msg_fut);
            let message = match futures::future::select(stop_fut, msg_fut).await {
                futures::future::Either::Left((_, _)) => {
                    break;
                }
                futures::future::Either::Right((incoming, _)) => match incoming {
                    Ok(message) => message,
                    Err(bus::RecvError::Lagged(skipped)) => {
                        tracing::debug!(
                            skipped = skipped,
                            channel = %task_channel,
                            "rs-query broadcast: receiver lagged"
                        );
                        continue;
                    }
                    Err(bus::RecvError::Closed) => {
                        break;
                    }
                },
            };
            if task_flag.load(Ordering::SeqCst) {
                break;
            }
            // Apply with the re-entrancy guard raised: applying never
            // re-broadcasts (TanStack's `transaction` guard).
            task_guard.store(true, Ordering::SeqCst);
            task_client.hydrate(message.client_state, HydrateOptions::default());
            task_guard.store(false, Ordering::SeqCst);
        }
        drop(receiver);
        maybe_remove_channel(&task_channel);
        let _ = done_tx.send(());
    };
    let handle = client.runtime().spawn(Box::pin(task));
    let task = spawn_handle(Box::new(move || handle.abort()), done_rx);

    BroadcastHandle {
        unsubscribe,
        task: Some(task),
        stop_signal: stop_tx,
        applying_remote,
    }
}

/// Publish the client's current dehydrated state to a channel.
///
/// Broadcasting is best-effort (`TanStack`'s `safePost`): with no subscribers
/// the call is a no-op, and delivery failures are only logged.
pub fn publish_state(client: &QueryClient, channel: &str) {
    let snapshot = PersistedClient {
        timestamp_ms: now_ms(),
        buster: String::new(),
        client_state: client.dehydrate(),
    };
    let sender = {
        let mut registry = CHANNEL_REGISTRY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        registry
            .entry(channel.to_string())
            .or_insert_with(|| bus::Sender::channel(CHANNEL_CAPACITY).0)
            .clone()
    };
    if sender.receiver_count() == 0 {
        return;
    }
    sender.send(&snapshot);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::QueryKey;
    use crate::options::QueryOptions;
    use std::time::Duration;

    async fn wait_for(mut condition: impl FnMut() -> bool) -> bool {
        for _ in 0..200 {
            if condition() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        condition()
    }

    fn set_string(client: &QueryClient, name: &str, value: &str) {
        client.set_query_data(
            &QueryKey::new(name),
            value.to_string(),
            QueryOptions::default(),
        );
    }

    fn get_string(client: &QueryClient, name: &str) -> Option<String> {
        client.get_query_data::<String>(&QueryKey::new(name))
    }

    #[test]
    fn test_options_default_channel() {
        let options = BroadcastQueryClientOptions::new();
        assert_eq!(options.channel(), DEFAULT_CHANNEL);
        assert!(format!("{options:?}").contains("BroadcastQueryClientOptions"));
        let custom = options.with_channel("my-channel");
        assert_eq!(custom.channel(), "my-channel");
    }

    #[tokio::test]
    async fn test_closed_channel_ends_the_subscription_task() {
        let client = QueryClient::new();
        let channel = "broadcast-closed-registry-entry";
        let handle = broadcast_query_client(&client, channel);

        // Park the task on `recv()` before closing the channel.
        tokio::task::yield_now().await;

        // Removing the registry entry drops the last sender while the task
        // is still subscribed, so its next `recv()` resolves with `Closed`
        // and the loop ends even though no stop was requested.
        CHANNEL_REGISTRY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(channel);
        for _ in 0..200 {
            if handle.task.as_ref().is_some_and(SpawnHandle::is_finished) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            handle.task.as_ref().is_some_and(SpawnHandle::is_finished),
            "closed channel must end the subscription task"
        );
        handle.unsubscribe();
    }

    #[tokio::test]
    async fn test_publish_updates_subscribed_client() {
        let channel = "broadcast-updates-client";
        let a = QueryClient::new();
        let b = QueryClient::new();
        set_string(&a, "shared", "alpha");
        set_string(&b, "b-only", "beta");

        let handle_a = broadcast_query_client(&a, channel);
        let handle_b = broadcast_query_client(&b, channel);

        publish_state(&a, channel);
        let delivered = wait_for(|| get_string(&b, "shared").as_deref() == Some("alpha")).await;
        assert!(delivered, "B should receive A's snapshot");

        // Second publish with new data also lands.
        set_string(&a, "shared", "alpha-2");
        publish_state(&a, channel);
        let delivered = wait_for(|| get_string(&b, "shared").as_deref() == Some("alpha-2")).await;
        assert!(delivered, "B should receive the second snapshot");

        // B's unrelated local data is untouched by hydration.
        assert_eq!(get_string(&b, "b-only").as_deref(), Some("beta"));

        handle_a.unsubscribe();
        handle_b.unsubscribe();
        handle_a.into_task().await.expect("task A should stop");
        handle_b.into_task().await.expect("task B should stop");
    }

    #[tokio::test]
    async fn test_guard_prevents_echo() {
        let channel = "broadcast-echo-guard";
        let a = QueryClient::new();
        let b = QueryClient::new();
        set_string(&a, "a-key", "first");
        set_string(&b, "b-key", "b-local");

        let handle_a = broadcast_query_client(&a, channel);
        let handle_b = broadcast_query_client(&b, channel);

        publish_state(&a, channel);
        let delivered = wait_for(|| get_string(&b, "a-key").as_deref() == Some("first")).await;
        assert!(delivered);

        // Publish twice: B applies both snapshots but never re-broadcasts
        // them, so A never receives B's state (which carries `b-key`).
        set_string(&a, "a-key", "second");
        publish_state(&a, channel);
        let delivered = wait_for(|| get_string(&b, "a-key").as_deref() == Some("second")).await;
        assert!(delivered);
        let settled = wait_for(|| !handle_b.is_applying_remote()).await;
        assert!(settled, "guard must be reset after applying");
        assert!(
            !a.cache.contains_key("b-key"),
            "a bounce would have delivered B's local-only entry to A"
        );

        handle_a.unsubscribe();
        handle_b.unsubscribe();
        handle_a.into_task().await.expect("task A should stop");
        handle_b.into_task().await.expect("task B should stop");
    }

    #[tokio::test]
    async fn test_unsubscribe_stops_delivery() {
        let channel = "broadcast-unsubscribe";
        let a = QueryClient::new();
        let b = QueryClient::new();
        set_string(&a, "a-key", "first");

        let handle_b = broadcast_query_client(&b, channel);
        publish_state(&a, channel);
        let delivered = wait_for(|| get_string(&b, "a-key").as_deref() == Some("first")).await;
        assert!(delivered);

        handle_b.unsubscribe();
        handle_b.into_task().await.expect("task should stop");

        set_string(&a, "a-key", "second");
        publish_state(&a, channel);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            get_string(&b, "a-key").as_deref(),
            Some("first"),
            "unsubscribed client must not receive further snapshots"
        );
    }

    #[tokio::test]
    async fn test_channels_are_isolated() {
        let channel_one = "broadcast-isolated-1";
        let channel_two = "broadcast-isolated-2";
        let a = QueryClient::new();
        let b = QueryClient::new();
        set_string(&a, "a-key", "from-one");

        let handle_a = broadcast_query_client(&a, channel_one);
        let handle_b = broadcast_query_client(&b, channel_two);

        publish_state(&a, channel_one);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            get_string(&b, "a-key").is_none(),
            "different channels must not cross-deliver"
        );

        handle_a.unsubscribe();
        handle_b.unsubscribe();
        handle_a.into_task().await.expect("task A should stop");
        handle_b.into_task().await.expect("task B should stop");
    }

    #[tokio::test]
    async fn test_publish_without_subscribers_is_ok() {
        let a = QueryClient::new();
        set_string(&a, "a-key", "lonely");
        // No subscriber on this channel: must be a silent no-op, no panic.
        publish_state(&a, "broadcast-no-subscribers");
    }

    #[tokio::test(start_paused = true)]
    async fn test_wait_for_returns_false_when_condition_never_holds() {
        // Paused time auto-advances the sleeps, so the full poll loop runs
        // quickly; the final fallback re-check must report the failure.
        assert!(!wait_for(|| false).await);
    }

    #[tokio::test]
    async fn test_subscriber_survives_lagged_broadcasts() {
        let channel = "broadcast-lagged";
        let a = QueryClient::new();
        let b = QueryClient::new();
        set_string(&a, "a-key", "first");

        let handle_b = broadcast_query_client(&b, channel);

        // Publish far more snapshots than the channel capacity without
        // yielding: the receiving task has not been polled yet, so its first
        // poll lags and skips the overflow before applying the final snapshot.
        for i in 0..(CHANNEL_CAPACITY * 2) {
            set_string(&a, "bulk", &format!("v{i}"));
            publish_state(&a, channel);
        }
        set_string(&a, "final", "last");
        publish_state(&a, channel);

        let delivered = wait_for(|| get_string(&b, "final").as_deref() == Some("last")).await;
        assert!(delivered, "the newest snapshot is applied after lagging");
        assert_eq!(get_string(&b, "bulk").as_deref(), Some("v127"));

        handle_b.unsubscribe();
        handle_b.into_task().await.expect("task should stop");
    }

    #[tokio::test]
    async fn test_unsubscribe_flag_stops_loop_before_applying() {
        let channel = "broadcast-flag-stop";
        let a = QueryClient::new();
        let b = QueryClient::new();
        set_string(&a, "a-key", "value");

        let handle = broadcast_query_client(&b, channel);

        // Raise the cooperative flag before publishing, without firing the
        // watch signal (the handle is forgotten so `stop_rx.changed()` never
        // fires): the loop must exit at the flag check right after receiving
        // the message, before applying it.
        let flag = Arc::clone(&handle.unsubscribe);
        std::mem::forget(handle);
        flag.store(true, Ordering::SeqCst);
        publish_state(&a, channel);

        // Yield so the receiving task consumes the message and hits the flag
        // check; the snapshot must not have been applied.
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert!(
            get_string(&b, "a-key").is_none(),
            "flag check precedes hydration"
        );
    }

    #[tokio::test]
    async fn test_dropping_handle_stops_subscription() {
        let channel = "broadcast-drop-handle";
        let a = QueryClient::new();
        let b = QueryClient::new();
        set_string(&a, "a-key", "value");

        let handle = broadcast_query_client(&b, channel);
        let task = handle.into_task();
        task.await.expect("task should stop after unsubscribe");
    }

    #[tokio::test]
    async fn test_receiver_stops_when_last_sender_dropped() {
        let channel = "broadcast-closed";
        let b = QueryClient::new();

        let handle = broadcast_query_client(&b, channel);

        // The registry holds the channel's only sender. Removing it drops the
        // last sender while the receiving task is still subscribed, so its
        // `recv()` reports `Closed` and the loop exits cleanly.
        let removed = CHANNEL_REGISTRY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(channel);
        assert!(removed.is_some(), "the channel entry must exist");

        handle
            .into_task()
            .await
            .expect("task should stop on closed channel");
    }

    #[tokio::test]
    async fn test_publish_swallows_send_failure_on_closed_receiver() {
        let channel = "broadcast-send-error";
        let a = QueryClient::new();
        set_string(&a, "a-key", "value");

        // `send` fails only when the receiver count drops to zero *between*
        // `publish_state`'s `receiver_count()` check and its `send` — a race
        // with a concurrently dropping receiver. Hammer the receiver count
        // from background threads while publishing in a loop: whenever a
        // drop lands in that window the send fails and the delivery error is
        // swallowed (`TanStack`'s `safePost`).
        let sender = {
            let mut registry = CHANNEL_REGISTRY
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry
                .entry(channel.to_string())
                .or_insert_with(|| bus::Sender::channel(CHANNEL_CAPACITY).0)
                .clone()
        };
        let stop = Arc::new(AtomicBool::new(false));
        let mut droppers = Vec::new();
        for _ in 0..4 {
            let sender = sender.clone();
            let stop = Arc::clone(&stop);
            droppers.push(std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    drop(sender.subscribe());
                }
            }));
        }

        let deadline = std::time::Instant::now() + Duration::from_millis(400);
        while std::time::Instant::now() < deadline {
            publish_state(&a, channel);
        }

        stop.store(true, Ordering::Relaxed);
        for dropper in droppers {
            dropper.join().expect("dropper thread finishes");
        }

        // Release the channel so later tests start from a clean registry.
        CHANNEL_REGISTRY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(channel);
    }
}
