//! Admission before constructing an erased source future and its working data.

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use super::{
    sizing, AdmittedError, AllocationPlan, IngressClass, IngressOwnershipPolicy, IngressWorkScope,
    OwnedWork,
};

/// A source port returns this synchronously, so an unsupported implementation
/// can refuse before constructing a boxed async-trait future. This owns future
/// storage, not executor task storage or authority to perform an operation.
pub struct AdmittedOperation<'a, T> {
    future: Pin<Box<dyn Future<Output = Result<OwnedWork<T>, AdmittedError>> + Send + 'a>>,
    // The box must be freed before its allowance; a guard inside the future
    // alone would drop before the executor freed that future's allocation.
    storage: IngressWorkScope,
}

impl<'a, T: Send + 'a> AdmittedOperation<'a, T> {
    /// `working` covers every source allocation, decode/working overlap and
    /// returned payload. This adds the compiler's exact future layout and scope
    /// owner before boxing. The initializer is invoked only on the first poll;
    /// any allocations it performs must already fit in `working`.
    pub fn new<F, I>(
        policy: &IngressOwnershipPolicy,
        class: IngressClass,
        working: AllocationPlan,
        initialize: I,
    ) -> Result<Self, AdmittedError>
    where
        F: Future<Output = crate::Result<T>> + Send + 'a,
        I: FnOnce(IngressWorkScope) -> F + Send + 'a,
    {
        let owned_policy = policy.clone();
        let build = move |scope: IngressWorkScope| async move {
            scope
                .initialize_owned(|| initialize(scope.clone()))
                .resolve_admitted_result(&owned_policy, class)
                .await
        };
        Self::allocate(policy, class, working, build)
    }

    /// Compose admitted producers without copying their payload or erasing
    /// error custody. Each returned value/error must own its complete lifetime;
    /// temporary work and the future box remain charged to this operation.
    /// This does not install a legacy diagnostic context: legacy calls must use
    /// `new` or `resolve_admitted_result` with their own complete working plan.
    pub fn from_owned<F, I>(
        policy: &IngressOwnershipPolicy,
        class: IngressClass,
        working: AllocationPlan,
        initialize: I,
    ) -> Result<Self, AdmittedError>
    where
        F: Future<Output = Result<OwnedWork<T>, AdmittedError>> + Send + 'a,
        I: FnOnce(IngressWorkScope) -> F + Send + 'a,
    {
        let owned_policy = policy.clone();
        Self::allocate(policy, class, working, move |scope| async move {
            let result = initialize(scope).await;
            let valid = match &result {
                Ok(value) => value.belongs_to(&owned_policy),
                Err(error) => error.belongs_to(&owned_policy),
            };
            if !valid {
                return Err(super::AdmissionRefusal::InvalidPlan {
                    phase: "source result ownership",
                }
                .into());
            }
            result
        })
    }

    fn allocate<F>(
        policy: &IngressOwnershipPolicy,
        class: IngressClass,
        working: AllocationPlan,
        build: impl FnOnce(IngressWorkScope) -> F,
    ) -> Result<Self, AdmittedError>
    where
        F: Future<Output = Result<OwnedWork<T>, AdmittedError>> + Send + 'a,
    {
        let plan = AllocationPlan::checked(
            working.phase(),
            &[
                working.requested_bytes(),
                future_bytes(&build),
                sizing::shared_owner_bytes::<()>(),
            ],
        )?;
        let storage = IngressWorkScope::new(policy.admit(class, plan)?);
        Ok(Self {
            future: Box::pin(build(storage.clone())),
            storage,
        })
    }
}

impl<T> AdmittedOperation<'_, T> {
    pub fn belongs_to(&self, policy: &IngressOwnershipPolicy) -> bool {
        self.storage.belongs_to(policy)
    }

    /// Compare the future storage's root and optional attachment before polling.
    /// Returned envelopes, payloads and dynamic errors need separate checks.
    pub fn shares_custody_domain(&self, policy: &IngressOwnershipPolicy) -> bool {
        self.storage.shares_custody_domain(policy)
    }

    pub fn requested_bytes(&self) -> usize {
        self.storage.requested_bytes()
    }
}

impl<T> Future for AdmittedOperation<'_, T> {
    type Output = Result<OwnedWork<T>, AdmittedError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.future.as_mut().poll(cx)
    }
}

impl<T> std::fmt::Debug for AdmittedOperation<'_, T> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("AdmittedOperation")
            .field("requested_bytes", &self.requested_bytes())
            .finish_non_exhaustive()
    }
}

fn future_bytes<F: Future>(_: &impl FnOnce(IngressWorkScope) -> F) -> usize {
    size_of::<F>()
}

#[cfg(test)]
#[path = "operation_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "operation_composition_tests.rs"]
mod composition_tests;
