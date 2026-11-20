use super::*;
use crate::ingress_budget::{
    AdmissionRefusal, AdmittedError, AdmittedOperation, AllocationPlan, IngressClass,
    IngressOwnershipPolicy,
};

impl HostedContextRefresh {
    pub(in crate::hosted) fn readiness_receipt_admitted<'a>(
        &'a self,
        session: &'a str,
        sequence: u64,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, Option<String>>, AdmittedError> {
        self.require_owner(policy)?;
        let owned_policy = policy.clone();
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Data,
            // receipt() streams borrowed fields into one fixed digest.
            AllocationPlan::checked("managed context receipt", &[64])?,
            move |scope| async move {
                let status = self.ready_status_admitted(&owned_policy)?.await?;
                Ok(scope.initialize_owned(|| {
                    status
                        .as_ref()
                        .map(|status| self.receipt(status, session, sequence))
                }))
            },
        )
    }

    pub(super) fn ready_status_admitted(
        &self,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'_, Option<RuntimeStatus>>, AdmittedError> {
        self.require_owner(policy)?;
        let owned_policy = policy.clone();
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Data,
            AllocationPlan::checked(
                "managed context status request",
                &[
                    self.0.identity.sandbox_id.len(),
                    self.0.identity.home_identity.len(),
                    // RuntimeStatus::validate_for can construct either fixed
                    // diagnostic. Successful validation does not allocate.
                    "runtime status identity is invalid"
                        .len()
                        .max("runtime status does not prove the requested identity".len()),
                ],
            )?,
            move |scope| async move {
                let request =
                    scope.initialize_owned(|| RequestFrame::RuntimeStatus(self.0.identity.clone()));
                let operation =
                    self.0
                        .backend
                        .exchange_admitted(&self.0.handle, request, &owned_policy)?;
                if !operation.belongs_to(&owned_policy) {
                    return Err(AdmissionRefusal::InvalidPlan {
                        phase: "context status source ownership",
                    }
                    .into());
                }
                let response = operation.await?;
                if !response.belongs_to(&owned_policy) {
                    return Err(AdmissionRefusal::InvalidPlan {
                        phase: "context status result ownership",
                    }
                    .into());
                }
                let ready = match &*response {
                    ResponseFrame::RuntimeStatus(status) => {
                        status.validate_for(&self.0.identity).is_ok()
                            && status.agent_filesystem
                            && status.credential_ready
                            && !status.restore_pending
                    }
                    _ => false,
                };
                // Move the validated status with its original source owner;
                // comparing two observations never needs an identity copy.
                Ok(response.map(|response| match response {
                    ResponseFrame::RuntimeStatus(status) if ready => Some(status),
                    _ => None,
                }))
            },
        )
    }
}

#[cfg(test)]
#[path = "receipt_admission_tests.rs"]
pub(in crate::hosted) mod tests;
