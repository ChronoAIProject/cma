//! Explicit custody for work that outlives its caller, never authorization.

use std::{future::Future, ops::Deref, sync::Arc};

use super::{custody::SharedCustody, ReservationBundle};

#[path = "scope_future.rs"]
mod owned_future;

tokio::task_local! {
    static CURRENT: IngressWorkScope;
}

#[derive(Clone, Debug)]
pub struct IngressWorkScope {
    storage: SharedCustody<ReservationBundle>,
}

impl IngressWorkScope {
    pub fn initialize_owned<T>(&self, initialize: impl FnOnce() -> T) -> OwnedWork<T> {
        OwnedWork {
            value: initialize(),
            scope: Some(self.clone()),
        }
    }
    pub fn current() -> Option<Self> {
        CURRENT.try_with(Clone::clone).ok()
    }

    /// Include this Arc and every overlapping child/result in the admitted plan.
    pub fn new(storage: ReservationBundle) -> Self {
        Self {
            storage: SharedCustody::new(storage),
        }
    }

    pub fn requested_bytes(&self) -> usize {
        self.storage.requested_bytes()
    }

    /// Reserve an additional phase, including its custody node, before building
    /// its storage. Every scope/result clone retains it until final release.
    /// This never waits for capacity or a concurrent custody update.
    pub fn admit_extra(
        &self,
        policy: &super::IngressOwnershipPolicy,
        class: super::IngressClass,
        plan: super::AllocationPlan,
    ) -> Result<(), super::AdmissionRefusal> {
        self.storage.admit_extra(policy, class, plan)
    }

    pub fn belongs_to(&self, policy: &super::IngressOwnershipPolicy) -> bool {
        self.storage.belongs_to(&policy.root)
    }

    /// Compare this scope's root and optional attachment exactly.
    /// Independently owned payloads require their own custody checks.
    pub fn shares_custody_domain(&self, policy: &super::IngressOwnershipPolicy) -> bool {
        self.storage.shares_custody_domain(&policy.root)
    }

    /// The plan must include this payload, its Arc and all overlapping siblings.
    pub fn initialize<T>(&self, initialize: impl FnOnce() -> T) -> super::SharedOwned<T> {
        super::SharedOwned {
            value: Arc::new(initialize()),
            storage: self.storage.clone(),
        }
    }

    pub async fn run<F: Future>(&self, future: F) -> OwnedWork<F::Output> {
        let value = CURRENT.scope(self.clone(), future).await;
        OwnedWork {
            value,
            scope: Some(self.clone()),
        }
    }

    /// Transport operations return status only. Payloads must transfer into an
    /// admitted socket envelope before this scope ends, including on cancellation.
    pub(crate) async fn run_io<F: Future<Output = crate::Result<()>>>(
        &self,
        future: F,
    ) -> crate::Result<()> {
        CURRENT.scope(self.clone(), future).await
    }

    /// Capture synchronously, before spawn. Tokio does not inherit task locals.
    /// A completed but unjoined task continues to own the returned envelope.
    pub fn inherit_owned<F: Future>(future: F) -> impl Future<Output = OwnedWork<F::Output>> {
        let scope = CURRENT.try_with(Clone::clone).ok();
        async move {
            match scope {
                Some(scope) => scope.run(future).await,
                None => OwnedWork {
                    value: future.await,
                    scope: None,
                },
            }
        }
    }
}

impl<T, E> OwnedWork<Result<T, E>> {
    /// Both success and error payloads keep the allocation phase's custody.
    pub fn transpose(self) -> Result<OwnedWork<T>, OwnedWork<E>> {
        match self.value {
            Ok(value) => Ok(OwnedWork {
                value,
                scope: self.scope,
            }),
            Err(value) => Err(OwnedWork {
                value,
                scope: self.scope,
            }),
        }
    }
}

/// Value precedes the guard both while running and in a completed JoinHandle.
#[derive(Debug)]
pub struct OwnedWork<T> {
    value: T,
    scope: Option<IngressWorkScope>,
}

impl OwnedWork<super::AdmittedError> {
    /// The error itself retains its source owner, independently of this scope.
    pub(crate) fn into_admitted_error(self) -> super::AdmittedError {
        self.value
    }
}

impl<T> OwnedWork<T> {
    /// Return into the exact active scope that already retains this storage.
    /// Callers must keep escaping results in that scope's owned envelope. A
    /// shared process root alone is not sufficient custody for this transfer.
    pub fn into_current(self) -> Result<T, Self> {
        let current = IngressWorkScope::current();
        let permitted = match (&self.scope, &current) {
            (Some(owner), Some(receiver)) => {
                SharedCustody::ptr_eq(&owner.storage, &receiver.storage)
            }
            (None, None) => true,
            _ => false,
        };
        if permitted {
            Ok(self.value)
        } else {
            Err(self)
        }
    }

    pub(crate) fn work_scope(&self) -> Option<IngressWorkScope> {
        self.scope.clone()
    }

    /// Transfer a completed result to immutable sharing without releasing custody.
    /// Its admitted plan must include the payload Arc's layout.
    pub fn into_shared(self) -> crate::Result<super::SharedOwned<T>> {
        self.into_shared_admitted().map_err(Into::into)
    }

    pub fn into_shared_admitted(self) -> Result<super::SharedOwned<T>, super::AdmissionRefusal> {
        let Some(scope) = self.scope else {
            return Err(super::AdmissionRefusal::InvalidPlan {
                phase: "unowned result sharing",
            });
        };
        Ok(scope.initialize(|| self.value))
    }

    pub fn belongs_to(&self, policy: &super::IngressOwnershipPolicy) -> bool {
        self.scope
            .as_ref()
            .is_some_and(|scope| scope.storage.belongs_to(&policy.root))
    }

    /// Compare the envelope's root and optional attachment, not nested owners.
    /// A result without an admitted scope never has a matching custody domain.
    pub fn shares_custody_domain(&self, policy: &super::IngressOwnershipPolicy) -> bool {
        self.scope
            .as_ref()
            .is_some_and(|scope| scope.shares_custody_domain(policy))
    }

    /// Admit result retention separately before releasing transformation capacity.
    /// Failure drops the attempted result with its original custody still held.
    pub fn retain(self, storage: ReservationBundle) -> crate::Result<Self> {
        self.retain_admitted(storage).map_err(Into::into)
    }

    pub fn retain_admitted(
        self,
        mut storage: ReservationBundle,
    ) -> Result<Self, super::AdmissionRefusal> {
        if !self
            .scope
            .as_ref()
            .is_some_and(|scope| storage.retain_attachment_from(&scope.storage))
        {
            return Err(super::AdmissionRefusal::InvalidPlan {
                phase: "result retention ownership",
            });
        }
        let retained = Self {
            value: self.value,
            scope: Some(IngressWorkScope::new(storage)),
        };
        drop(self.scope);
        Ok(retained)
    }

    /// Transfers custody through a transformation, retaining the same work plan.
    /// Any new simultaneous allocation must already be covered by that plan.
    pub fn map<U>(self, transform: impl FnOnce(T) -> U) -> OwnedWork<U> {
        OwnedWork {
            value: transform(self.value),
            scope: self.scope,
        }
    }

    pub fn is_admitted(&self) -> bool {
        self.scope.is_some()
    }
}

impl<T> Deref for OwnedWork<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

#[cfg(test)]
#[path = "scope_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "scope_transfer_tests.rs"]
mod transfer_tests;
