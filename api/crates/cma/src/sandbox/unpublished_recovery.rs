//! Composition of model admission with exact retained-home/provider evidence.
use layer3_adapter::recovery_diagnostics::{Observation, Reason, Stage};
use std::sync::Arc;

#[path = "unpublished_recovery_source.rs"]
mod source;
pub(crate) use source::verify_source;

use layer2_core::sandbox::{
    SandboxRecoveryConversationProof, SandboxStoreError, SandboxUnpublishedRecoveryProof,
    SandboxUnpublishedRecoverySnapshot, SandboxUnpublishedRecoveryVerifier,
};
use layer3_adapter::{
    AdapterError, RecoveryConversation, RecoveryConversationHistory,
    RuntimeRecoveryInspectionRequest, RuntimeStatusRequest, SandboxAdapter,
};

pub(crate) struct UnpublishedRuntimeRecovery {
    adapter: Arc<dyn SandboxAdapter>,
    deployment: String,
}

impl UnpublishedRuntimeRecovery {
    pub(crate) fn new(adapter: Arc<dyn SandboxAdapter>, deployment: String) -> Self {
        Self {
            adapter,
            deployment,
        }
    }
}

#[async_trait::async_trait]
impl SandboxUnpublishedRecoveryVerifier for UnpublishedRuntimeRecovery {
    async fn verify(
        &self,
        snapshot: &SandboxUnpublishedRecoverySnapshot,
    ) -> Result<SandboxUnpublishedRecoveryProof, SandboxStoreError> {
        let mut observation = Observation::new(Stage::Proof);
        let result = self.verify_snapshot(snapshot).await;
        observation.finish(match &result {
            Ok(_) => Reason::Success,
            Err(SandboxStoreError::Unavailable(_)) => Reason::Unavailable,
            Err(_) => Reason::Ineligible,
        });
        result
    }
}

impl UnpublishedRuntimeRecovery {
    async fn verify_snapshot(
        &self,
        snapshot: &SandboxUnpublishedRecoverySnapshot,
    ) -> Result<SandboxUnpublishedRecoveryProof, SandboxStoreError> {
        snapshot.validate()?;
        if snapshot.sandbox.legacy_unpublished_allocation() {
            // Avoid overlap with ordinary bounded assignment calls. This age is
            // not a cancellation receipt: remote assignment can survive a lost
            // exec response, so fresh process/identity proof remains mandatory.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| refused())?
                .as_secs();
            let failed_at = layer2_core::seconds_from_rfc3339(&snapshot.sandbox.updated_at)
                .ok_or_else(refused)?;
            if now.saturating_sub(failed_at) < 60 {
                return Err(refused());
            }
        }
        let binding = snapshot
            .sandbox
            .unpublished_recovery_binding()
            .ok_or_else(refused)?;
        let generation = snapshot.runtime_generation()?;
        let home = snapshot.home_identity()?;
        if snapshot.agents.is_empty() {
            let source = super::runtime_attachment::retained_runtime_request(
                &snapshot.sandbox,
                binding,
                &self.deployment,
            )
            .map_err(store_failure)?;
            let recovery = self.adapter.runtime_recovery().ok_or_else(refused)?;
            let (evidence, reserved_provision, observed_provision_generation) =
                match recovery.inspect_nonexecuting_recovery(&source).await {
                    Ok(evidence) => (evidence, None, None),
                    Err(error) => {
                        let Some(reserved) =
                            super::runtime_attachment::reserved_recovery_runtime_request(
                                snapshot,
                                &self.deployment,
                            )
                            .map_err(store_failure)?
                        else {
                            return Err(store_failure(error));
                        };
                        (
                            recovery
                                .inspect_nonexecuting_recovery(&reserved)
                                .await
                                .map_err(store_failure)?,
                            snapshot.recovery_provision_reservation()?.cloned(),
                            snapshot
                                .sandbox
                                .legacy_unpublished_allocation()
                                .then_some(reserved.generation),
                        )
                    }
                };
            if let Some(evidence) = evidence {
                if !evidence.runtime_process_absent || !evidence.histories.is_empty() {
                    return Err(refused());
                }
                let proof = SandboxUnpublishedRecoveryProof {
                    sandbox_id: snapshot.sandbox.id.clone(),
                    runtime_id: binding.runtime_id.clone(),
                    runtime_generation: generation,
                    reserved_provision,
                    observed_provision_generation,
                    home_identity: home,
                    boot_id: None,
                    runtime_process_absent: true,
                    requires_native_quiescence: true,
                    provider_plan: evidence.provider_plan,
                    provider_plan_sha256: evidence.provider_plan_sha256,
                    conversations: vec![],
                };
                proof.validate_for(snapshot)?;
                return Ok(proof);
            }
        }
        let boot = binding.boot_id.clone().ok_or_else(refused)?;
        let conversations = snapshot
            .agents
            .iter()
            .filter(|agent| agent.status.is_live())
            .map(|agent| {
                Ok(RecoveryConversation {
                    agent_id: agent.id.to_string(),
                    conversation_id: agent
                        .conversation
                        .conversation_id
                        .clone()
                        .ok_or_else(refused)?,
                })
            })
            .collect::<Result<Vec<_>, SandboxStoreError>>()?;
        let source = RuntimeRecoveryInspectionRequest {
            runtime: layer3_adapter::RuntimeHandle::new(&binding.runtime_id),
            identity: RuntimeStatusRequest {
                sandbox_id: snapshot.sandbox.id.to_string(),
                generation,
                home_identity: home.clone(),
            },
            boot_id: boot.clone(),
            conversations,
        };
        verify_source(
            self.adapter.as_ref(),
            &snapshot.sandbox,
            &source,
            &self.deployment,
        )
        .await
        .map_err(store_failure)?;
        let evidence = self
            .adapter
            .runtime_recovery()
            .ok_or_else(refused)?
            .inspect_recovery(&source)
            .await
            .map_err(store_failure)?;
        source::verify_source_at(
            self.adapter.as_ref(),
            &snapshot.sandbox,
            &source,
            &self.deployment,
            Stage::NativeAfter,
        )
        .await
        .map_err(store_failure)?;
        let proof = SandboxUnpublishedRecoveryProof {
            sandbox_id: snapshot.sandbox.id.clone(),
            runtime_id: binding.runtime_id.clone(),
            runtime_generation: generation,
            reserved_provision: None,
            observed_provision_generation: None,
            home_identity: home,
            boot_id: Some(boot),
            runtime_process_absent: evidence.runtime_process_absent,
            requires_native_quiescence: true,
            provider_plan: evidence.provider_plan,
            provider_plan_sha256: evidence.provider_plan_sha256,
            conversations: histories(evidence.histories)?,
        };
        proof.validate_for(snapshot)?;
        Ok(proof)
    }
}

pub(crate) fn histories(
    histories: Vec<RecoveryConversationHistory>,
) -> Result<Vec<SandboxRecoveryConversationProof>, SandboxStoreError> {
    histories
        .into_iter()
        .map(|history| {
            Ok(SandboxRecoveryConversationProof {
                agent_id: layer2_core::AgentId(history.agent_id),
                conversation_id: history.conversation_id,
                history_sha256: history.history_sha256,
            })
        })
        .collect()
}

fn refused() -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction("original recovery authority is incomplete")
}

fn store_failure(error: AdapterError) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
