use super::instructions_admission::MAX_INSTRUCTIONS;
use super::*;
use crate::ingress_budget::{
    diagnostic::OperationError, sizing, AdmittedError, AdmittedOperation, AllocationPlan,
    IngressClass, IngressOwnershipPolicy, IngressWorkScope, OwnedWork,
};

struct PreparedGuidance {
    request: crate::AgentFileWriteRequest,
    sha256: String,
    bytes: u64,
}

impl HostedContextRefresh {
    pub(in crate::hosted) fn append_guidance_admitted<'a>(
        &'a self,
        session: &'a str,
        sequence: u64,
        readiness_receipt: &'a str,
        guidance: &'a str,
        previous: &'a [String],
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, ()>, AdmittedError> {
        self.require_owner(policy)?;
        let owned_policy = policy.clone();
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Data,
            AllocationPlan::checked(
                "managed guidance identity",
                &[
                    self.0.identity.sandbox_id.len(),
                    self.0.identity.home_identity.len(),
                    64, // receipt() creates one fixed hexadecimal String.
                    "runtime status identity is invalid"
                        .len()
                        .max("runtime status does not prove the requested identity".len()),
                ],
            )?,
            move |scope| async move {
                let policy = &owned_policy;
                scope
                    .initialize_owned(|| async {
                        if guidance.is_empty()
                            || guidance.len() > 16 * 1024
                            || guidance.contains('\0')
                        {
                            return Err(AdapterError::Unsupported(text(
                                "managed guidance is invalid",
                            ))
                            .into());
                        }
                        let status = self.instruction_status_owned(&scope, policy).await?;
                        if sequence == 0
                            || self.receipt(&status, session, sequence) != readiness_receipt
                        {
                            return Err(AdapterError::OperationIdentityConflict(text(
                                "managed context readiness belongs to another runtime identity",
                            ))
                            .into());
                        }
                        scope
                            .admit_extra(
                                policy,
                                IngressClass::Data,
                                AllocationPlan::checked(
                                    "managed guidance read request",
                                    &[
                                        status.sandbox_id.len(),
                                        status.boot_id.len(),
                                        self.0.path.len(),
                                    ],
                                )
                                .map_err(AdmittedError::from)?,
                            )
                            .map_err(AdmittedError::from)?;
                        let request = scope.initialize_owned(|| {
                            RequestFrame::ReadAgentFile(crate::AgentFileReadRequest {
                                owner_access: true,
                                protected_paths: Vec::new(),
                                sandbox_id: status.sandbox_id.clone(),
                                runtime_generation: status.generation,
                                boot_id: status.boot_id.clone(),
                                home_identity: None,
                                path: self.0.path.clone(),
                                max_bytes: MAX_INSTRUCTIONS as u64,
                            })
                        });
                        let response = self.instruction_exchange_owned(request, policy).await?;
                        let ResponseFrame::ReadAgentFile(file) = &*response else {
                            return Err(AdapterError::unexpected_reply(
                                "managed instructions read",
                            )
                            .into());
                        };
                        if file.path != self.0.path {
                            return Err(AdapterError::unexpected_reply(
                                "managed instructions path",
                            )
                            .into());
                        }
                        if file.size_bytes > MAX_INSTRUCTIONS as u64
                            || file.content_base64.len() > MAX_INSTRUCTIONS.div_ceil(3) * 4
                        {
                            return Err(AdapterError::Rejected(text(
                                "file response exceeds its size limit",
                            ))
                            .into());
                        }
                        let prepared = self
                            .prepare_guidance(&status, file, guidance, previous, policy)
                            .await?;
                        let prepared = retain_guidance(prepared, policy)?;
                        drop((response, status));
                        self.write_guidance(prepared, policy).await?;
                        Ok::<(), OperationError>(())
                    })
                    .resolve_source_result(policy, IngressClass::Data)
                    .await
            },
        )
    }

    async fn prepare_guidance(
        &self,
        status: &RuntimeStatus,
        file: &crate::AgentFileReadReceipt,
        guidance: &str,
        previous: &[String],
        policy: &IngressOwnershipPolicy,
    ) -> Result<OwnedWork<Option<PreparedGuidance>>, AdmittedError> {
        // content() keeps its base64 output while formatting the digest.
        // Prefix replacement allocates exactly its checked length. Encoding
        // then overlaps original bytes, updated bytes and the new request.
        let scope = IngressWorkScope::new(
            policy.admit(
                IngressClass::Transformation,
                AllocationPlan::checked(
                    "managed guidance transformation",
                    &[
                        file.content_base64.len().div_ceil(4) * 3,
                        MAX_INSTRUCTIONS,
                        MAX_INSTRUCTIONS.div_ceil(3) * 4,
                        3 * 64, // content() digest formatting growth.
                        3 * 64, // updated digest formatting growth.
                        64,     // expected digest copied into the independently owned request.
                        status.sandbox_id.len(),
                        status.boot_id.len(),
                        self.0.path.len(),
                        "runtime returned invalid file encoding"
                            .len()
                            .max("runtime returned an invalid file digest".len())
                            .max("file response exceeds its size limit".len()),
                        sizing::shared_owner_bytes::<PreparedGuidance>(),
                    ],
                )?,
            )?,
        );
        let prepared = scope
            .initialize_owned(|| async {
                let original = String::from_utf8(file.content()?).map_err(|_| {
                    AdapterError::Rejected(text("Agent instructions are not UTF-8"))
                })?;
                let Some(updated) = replace_managed_prefix(&original, guidance, previous)? else {
                    return Ok(None);
                };
                let sha256 = format!("{:x}", Sha256::digest(updated.as_bytes()));
                Ok(Some(PreparedGuidance {
                    bytes: updated.len() as u64,
                    sha256,
                    request: crate::AgentFileWriteRequest {
                        owner_access: true,
                        protected_paths: Vec::new(),
                        sandbox_id: status.sandbox_id.clone(),
                        runtime_generation: status.generation,
                        boot_id: status.boot_id.clone(),
                        path: self.0.path.clone(),
                        expected_sha256: Some(file.sha256.clone()),
                        content_base64: base64::engine::general_purpose::STANDARD
                            .encode(updated.as_bytes()),
                        directory: false,
                    },
                }))
            })
            .resolve_admitted_result(policy, IngressClass::Transformation)
            .await?;
        Ok(prepared)
    }

    async fn write_guidance(
        &self,
        prepared: OwnedWork<Option<PreparedGuidance>>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<OwnedWork<()>, AdmittedError> {
        if prepared.is_none() {
            return Ok(prepared.map(|_| ()));
        }
        let scope = prepared.work_scope().expect("admitted guidance");
        prepared
            .map(|prepared| async move {
                let Some(prepared) = prepared else {
                    return Ok(());
                };
                let request =
                    scope.initialize_owned(|| RequestFrame::WriteAgentFile(prepared.request));
                let response = self.instruction_exchange_owned(request, policy).await?;
                let ResponseFrame::WriteAgentFile(receipt) = &*response else {
                    return Err(AdapterError::unexpected_reply("managed instructions write").into());
                };
                if receipt.path != self.0.path
                    || receipt.sha256 != prepared.sha256
                    || receipt.size_bytes != prepared.bytes
                {
                    return Err(AdapterError::Rejected(text(
                        "runtime did not confirm managed instructions",
                    ))
                    .into());
                }
                Ok::<(), OperationError>(())
            })
            .resolve_source_result(policy, IngressClass::Data)
            .await
    }
}

fn retain_guidance(
    prepared: OwnedWork<Option<PreparedGuidance>>,
    policy: &IngressOwnershipPolicy,
) -> Result<OwnedWork<Option<PreparedGuidance>>, AdmittedError> {
    let Some(value) = &*prepared else {
        return Ok(prepared);
    };
    let request = &value.request;
    let retained = policy.admit(
        IngressClass::Data,
        AllocationPlan::checked(
            "managed guidance retained write",
            &[
                request.sandbox_id.capacity(),
                request.boot_id.capacity(),
                request.path.capacity(),
                request.expected_sha256.as_ref().map_or(0, String::capacity),
                request.content_base64.capacity(),
                value.sha256.capacity(),
                sizing::shared_owner_bytes::<PreparedGuidance>(),
            ],
        )?,
    )?;
    // No write starts while transformation capacity owns the request.
    // Refusal drops the attempted replacement with the old scope intact.
    Ok(prepared.retain_admitted(retained)?)
}

#[cfg(test)]
#[path = "guidance_source_fixture.rs"]
pub(in crate::hosted) mod fixture;

#[cfg(test)]
#[path = "guidance_admission_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "guidance_allocation_tests.rs"]
mod allocation_tests;
