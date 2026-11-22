//! Allocation custody; these envelopes confer no runtime or durable authority.

use std::{fmt, ops::Deref, sync::Arc};

use super::{
    custody::SharedCustody, AdmissionRefusal, AggregateReservation, AllocationPlan, IngressBudget,
};

pub use crate::runtime_output::{AdmittedAckError, RuntimeOutputDelivery};

#[path = "extension.rs"]
mod extension;

/// Admission never waits, including when the caller already holds other owners.
pub struct ReservationBundle {
    extensions: extension::Extensions,
    reservation: AggregateReservation,
    phase: &'static str,
}

impl ReservationBundle {
    pub(super) fn belongs_to(&self, root: &IngressBudget) -> bool {
        Arc::ptr_eq(self.reservation.root.semaphore(), &root.0)
            && root.3.as_ref().is_none_or(|expected| {
                self.reservation
                    .attachment
                    .as_ref()
                    .is_some_and(|actual| SharedCustody::ptr_eq(actual, expected))
            })
    }

    pub(super) fn shares_custody_domain(&self, root: &IngressBudget) -> bool {
        Arc::ptr_eq(self.reservation.root.semaphore(), &root.0)
            && match (&self.reservation.attachment, &root.3) {
                (None, None) => true,
                (Some(actual), Some(expected)) => SharedCustody::ptr_eq(actual, expected),
                _ => false,
            }
    }

    pub(super) fn shares_root(&self, other: &Self) -> bool {
        Arc::ptr_eq(
            self.reservation.root.semaphore(),
            other.reservation.root.semaphore(),
        )
    }

    pub(super) fn retain_attachment_from(&mut self, source: &Self) -> bool {
        if !self.shares_root(source) {
            return false;
        }
        match (
            &mut self.reservation.attachment,
            &source.reservation.attachment,
        ) {
            (Some(target), Some(source)) => SharedCustody::ptr_eq(target, source),
            (target @ None, source) => {
                *target = source.clone();
                true
            }
            (Some(_), None) => true,
        }
    }

    pub(super) fn try_acquire(budget: &IngressBudget, plan: AllocationPlan) -> crate::Result<Self> {
        Self::try_admit(budget, plan).map_err(|refusal| {
            tracing::debug!(phase = plan.phase(), "ingress allocation admission refused");
            refusal.into()
        })
    }

    pub(super) fn try_admit(
        budget: &IngressBudget,
        plan: AllocationPlan,
    ) -> Result<Self, AdmissionRefusal> {
        let reservation =
            budget
                .try_reserve(plan.requested_bytes())
                .ok_or(AdmissionRefusal::AtCapacity {
                    phase: plan.phase(),
                })?;
        Ok(Self {
            extensions: extension::Extensions::default(),
            reservation,
            phase: plan.phase(),
        })
    }

    pub fn requested_bytes(&self) -> usize {
        self.reservation.root.num_permits() + self.extensions.requested_bytes()
    }

    pub(super) fn admit_extra(
        &self,
        policy: &super::IngressOwnershipPolicy,
        class: super::IngressClass,
        plan: AllocationPlan,
    ) -> Result<(), AdmissionRefusal> {
        if !self.belongs_to(&policy.root) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "extended allocation ownership",
            });
        }
        self.extensions.admit(policy, class, plan)
    }

    /// Transfer a proven sub-plan without briefly returning its capacity.
    pub fn split(&mut self, plan: AllocationPlan) -> crate::Result<Self> {
        self.split_admitted(plan).map_err(Into::into)
    }

    pub fn split_admitted(&mut self, plan: AllocationPlan) -> Result<Self, AdmissionRefusal> {
        let reservation = self.reservation.split(plan.requested_bytes()).ok_or(
            AdmissionRefusal::InvalidPlan {
                phase: plan.phase(),
            },
        )?;
        Ok(Self {
            extensions: extension::Extensions::default(),
            reservation,
            phase: plan.phase(),
        })
    }

    /// The closure executes only after admission. Its output has no unowned exit.
    pub fn initialize<T>(self, initialize: impl FnOnce() -> T) -> SharedOwned<T> {
        SharedOwned {
            value: Arc::new(initialize()),
            storage: SharedCustody::new(self),
        }
    }

    /// Admit the larger success/error owner layout and all initialization scratch.
    /// Either returned branch retains the original reservation without reacquiring.
    pub fn try_initialize<T, E>(
        self,
        initialize: impl FnOnce() -> Result<T, E>,
    ) -> Result<SharedOwned<T>, SharedOwned<E>> {
        match initialize() {
            Ok(value) => Ok(self.initialize(|| value)),
            Err(error) => Err(self.initialize(|| error)),
        }
    }

    /// Only this concrete, allocation-free failure may leave without an owner.
    /// Partial initialization must free its storage before returning the refusal.
    pub fn try_initialize_static<T>(
        self,
        initialize: impl FnOnce() -> Result<T, AdmissionRefusal>,
    ) -> Result<SharedOwned<T>, AdmissionRefusal> {
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
pub struct SharedOwned<T: ?Sized> {
    // Drop payload storage first. Custody then frees its own Arc backing before
    // releasing the extracted reservation, including during racing final drops.
    pub(super) value: Arc<T>,
    pub(super) storage: SharedCustody<ReservationBundle>,
}

impl<T: ?Sized> SharedOwned<T> {
    /// Check the root and any attachment required by the receiving policy.
    pub fn belongs_to(&self, policy: &super::IngressOwnershipPolicy) -> bool {
        self.storage.belongs_to(&policy.root)
    }

    /// Independent copies must retain the same root and optional attachment.
    /// A process policy cannot replace an attached source's lifetime owner.
    pub fn shares_custody_domain(&self, policy: &super::IngressOwnershipPolicy) -> bool {
        self.storage.shares_custody_domain(&policy.root)
    }

    pub fn requested_bytes(&self) -> usize {
        self.storage.requested_bytes()
    }
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.value, &other.value)
    }

    #[cfg(test)]
    pub(crate) fn strong_count(&self) -> usize {
        Arc::strong_count(&self.value)
    }
}

impl<T: ?Sized> Clone for SharedOwned<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            storage: self.storage.clone(),
        }
    }
}

impl<T: ?Sized> Deref for SharedOwned<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T: fmt::Debug + ?Sized> fmt::Debug for SharedOwned<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(f)
    }
}

/// Transport-specific payloads remain inseparable from their pre-admitted owner.
pub type AdmittedFrame<T> = SharedOwned<T>;

/// Pinned codecs implement this port; post-read wrapping is not admission proof.
/// Errors also require pre-admission and custody, including failures produced
/// during readiness, send, flush and close without an initiating caller scope.
pub trait AdmittedSocket<T>:
    futures_util::Stream<Item = Result<AdmittedFrame<T>, super::AdmittedError>>
    + futures_util::Sink<AdmittedFrame<T>, Error = super::AdmittedError>
    + Unpin
    + Send
{
}

impl<T, S> AdmittedSocket<T> for S where
    S: futures_util::Stream<Item = Result<AdmittedFrame<T>, super::AdmittedError>>
        + futures_util::Sink<AdmittedFrame<T>, Error = super::AdmittedError>
        + Unpin
        + Send
{
}

#[cfg(test)]
#[path = "ownership_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "custody_tests.rs"]
mod custody_tests;

#[cfg(test)]
#[path = "initialization_tests.rs"]
mod initialization_tests;
