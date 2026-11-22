use std::{ops::Deref, sync::Arc};

use super::{custody::SharedCustody, IngressReservation};

/// Shared payload storage that retains its component and aggregate reservation.
/// Allocations made through interior mutability still need separate admission.
pub struct SharedComponentOwned<T> {
    // Each clone frees its payload reference before its custody reference. The
    // last payload destructor therefore retains custody through physical free.
    value: Arc<T>,
    storage: SharedCustody<IngressReservation>,
}

impl IngressReservation {
    /// Transfer an acquired reservation without releasing or reacquiring permits.
    /// The caller must include sizing::component_shared_owner_bytes::<T>(),
    /// dynamic payload capacities and simultaneous initialization scratch in the
    /// original reservation. Partial initialization unwinds before that owner.
    pub fn initialize_shared<T>(self, initialize: impl FnOnce() -> T) -> SharedComponentOwned<T> {
        SharedComponentOwned {
            value: Arc::new(initialize()),
            storage: SharedCustody::new(self),
        }
    }
}

impl<T> Clone for SharedComponentOwned<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            storage: self.storage.clone(),
        }
    }
}

impl<T> Deref for SharedComponentOwned<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

#[cfg(test)]
#[path = "component_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "component_deallocation_tests.rs"]
mod deallocation_tests;
