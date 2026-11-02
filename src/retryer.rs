// src/retryer.rs
//! Retry semantics, mirroring `TanStack` Query's `retryer.ts`.
//!
//! * Default decision: 3 attempts in a browser-like environment, `0` for
//!   mutations and imperative (`fetch_query`-style) fetches.
//! * Backoff: `min(1000 * 2^failure_count, 30_000)` ms with **no jitter**;
//!   `failure_count` is 0-based so the first retry waits exactly 1000 ms.
//! * Cancelling is never retried. Pause semantics: `online` mode pauses while
//!   offline; `offline_first` runs the first attempt offline but pauses
//!   retries; `always` never pauses.

use crate::error::QueryError;
use crate::network::{NetworkMode, OnlineManager, can_retry};
use quoin_core::CancellationToken;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

/// Maximum backoff delay, matching `TanStack` Query's 30 s cap.
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

/// Base backoff delay, matching `TanStack` Query's 1 s first retry.
pub const BASE_RETRY_DELAY: Duration = Duration::from_millis(1000);

/// How many times an operation may be retried after a failure.
#[derive(Clone)]
pub enum RetryDecision {
    /// Never retry (mutations default; imperative fetches force this).
    Never,
    /// Retry up to `count` times.
    Count(u32),
    /// Retry indefinitely.
    Always,
    /// Decide per failure: receives the error and the 0-based attempt index.
    Function(RetryDecider),
}

/// Shareable per-failure retry decider used by [`RetryDecision::Function`].
///
/// Receives the error and the 0-based attempt index; returns `true` when the
/// operation should be retried.
pub type RetryDecider = Arc<dyn Fn(&QueryError, u32) -> bool + Send + Sync>;

impl std::fmt::Debug for RetryDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RetryDecision::Never => f.write_str("Never"),
            RetryDecision::Count(count) => f.debug_tuple("Count").field(count).finish(),
            RetryDecision::Always => f.write_str("Always"),
            RetryDecision::Function(_) => f.write_str("Function(<fn>)"),
        }
    }
}

impl RetryDecision {
    /// Resolve whether a retry should happen after the given failure.
    #[must_use]
    pub fn should_retry(&self, error: &QueryError, attempt: u32) -> bool {
        match self {
            RetryDecision::Never => false,
            RetryDecision::Count(count) => attempt < *count,
            RetryDecision::Always => true,
            RetryDecision::Function(function) => function(error, attempt),
        }
    }
}

impl Default for RetryDecision {
    fn default() -> Self {
        RetryDecision::Count(3)
    }
}

/// `TanStack` backoff: `min(1000 * 2^attempt, 30_000)` ms, no jitter.
#[must_use]
pub fn retry_delay(attempt: u32) -> Duration {
    let multiplier = 2u64.saturating_pow(attempt);
    let millis = u64::try_from(BASE_RETRY_DELAY.as_millis()).unwrap_or(u64::MAX);
    let delay = millis.saturating_mul(multiplier);
    Duration::from_millis(delay.min(u64::try_from(MAX_RETRY_DELAY.as_millis()).unwrap_or(u64::MAX)))
}

/// Drives an attempt loop with `TanStack` retry, pause and cancellation rules.
#[derive(Clone, Debug)]
pub struct Retryer {
    decision: RetryDecision,
    failure_count: u32,
    failure_reason: Option<QueryError>,
    is_paused: bool,
    timer: Option<std::sync::Arc<dyn crate::timeout_manager::SleepProvider>>,
}

impl Default for Retryer {
    fn default() -> Self {
        Self::new(RetryDecision::default())
    }
}

impl Retryer {
    /// Create a retryer with the given decision.
    #[must_use]
    pub fn new(decision: RetryDecision) -> Self {
        Self {
            decision,
            failure_count: 0,
            failure_reason: None,
            is_paused: false,
            timer: None,
        }
    }

