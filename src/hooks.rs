// src/hooks.rs
//! Reactive hooks for GPUI integration.

use crate::{ActivityEvent, QueryClient};
use gpui::{AppContext, AsyncApp, Context, Entity, Task, WeakEntity};

/// A reactive boolean that indicates if any query is fetching.
pub struct UseIsFetching {
    value: bool,
    _task: Task<()>,
}

impl UseIsFetching {
    /// Returns the current fetching status.
    pub fn get(&self) -> bool {
        self.value
    }
}

/// Create a reactive hook that tracks whether any query is fetching.
pub fn use_is_fetching<V: 'static>(
    cx: &mut Context<V>,
    client: &QueryClient,
) -> Entity<UseIsFetching> {
    let client = client.clone();
    let mut rx = client.subscribe_activity();
    let initial = client.is_fetching();

    cx.new(|cx: &mut Context<UseIsFetching>| {
        let task = cx.spawn(|this: WeakEntity<UseIsFetching>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                while let Ok(event) = rx.recv().await {
                    if let ActivityEvent::FetchingCountChanged(_) = event {
                        _ = this.update(
                            &mut cx,
                            |this: &mut UseIsFetching, cx: &mut Context<UseIsFetching>| {
                                this.value = client.is_fetching();
                                cx.notify();
                            },
                        );
                    }
                }
            }
        });
        UseIsFetching {
            value: initial,
            _task: task,
        }
    })
}

/// A reactive boolean that indicates if any mutation is in progress.
pub struct UseIsMutating {
    value: bool,
    _task: Task<()>,
}

impl UseIsMutating {
    /// Returns the current mutating status.
    pub fn get(&self) -> bool {
        self.value
    }
}

/// Create a reactive hook that tracks whether any mutation is in progress.
pub fn use_is_mutating<V: 'static>(
    cx: &mut Context<V>,
    client: &QueryClient,
) -> Entity<UseIsMutating> {
    let client = client.clone();
    let mut rx = client.subscribe_activity();
    let initial = client.is_mutating();

    cx.new(|cx: &mut Context<UseIsMutating>| {
        let task = cx.spawn(|this: WeakEntity<UseIsMutating>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                while let Ok(event) = rx.recv().await {
                    if let ActivityEvent::MutatingCountChanged(_) = event {
                        _ = this.update(
                            &mut cx,
                            |this: &mut UseIsMutating, cx: &mut Context<UseIsMutating>| {
                                this.value = client.is_mutating();
                                cx.notify();
                            },
                        );
                    }
                }
            }
        });
        UseIsMutating {
            value: initial,
            _task: task,
        }
    })
}

/// A reactive counter that tracks the number of in‑flight fetches.
pub struct UseFetchingCount {
    value: usize,
    _task: Task<()>,
}

impl UseFetchingCount {
    /// Returns the current fetching count.
    pub fn get(&self) -> usize {
        self.value
    }
}

/// Create a reactive hook that tracks the number of in‑flight fetch operations.
pub fn use_fetching_count<V: 'static>(
    cx: &mut Context<V>,
    client: &QueryClient,
) -> Entity<UseFetchingCount> {
    let client = client.clone();
    let mut rx = client.subscribe_activity();
    let initial = client.fetching_count();

    cx.new(|cx: &mut Context<UseFetchingCount>| {
        let task = cx.spawn(|this: WeakEntity<UseFetchingCount>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                while let Ok(event) = rx.recv().await {
                    if let ActivityEvent::FetchingCountChanged(count) = event {
                        _ = this.update(
                            &mut cx,
                            |this: &mut UseFetchingCount, cx: &mut Context<UseFetchingCount>| {
                                this.value = count;
                                cx.notify();
                            },
                        );
                    }
                }
            }
        });
        UseFetchingCount {
            value: initial,
            _task: task,
        }
    })
}

/// A reactive counter that tracks the number of in‑flight mutations.
pub struct UseMutatingCount {
    value: usize,
    _task: Task<()>,
}

impl UseMutatingCount {
    /// Returns the current mutating count.
    pub fn get(&self) -> usize {
        self.value
    }
}

