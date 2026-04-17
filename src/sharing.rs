// src/sharing.rs
//! Structural sharing utilities

use std::any::Any;

/// Trait for types that can be compared for equality dynamically.
/// Object-safe and extends `Any` for downcasting.
pub trait PartialEqAny: Any {
    fn as_any(&self) -> &dyn Any;
    fn eq_any(&self, other: &dyn Any) -> bool;
}

impl<T: PartialEq + 'static> PartialEqAny for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn eq_any(&self, other: &dyn Any) -> bool {
        other.downcast_ref::<T>().map_or(false, |o| self == o)
    }
}
