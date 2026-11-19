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
        let owned_policy = policy.clone();
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Data,
            AllocationPlan::checked(
                "managed context receipt request and result",
                &[
                    self.0.identity.sandbox_id.len(),
                    self.0.identity.home_identity.len(),
                    64, // receipt() streams borrowed fields into one fixed digest.
                    // RuntimeStatus::validate_for can construct either fixed
                    // diagnostic. It does not allocate on successful validation.
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
                // A response alone cannot prove readiness. Match the original
                // status identity and prerequisites before issuing its receipt.
                let ready = match &*response {
                    ResponseFrame::RuntimeStatus(status) => {
                        status.validate_for(&self.0.identity).is_ok()
                            && status.agent_filesystem
                            && status.credential_ready
                            && !status.restore_pending
                    }
                    _ => false,
                };
                Ok(scope.initialize_owned(|| match &*response {
                    ResponseFrame::RuntimeStatus(status) if ready => {
                        Some(self.receipt(status, session, sequence))
                    }
                    _ => None,
                }))
            },
        )
    }
}

#[cfg(test)]
#[path = "receipt_admission_tests.rs"]
pub(in crate::hosted) mod tests;