/// Create a reactive hook that tracks the number of in‑flight mutation operations.
pub fn use_mutating_count<V: 'static>(
    cx: &mut Context<V>,
    client: &QueryClient,
) -> Entity<UseMutatingCount> {
    let client = client.clone();
    let mut rx = client.subscribe_activity();
    let initial = client.mutating_count();

    cx.new(|cx: &mut Context<UseMutatingCount>| {
        let task = cx.spawn(|this: WeakEntity<UseMutatingCount>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                while let Ok(event) = rx.recv().await {
                    if let ActivityEvent::MutatingCountChanged(count) = event {
                        _ = this.update(
                            &mut cx,
                            |this: &mut UseMutatingCount, cx: &mut Context<UseMutatingCount>| {
                                this.value = count;
                                cx.notify();
                            },
                        );
                    }
                }
            }
        });
        UseMutatingCount {
            value: initial,
            _task: task,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QueryClient;
    use gpui::{Empty, Render, TestAppContext, Window};

    struct TestView {
        is_fetching: Entity<UseIsFetching>,
        is_mutating: Entity<UseIsMutating>,
        fetching_count: Entity<UseFetchingCount>,
        mutating_count: Entity<UseMutatingCount>,
    }

    impl Render for TestView {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            Empty
        }
    }

    fn create_test_view(cx: &mut TestAppContext, client: &QueryClient) -> Entity<TestView> {
        let client = client.clone();
        cx.add_window_view(move |_window, cx| TestView {
            is_fetching: use_is_fetching(cx, &client),
            is_mutating: use_is_mutating(cx, &client),
            fetching_count: use_fetching_count(cx, &client),
            mutating_count: use_mutating_count(cx, &client),
        })
        .0
    }

    #[gpui::test]
    async fn test_use_is_fetching_reacts_to_activity(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let view = create_test_view(cx, &client);
        let entity = view.read_with(cx, |v, _| v.is_fetching.clone());

        assert!(!entity.read_with(cx, |e: &UseIsFetching, _| e.get()));

        client.inc_fetching();
        cx.run_until_parked();
        assert!(entity.read_with(cx, |e: &UseIsFetching, _| e.get()));

        client.dec_fetching();
        cx.run_until_parked();
        assert!(!entity.read_with(cx, |e: &UseIsFetching, _| e.get()));
    }

    #[gpui::test]
    async fn test_use_is_mutating_reacts_to_activity(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let view = create_test_view(cx, &client);
        let entity = view.read_with(cx, |v, _| v.is_mutating.clone());

        assert!(!entity.read_with(cx, |e: &UseIsMutating, _| e.get()));

        client.inc_mutating();
        cx.run_until_parked();
        assert!(entity.read_with(cx, |e: &UseIsMutating, _| e.get()));

        client.dec_mutating();
        cx.run_until_parked();
        assert!(!entity.read_with(cx, |e: &UseIsMutating, _| e.get()));
    }

    #[gpui::test]
    async fn test_use_fetching_count_reacts_to_activity(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let view = create_test_view(cx, &client);
        let entity = view.read_with(cx, |v, _| v.fetching_count.clone());

        assert_eq!(entity.read_with(cx, |e: &UseFetchingCount, _| e.get()), 0);

        client.inc_fetching();
        cx.run_until_parked();
        assert_eq!(entity.read_with(cx, |e: &UseFetchingCount, _| e.get()), 1);

        client.inc_fetching();
        cx.run_until_parked();
        assert_eq!(entity.read_with(cx, |e: &UseFetchingCount, _| e.get()), 2);

        client.dec_fetching();
        cx.run_until_parked();
        assert_eq!(entity.read_with(cx, |e: &UseFetchingCount, _| e.get()), 1);
    }

    #[gpui::test]
    async fn test_use_mutating_count_reacts_to_activity(cx: &mut TestAppContext) {
        let client = QueryClient::new();
        let view = create_test_view(cx, &client);
        let entity = view.read_with(cx, |v, _| v.mutating_count.clone());

        assert_eq!(entity.read_with(cx, |e: &UseMutatingCount, _| e.get()), 0);

        client.inc_mutating();
        cx.run_until_parked();
        assert_eq!(entity.read_with(cx, |e: &UseMutatingCount, _| e.get()), 1);

        client.inc_mutating();
        cx.run_until_parked();
        assert_eq!(entity.read_with(cx, |e: &UseMutatingCount, _| e.get()), 2);

        client.dec_mutating();
        cx.run_until_parked();
        assert_eq!(entity.read_with(cx, |e: &UseMutatingCount, _| e.get()), 1);
    }
}