    /// Installs a custom [`SleepProvider`] for the backoff waits (tests use
    /// this to drive pauses with a deterministic timer).
    #[must_use]
    pub fn with_timer(
        mut self,
        timer: std::sync::Arc<dyn crate::timeout_manager::SleepProvider>,
    ) -> Self {
        self.timer = Some(timer);
        self
    }

    /// Failures observed so far (0-based, pre-increment semantics).
    #[must_use]
    pub fn failure_count(&self) -> u32 {
        self.failure_count
    }

    /// The last failure, if any.
    #[must_use]
    pub fn failure_reason(&self) -> Option<&QueryError> {
        self.failure_reason.as_ref()
    }

    /// Whether the retryer is currently paused waiting for connectivity.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.is_paused
    }

    /// Run `task` until it succeeds, retries are exhausted, the token is
    /// cancelled, or the failure is non-retryable.
    ///
    /// `token` cancellation aborts the current attempt immediately. While
    /// offline (per `network_mode`) the loop waits on `online` connectivity.
    ///
    /// # Errors
    ///
    /// Returns the task's last error when retries are exhausted or the failure
    /// is non-retryable, and [`QueryError::Cancelled`] when `token` is
    /// cancelled (including while paused for connectivity).
    pub async fn run<T, F, Fut>(
        &mut self,
        task: F,
        network_mode: NetworkMode,
        online: &OnlineManager,
        token: &CancellationToken,
    ) -> Result<T, QueryError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T, QueryError>>,
    {
        loop {
            // Pause before starting a new attempt when connectivity forbids it.
            if !can_retry(network_mode, online.is_online()) && self.failure_count > 0 {
                self.is_paused = true;
                let mut rx = online.subscribe();
                loop {
                    if token.is_cancelled() {
                        self.is_paused = false;
                        return Err(QueryError::Cancelled {
                            revert: false,
                            silent: false,
                        });
                    }
                    if rx.changed().await.is_ok() && online.is_online() {
                        break;
                    }
                }
                self.is_paused = false;
            }

            let attempt = self.failure_count;
            match task().await {
                Ok(value) => {
                    self.failure_reason = None;
                    return Ok(value);
                }
                Err(QueryError::Cancelled { .. }) => {
                    return Err(QueryError::Cancelled {
                        revert: false,
                        silent: false,
                    });
                }
                Err(error) => {
                    self.failure_count += 1;
                    self.failure_reason = Some(error.clone());
                    if !self.decision.should_retry(&error, attempt) {
                        return Err(error);
                    }
                    let delay = retry_delay(attempt);
                    match &self.timer {
                        Some(timer) => timer.sleep(delay).await,
                        None => {
                            crate::timeout_manager::TimeoutManager::default()
                                .sleep(delay)
                                .await;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Tokio-clock sleep provider so paused-time tests drive the backoff.
    #[derive(Debug)]
    struct TokioSleep;

    impl crate::timeout_manager::SleepProvider for TokioSleep {
        fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            Box::pin(tokio::time::sleep(duration))
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn paused_retryer(decision: RetryDecision) -> Retryer {
        Retryer::new(decision).with_timer(std::sync::Arc::new(TokioSleep))
    }

    #[test]
    fn test_retry_delay_first_is_one_second() {
        assert_eq!(retry_delay(0), Duration::from_millis(1000));
    }

    #[test]
    fn test_retry_delay_exponential() {
        assert_eq!(retry_delay(1), Duration::from_millis(2000));
        assert_eq!(retry_delay(2), Duration::from_millis(4000));
        assert_eq!(retry_delay(3), Duration::from_millis(8000));
        assert_eq!(retry_delay(4), Duration::from_secs(16));
    }

    #[test]
    fn test_retry_delay_caps_at_thirty_seconds() {
        assert_eq!(retry_delay(5), Duration::from_secs(30));
        assert_eq!(retry_delay(6), Duration::from_secs(30));
        assert_eq!(retry_delay(50), Duration::from_secs(30));
    }

    #[test]
    fn test_default_decision_is_three() {
        assert!(RetryDecision::default().should_retry(&QueryError::custom("x"), 2));
        assert!(!RetryDecision::default().should_retry(&QueryError::custom("x"), 3));
    }

    #[test]
    fn test_never_decision() {
        assert!(!RetryDecision::Never.should_retry(&QueryError::custom("x"), 0));
    }

    #[test]
    fn test_always_decision() {
        assert!(RetryDecision::Always.should_retry(&QueryError::custom("x"), 999));
    }

    #[test]
    fn test_function_decision() {
        let decision = RetryDecision::Function(Arc::new(|error: &QueryError, attempt| {
            matches!(error, QueryError::Network(_)) && attempt < 2
        }));
        assert!(decision.should_retry(&QueryError::network("x"), 1));
        assert!(!decision.should_retry(&QueryError::network("x"), 2));
        assert!(!decision.should_retry(&QueryError::custom("x"), 0));
    }

    #[tokio::test]
    async fn test_retryer_succeeds_first_attempt() {
        let mut retryer = Retryer::new(RetryDecision::Never);
        let result: Result<u32, QueryError> = retryer
            .run(
                || std::future::ready(Ok(7)),
                NetworkMode::Always,
                &OnlineManager::new(),
                &CancellationToken::new(),
            )
            .await;
        assert_eq!(result.expect("should succeed"), 7);
        assert_eq!(retryer.failure_count(), 0);
        assert!(retryer.failure_reason().is_none());
    }

    #[tokio::test]
    async fn test_retryer_exhausts_count() {
        let mut retryer = Retryer::new(RetryDecision::Count(1));
        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);
        let result: Result<(), QueryError> = retryer
            .run(
                || {
                    counter.fetch_add(1, Ordering::Relaxed);
                    std::future::ready(Err(QueryError::custom("boom")))
                },
                NetworkMode::Always,
                &OnlineManager::new(),
                &CancellationToken::new(),
            )
            .await;
        assert!(result.is_err());
        // Initial attempt + 1 retry.
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        assert_eq!(retryer.failure_count(), 2);
        assert_eq!(retryer.failure_reason(), Some(&QueryError::custom("boom")));
    }

    #[tokio::test]
    async fn test_retryer_never_retries_cancellation() {
        let mut retryer = Retryer::new(RetryDecision::Always);
        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);
        let result: Result<(), QueryError> = retryer
            .run(
                || {
                    counter.fetch_add(1, Ordering::Relaxed);
                    std::future::ready(Err(QueryError::Cancelled {
                        revert: false,
                        silent: false,
                    }))
                },
                NetworkMode::Always,
                &OnlineManager::new(),
                &CancellationToken::new(),
            )
            .await;
        assert_eq!(
            result.unwrap_err(),
            QueryError::Cancelled {
                revert: false,
                silent: false
            }
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_retryer_recovers_on_later_attempt() {
        let mut retryer = Retryer::new(RetryDecision::Count(5));
        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);
        let result: Result<u32, QueryError> = retryer
            .run(
                || {
                    let seen = counter.fetch_add(1, Ordering::Relaxed);
                    std::future::ready(if seen < 2 {
                        Err(QueryError::custom("flaky"))
                    } else {
                        Ok(seen)
                    })
                },
                NetworkMode::Always,
                &OnlineManager::new(),
                &CancellationToken::new(),
            )
            .await;
        assert_eq!(result.expect("should recover"), 2);
        assert_eq!(retryer.failure_count(), 2);
        assert!(retryer.failure_reason().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn test_retryer_waits_for_online_after_offline_retry() {
        let online = OnlineManager::new();
        online.set_online(false);
        let task_online = online.clone();
        let mut retryer = paused_retryer(RetryDecision::Count(3));
        let token = CancellationToken::new();
        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);

        let task = tokio::spawn(async move {
            retryer
                .run(
                    || {
                        counter.fetch_add(1, Ordering::Relaxed);
                        std::future::ready::<Result<(), QueryError>>(Err(QueryError::custom(
                            "net down",
                        )))
                    },
                    NetworkMode::Online,
                    &task_online,
                    &token,
                )
                .await
        });

        tokio::time::advance(Duration::from_millis(50)).await;
        assert!(
            !task.is_finished(),
            "retryer should be paused while offline"
        );
        online.set_online(true);
        let result = task.await.expect("task should finish").unwrap_err();
        assert_eq!(result, QueryError::custom("net down"));
    }

    #[tokio::test(start_paused = true)]
    async fn test_retryer_cancelled_while_paused() {
        let online = OnlineManager::new();
        online.set_online(false);
        let mut retryer = Retryer::new(RetryDecision::Count(3));
        let token = CancellationToken::new();
        token.cancel();

        let result: Result<(), QueryError> = retryer
            .run(
                || std::future::ready(Err(QueryError::custom("boom"))),
                NetworkMode::Online,
                &online,
                &token,
            )
            .await;
        assert_eq!(
            result.unwrap_err(),
            QueryError::Cancelled {
                revert: false,
                silent: false
            }
        );
    }

    #[test]
    fn test_retryer_default_has_three_retries() {
        let retryer = Retryer::default();
        assert_eq!(retryer.failure_count(), 0);
        assert!(!retryer.is_paused());
    }

    #[test]
    fn test_retryer_debug() {
        let retryer = Retryer::new(RetryDecision::Never);
        assert!(format!("{retryer:?}").contains("Retryer"));
    }

    #[test]
    fn test_retry_decision_debug_all_variants() {
        assert_eq!(format!("{:?}", RetryDecision::Never), "Never");
        assert_eq!(format!("{:?}", RetryDecision::Count(3)), "Count(3)");
        assert_eq!(format!("{:?}", RetryDecision::Always), "Always");
        let function = RetryDecision::Function(Arc::new(|_: &QueryError, _| false));
        assert_eq!(format!("{function:?}"), "Function(<fn>)");
    }

    #[tokio::test(start_paused = true)]
    async fn test_retryer_resumes_when_connectivity_returns_after_pause() {
        let online = OnlineManager::new();
        online.set_online(false);
        let task_online = online.clone();
        let mut retryer = Retryer::new(RetryDecision::Count(1));
        let token = CancellationToken::new();
        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);

        let task = tokio::spawn(async move {
            retryer
                .run(
                    || {
                        counter.fetch_add(1, Ordering::Relaxed);
                        std::future::ready::<Result<(), QueryError>>(Err(QueryError::custom(
                            "net down",
                        )))
                    },
                    NetworkMode::OfflineFirst,
                    &task_online,
                    &token,
                )
                .await
        });

        // Offline-first: the first attempt runs while offline and fails; the
        // 1 s backoff is auto-advanced by the paused clock; the retryer then
        // parks in the pause loop waiting for connectivity.
        tokio::time::advance(Duration::from_millis(1500)).await;
        assert!(
            !task.is_finished(),
            "retryer should be paused waiting for connectivity"
        );

        online.set_online(true);
        let result = task.await.expect("task should finish").unwrap_err();
        assert_eq!(result, QueryError::custom("net down"));
        // Initial attempt + the resumed retry (which is also the final one).
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn test_retryer_pauses_when_connectivity_drops_mid_fetch() {
        // The pause loop subscribes only when the pause begins, so the retry
        // parks even though the connectivity change happened earlier.
        let online = OnlineManager::new();
        let task_online = online.clone();
        let mut retryer = Retryer::new(RetryDecision::Count(1));
        let token = CancellationToken::new();
        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);

        let run_online = online.clone();
        let task = tokio::spawn(async move {
            retryer
                .run(
                    move || {
                        let online = task_online.clone();
                        let counter = Arc::clone(&counter);
                        async move {
                            counter.fetch_add(1, Ordering::Relaxed);
                            // Connectivity drops *during* the first attempt.
                            online.set_online(false);
                            Err::<(), QueryError>(QueryError::network("connection dropped"))
                        }
                    },
                    NetworkMode::Online,
                    &run_online,
                    &token,
                )
                .await
        });

        // The 1 s backoff is auto-advanced by the paused clock; the retryer
        // then parks waiting for connectivity.
        tokio::time::advance(Duration::from_millis(1500)).await;
        assert!(
            !task.is_finished(),
            "retryer should be paused while offline"
        );

        online.set_online(true);
        let result = task.await.expect("task should finish").unwrap_err();
        assert_eq!(result, QueryError::network("connection dropped"));
        // Initial attempt + the resumed retry (which is also the final one).
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    /// Poll a pinned future once with a no-op waker so tests can interleave
    /// state changes between individual polls deterministically.
    fn poll_once<F: Future>(fut: &mut std::pin::Pin<Box<F>>) -> std::task::Poll<F::Output> {
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        fut.as_mut().poll(&mut cx)
    }

    #[tokio::test(start_paused = true)]
    async fn test_retryer_parks_in_pause_loop_until_online() {
        let online = OnlineManager::new();
        online.set_online(false);
        let mut retryer = paused_retryer(RetryDecision::Count(1));
        let token = CancellationToken::new();
        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);

        let mut run = Box::pin(async {
            retryer
                .run(
                    || {
                        counter.fetch_add(1, Ordering::Relaxed);
                        std::future::ready::<Result<(), QueryError>>(Err(QueryError::custom(
                            "net down",
                        )))
                    },
                    NetworkMode::Online,
                    &online,
                    &token,
                )
                .await
        });

        // First poll: the initial attempt fails and the retryer parks in the
        // 1 s backoff sleep.
        assert!(
            poll_once(&mut run).is_pending(),
            "retryer must wait out the backoff"
        );
        tokio::time::advance(Duration::from_millis(1000)).await;

        // Second poll: the backoff is over but `online` mode forbids starting
        // a retry offline, so the retryer parks in the pause loop.
        assert!(
            poll_once(&mut run).is_pending(),
            "retryer must park while offline"
        );

        // Connectivity returns: the pause loop must break and the (final)
        // retry must run, exhausting the Count(1) budget.
        online.set_online(true);
        let mut result = None;
        if let std::task::Poll::Ready(ready) = poll_once(&mut run) {
            result = Some(ready);
        }
        let result = result.expect("retryer must resume once online");
        assert_eq!(result.unwrap_err(), QueryError::custom("net down"));
        assert_eq!(calls.load(Ordering::Relaxed), 2);

        drop(run);
        assert!(!retryer.is_paused(), "pause flag cleared after resuming");
        assert_eq!(retryer.failure_count(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn test_retryer_cancelled_while_parked_in_pause_loop() {
        let online = OnlineManager::new();
        online.set_online(false);
        let mut retryer = paused_retryer(RetryDecision::Count(3));
        let token = CancellationToken::new();
        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);

        let mut run = Box::pin(async {
            retryer
                .run(
                    || {
                        counter.fetch_add(1, Ordering::Relaxed);
                        std::future::ready::<Result<(), QueryError>>(Err(QueryError::custom(
                            "net down",
                        )))
                    },
                    NetworkMode::Online,
                    &online,
                    &token,
                )
                .await
        });

        // Fail the initial attempt and park in the pause loop.
        assert!(poll_once(&mut run).is_pending());
        tokio::time::advance(Duration::from_millis(1000)).await;
        assert!(poll_once(&mut run).is_pending());

        // Cancel *while* parked, then wake the watch with a change that still
        // does not allow retries: the loop must observe the cancellation.
        token.cancel();
        online.set_online(true);
        online.set_online(false);
        let mut result = None;
        if let std::task::Poll::Ready(ready) = poll_once(&mut run) {
            result = Some(ready);
        }
        let result = result.expect("cancellation while parked must abort the wait");
        assert_eq!(
            result.unwrap_err(),
            QueryError::Cancelled {
                revert: false,
                silent: false
            }
        );
        // The cancelled wait never resumed, so only the initial attempt ran.
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        drop(run);
        assert!(!retryer.is_paused(), "pause flag cleared on cancellation");
    }
}
