use super::*;
use crate::ingress_budget::{
    sizing, AdmissionRefusal, AdmittedError, AdmittedOperation, AllocationPlan, IngressClass,
    IngressOwnershipPolicy, IngressWorkScope, OwnedWork,
};

pub(super) const MAX_INSTRUCTIONS: usize = 32 * 1024;

impl HostedContextRefresh {
    pub(in crate::hosted) fn confirm_instructions_admitted<'a>(
        &'a self,
        expected: &'a str,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, ()>, AdmittedError> {
        self.require_owner(policy)?;
        let owned_policy = policy.clone();
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Data,
            AllocationPlan::checked(
                "instruction confirmation request",
                &[
                    self.0.identity.sandbox_id.len(),
                    self.0.identity.home_identity.len(),
                    // validate_for constructs one of these fixed diagnostics.
                    "runtime status identity is invalid"
                        .len()
                        .max("runtime status does not prove the requested identity".len()),
                ],
            )?,
            move |scope| async move {
                let policy = &owned_policy;
                let status = self.instruction_status_owned(&scope, policy).await?;
                status.work_scope().expect("admitted status").admit_extra(
                    policy,
                    IngressClass::Data,
                    AllocationPlan::checked("instruction read path", &[self.0.path.len()])?,
                )?;
                let request = status.map(|status| {
                    RequestFrame::ReadAgentFile(crate::AgentFileReadRequest {
                        owner_access: true,
                        protected_paths: Vec::new(),
                        sandbox_id: status.sandbox_id,
                        runtime_generation: status.generation,
                        boot_id: status.boot_id,
                        home_identity: None,
                        path: self.0.path.clone(),
                        max_bytes: MAX_INSTRUCTIONS as u64,
                    })
                });
                let response = self.instruction_exchange_owned(request, policy).await?;
                let checked = scope
                    .initialize_owned(|| async {
                        let ResponseFrame::ReadAgentFile(file) = &*response else {
                            return Err(AdapterError::unexpected_reply(
                                "retained instructions read",
                            ));
                        };
                        if file.path != self.0.path {
                            return Err(instruction_mismatch());
                        }
                        if file.size_bytes > MAX_INSTRUCTIONS as u64
                            || file.content_base64.len() > MAX_INSTRUCTIONS.div_ceil(3) * 4
                        {
                            return Err(AdapterError::Rejected(text(
                                "file response exceeds its size limit",
                            )));
                        }
                        Ok(())
                    })
                    .resolve_admitted_result(policy, IngressClass::Data)
                    .await?;
                drop(checked);
                let ResponseFrame::ReadAgentFile(file) = &*response else {
                    unreachable!("validated file response")
                };
                verify_instructions(file, expected, policy).await
            },
        )
    }

    pub(super) async fn instruction_status_owned(
        &self,
        scope: &IngressWorkScope,
        policy: &IngressOwnershipPolicy,
    ) -> Result<OwnedWork<RuntimeStatus>, AdmittedError> {
        let request =
            scope.initialize_owned(|| RequestFrame::RuntimeStatus(self.0.identity.clone()));
        let response = self.instruction_exchange_owned(request, policy).await?;
        scope
            .initialize_owned(|| async {
                let ResponseFrame::RuntimeStatus(status) = &*response else {
                    return Err(AdapterError::unexpected_reply("managed context runtime"));
                };
                status.validate_for(&self.0.identity)?;
                if !status.agent_filesystem || !status.credential_ready || status.restore_pending {
                    return Err(AdapterError::Unsupported(text(
                        "runtime is not ready for managed context refresh",
                    )));
                }
                Ok(())
            })
            .resolve_admitted_result(policy, IngressClass::Data)
            .await?;
        Ok(response.map(|response| match response {
            ResponseFrame::RuntimeStatus(status) => status,
            _ => unreachable!("validated runtime status"),
        }))
    }

    pub(super) async fn instruction_exchange_owned(
        &self,
        request: OwnedWork<RequestFrame>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<OwnedWork<ResponseFrame>, AdmittedError> {
        let expected = request.operation();
        let operation = self
            .0
            .backend
            .exchange_admitted(&self.0.handle, request, policy)?;
        if !operation.belongs_to(policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "instruction source ownership",
            }
            .into());
        }
        let response = operation.await?;
        if !response.belongs_to(policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "instruction response ownership",
            }
            .into());
        }
        if let ResponseFrame::Failure(failure) = &*response {
            if failure.operation == expected {
                // failure_error formats code + separator + detail. String's
                // geometric growth overlaps old and new allocations, bounded
                // by three times the final length plus its
                // minimum allocation, alongside the still-owned wire fields.
                let plan = AllocationPlan::checked(
                    "instruction wire failure",
                    &[
                        failure.code.len(),
                        failure.code.len(),
                        failure.code.len(),
                        failure.detail.len(),
                        failure.detail.len(),
                        failure.detail.len(),
                        6 + 8,
                    ],
                )?;
                response
                    .work_scope()
                    .expect("admitted response")
                    .admit_extra(policy, IngressClass::Data, plan)?;
                return response
                    .map(|response| async move {
                        let ResponseFrame::Failure(failure) = response else {
                            unreachable!()
                        };
                        Err(super::super::reply::failure_error(failure))
                    })
                    .resolve_admitted_result(policy, IngressClass::Data)
                    .await;
            }
        }
        if super::super::reply::response_operation(&response) != expected {
            return response
                .map(|_| async { Err(AdapterError::unexpected_reply("managed context")) })
                .resolve_admitted_result(policy, IngressClass::Data)
                .await;
        }
        Ok(response)
    }
}

async fn verify_instructions(
    file: &crate::AgentFileReadReceipt,
    expected: &str,
    policy: &IngressOwnershipPolicy,
) -> Result<OwnedWork<()>, AdmittedError> {
    // base64 0.22 reserves three bytes per complete/partial quartet.
    // Digest formatting can overlap successive 32/64-byte buffers;
    // three digest lengths also cover its allocation growth.
    let decode = IngressWorkScope::new(
        policy.admit(
            IngressClass::Transformation,
            AllocationPlan::checked(
                "instruction file verification",
                &[
                    file.content_base64.len().div_ceil(4) * 3,
                    3 * 64,
                    "runtime returned invalid file encoding"
                        .len()
                        .max("runtime returned an invalid file digest".len())
                        .max("file response exceeds its size limit".len()),
                    sizing::shared_owner_bytes::<()>(),
                ],
            )?,
        )?,
    );
    decode
        .initialize_owned(|| async {
            if file.content()? != expected.as_bytes() {
                return Err(instruction_mismatch());
            }
            Ok(())
        })
        .resolve_admitted_result(policy, IngressClass::Transformation)
        .await
}

fn instruction_mismatch() -> AdapterError {
    AdapterError::OperationIdentityConflict(text(
        "retained instructions do not match the pending settings operation",
    ))
}

#[cfg(test)]
#[path = "instructions_admission_tests.rs"]
pub(in crate::hosted) mod tests;
