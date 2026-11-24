use super::SubscriptionError;
use crate::connections::subscription_store::SubscriptionOperation;
use layer3_adapter::ingress_budget::{
    AdmissionRefusal, AdmittedError, IngressOwnershipPolicy, OwnedWork, SharedOwned,
};
use std::borrow::Borrow;

pub(super) struct Retained<T>(pub(super) SharedOwned<T>);

impl<T> Borrow<T> for Retained<T> {
    fn borrow(&self) -> &T {
        &self.0
    }
}
impl<T> Clone for Retained<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

pub(super) fn invalid() -> AdmittedError {
    AdmissionRefusal::InvalidPlan {
        phase: "subscription refresh custody",
    }
    .into()
}

pub(super) fn check<T>(
    value: &SharedOwned<T>,
    policy: &IngressOwnershipPolicy,
) -> Result<(), AdmittedError> {
    if value.shares_custody_domain(policy) {
        Ok(())
    } else {
        Err(invalid())
    }
}

pub(super) fn error(error: AdmittedError, policy: &IngressOwnershipPolicy) -> AdmittedError {
    match &error {
        AdmittedError::Adapter(payload) if !payload.shares_custody_domain(policy) => invalid(),
        _ => error,
    }
}

pub(super) async fn source<T>(
    operation: Result<SubscriptionOperation<'_, T>, AdmittedError>,
    policy: &IngressOwnershipPolicy,
) -> Result<OwnedWork<Result<T, SubscriptionError>>, AdmittedError> {
    let operation = operation.map_err(|e| error(e, policy))?;
    if !operation.shares_custody_domain(policy) {
        return Err(invalid());
    }
    let result = operation.await.map_err(|e| error(e, policy))?;
    if !result.shares_custody_domain(policy) {
        return Err(invalid());
    }
    Ok(result)
}
