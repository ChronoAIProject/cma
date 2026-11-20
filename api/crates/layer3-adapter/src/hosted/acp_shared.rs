//! Shared container allocation custody; payload growth has its own admission.

use std::{ops::Deref, sync::Arc};

use crate::ingress_budget::{
    sizing, AdmissionRefusal, AllocationPlan, IngressClass, IngressOwnershipPolicy, SharedOwned,
};

pub(in crate::hosted) enum Shared<T: ?Sized> {
    Legacy(Arc<T>),
    Admitted(SharedOwned<T>),
}

impl<T> Shared<T> {
    /// Admit the outer allocation before constructing it. Any heap storage
    /// already owned by value, or grown through its interior mutability, needs
    /// its own owner; this reservation covers only this Arc and its custody.
    pub(super) fn new(
        value: T,
        policy: Option<&IngressOwnershipPolicy>,
        phase: &'static str,
    ) -> Result<Self, AdmissionRefusal> {
        match policy {
            Some(policy) => Ok(Self::Admitted(
                policy
                    .admit(
                        IngressClass::Data,
                        AllocationPlan::checked(phase, &[sizing::shared_owner_bytes::<T>()])?,
                    )?
                    .initialize(|| value),
            )),
            None => Ok(Self::Legacy(Arc::new(value))),
        }
    }

    #[cfg(test)]
    pub(super) fn requested_bytes(&self) -> usize {
        match self {
            Self::Legacy(_) => 0,
            Self::Admitted(value) => value.requested_bytes(),
        }
    }
}

impl<T: ?Sized> Shared<T> {
    pub(in crate::hosted) fn belongs_to(&self, policy: &IngressOwnershipPolicy) -> bool {
        matches!(self, Self::Admitted(value) if value.belongs_to(policy))
    }

    pub(in crate::hosted) fn ptr_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Legacy(left), Self::Legacy(right)) => Arc::ptr_eq(left, right),
            (Self::Admitted(left), Self::Admitted(right)) => left.ptr_eq(right),
            _ => false,
        }
    }
}

impl<T: ?Sized> Clone for Shared<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Legacy(value) => Self::Legacy(value.clone()),
            Self::Admitted(value) => Self::Admitted(value.clone()),
        }
    }
}

impl<T: ?Sized> Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            Self::Legacy(value) => value,
            Self::Admitted(value) => value,
        }
    }
}

#[cfg(test)]
#[path = "acp_shared_tests.rs"]
mod tests;
