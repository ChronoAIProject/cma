//! Allocation custody; these envelopes confer no runtime or durable authority.

use std::{fmt, ops::Deref, sync::Arc};

use super::{AggregateReservation, AllocationPlan, IngressBudget};

pub use crate::runtime_output::RuntimeOutputDelivery;

/// Admission never waits, including when the caller already holds other owners.
pub struct ReservationBundle {
    reservation: AggregateReservation,
    phase: &'static str,
}

impl ReservationBundle {
    pub(super) fn belongs_to(&self, root: &IngressBudget) -> bool {
        Arc::ptr_eq(self.reservation.root.semaphore(), &root.0)
    }

    pub(super) fn shares_root(&self, other: &Self) -> bool {
        Arc::ptr_eq(
            self.reservation.root.semaphore(),
            other.reservation.root.semaphore(),
        )
    }

    pub(super) fn try_acquire(budget: &IngressBudget, plan: AllocationPlan) -> crate::Result<Self> {
        let reservation = budget.try_reserve(plan.requested_bytes()).ok_or_else(|| {
            tracing::debug!(phase = plan.phase(), "ingress allocation admission refused");
            crate::AdapterError::AtCapacity("ingress allocation capacity exhausted".into())
        })?;
        Ok(Self {
            reservation,
            phase: plan.phase(),
        })
    }

    pub fn requested_bytes(&self) -> usize {
        self.reservation.root.num_permits()
    }

    /// Transfer a proven sub-plan without briefly returning its capacity.
    pub fn split(&mut self, plan: AllocationPlan) -> crate::Result<Self> {
        let reservation = self
            .reservation
            .split(plan.requested_bytes())
            .ok_or_else(super::sizing::invalid)?;
        Ok(Self {
            reservation,
            phase: plan.phase(),
        })
    }

    /// The closure executes only after admission. Its output has no unowned exit.
    pub fn initialize<T>(self, initialize: impl FnOnce() -> T) -> SharedOwned<T> {
        SharedOwned {
            value: Arc::new(initialize()),
            storage: Arc::new(self),
        }
    }

    pub fn try_initialize<T, E>(
        self,
        initialize: impl FnOnce() -> Result<T, E>,
    ) -> Result<SharedOwned<T>, E> {
        let value = initialize()?;
        Ok(self.initialize(|| value))
    }
}

impl fmt::Debug for ReservationBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReservationBundle")
            .field("phase", &self.phase)
            .field("requested_bytes", &self.requested_bytes())
            .finish()
    }
}

/// Clones share physical storage. No mutable borrow or unowned extraction exists.
pub struct SharedOwned<T> {
    // Separate Arcs ensure the entire payload allocation, including its header,
    // is freed before the last reservation is released.
    pub(super) value: Arc<T>,
    pub(super) storage: Arc<ReservationBundle>,
}

impl<T> SharedOwned<T> {
    pub fn belongs_to(&self, policy: &super::IngressOwnershipPolicy) -> bool {
        self.storage.belongs_to(&policy.root)
    }

    pub fn requested_bytes(&self) -> usize {
        self.storage.requested_bytes()
    }
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.value, &other.value)
    }
}

impl<T> Clone for SharedOwned<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            storage: self.storage.clone(),
        }
    }
}

impl<T> Deref for SharedOwned<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T: fmt::Debug> fmt::Debug for SharedOwned<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(f)
    }
}

/// Transport-specific payloads remain inseparable from their pre-admitted owner.
pub type AdmittedFrame<T> = SharedOwned<T>;

/// Pinned codecs implement this port; post-read wrapping is not admission proof.
pub trait AdmittedSocket<T>:
    futures_util::Stream<Item = crate::Result<AdmittedFrame<T>>>
    + futures_util::Sink<AdmittedFrame<T>, Error = crate::AdapterError>
    + Unpin
    + Send
{
}

impl<T, S> AdmittedSocket<T> for S where
    S: futures_util::Stream<Item = crate::Result<AdmittedFrame<T>>>
        + futures_util::Sink<AdmittedFrame<T>, Error = crate::AdapterError>
        + Unpin
        + Send
{
}

#[cfg(test)]
#[path = "ownership_tests.rs"]
mod tests;
