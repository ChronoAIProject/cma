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
