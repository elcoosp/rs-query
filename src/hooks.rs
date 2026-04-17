// src/hooks.rs
//! Reactive hooks for GPUI integration.
//! NOTE: The GPUI version in use does not provide `Model`. Hooks are stubbed.
//! For reactive updates, consider using `cx.notify()` in combination with
//! `client.subscribe_activity()` directly in your view.

use crate::QueryClient;
use gpui::Context;

/// Stub: returns the current fetching status (non‑reactive).
pub fn use_is_fetching<V: 'static>(_cx: &mut Context<V>, client: &QueryClient) -> bool {
    client.is_fetching()
}

/// Stub: returns the current mutating status (non‑reactive).
pub fn use_is_mutating<V: 'static>(_cx: &mut Context<V>, client: &QueryClient) -> bool {
    client.is_mutating()
}

/// Stub: returns the current fetching count (non‑reactive).
pub fn use_fetching_count<V: 'static>(_cx: &mut Context<V>, client: &QueryClient) -> usize {
    client.fetching_count()
}

/// Stub: returns the current mutating count (non‑reactive).
pub fn use_mutating_count<V: 'static>(_cx: &mut Context<V>, client: &QueryClient) -> usize {
    client.mutating_count()
}
