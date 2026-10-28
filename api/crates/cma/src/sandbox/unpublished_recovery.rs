//! Composition of model admission with exact retained-home/provider evidence.
use std::sync::Arc;

use layer2_core::sandbox::{
    Sandbox, SandboxRecoveryConversationProof, SandboxStoreError, SandboxUnpublishedRecoveryProof,
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
        snapshot.validate()?;
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
            let (evidence, reserved_provision) =
                match recovery.inspect_nonexecuting_recovery(&source).await {
                    Ok(evidence) => (evidence, None),
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
        verify_source(
            self.adapter.as_ref(),
            &snapshot.sandbox,
            &source,
            &self.deployment,
        )
        .await
        .map_err(store_failure)?;
        let proof = SandboxUnpublishedRecoveryProof {
            sandbox_id: snapshot.sandbox.id.clone(),
            runtime_id: binding.runtime_id.clone(),
            runtime_generation: generation,
            reserved_provision: None,
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

pub(crate) async fn verify_source(
    adapter: &dyn SandboxAdapter,
    sandbox: &Sandbox,
    source: &RuntimeRecoveryInspectionRequest,
    deployment: &str,
) -> layer3_adapter::Result<()> {
    let binding = sandbox
        .runtime
        .as_ref()
        .or(sandbox.lifecycle.suspended_runtime.as_deref())
        .ok_or_else(invalid)?;
    if binding.runtime_id != source.runtime.as_str()
        || binding.provision_generation != Some(source.identity.generation)
        || binding.boot_id.as_deref() != Some(source.boot_id.as_str())
        || sandbox.id.as_str() != source.identity.sandbox_id
    {
        return Err(invalid());
    }
    let attachment = sandbox
        .lifecycle
        .home
        .as_ref()
        .filter(|home| !home.released)
        .and_then(|home| home.attachment())
        .ok_or_else(invalid)?;
    if attachment.identity()? != source.identity.home_identity {
        return Err(invalid());
    }
    let volumes = adapter.home_volumes().ok_or_else(invalid)?;
    volumes.verify(&attachment).await?;
    let request =
        super::runtime_attachment::retained_runtime_request(sandbox, binding, deployment)?;
    let inspection = adapter
        .inspect_runtime(&request)
        .await?
        .ok_or_else(invalid)?;
    if inspection.observation.handle != source.runtime
        || inspection.observation.sandbox_id != source.identity.sandbox_id
        || inspection.observation.generation != source.identity.generation
        || !inspection.observation.lifecycle.is_running()
    {
        return Err(invalid());
    }
    let status = inspection.status.ok_or_else(invalid)?;
    status.validate_for(&source.identity)?;
    if status.boot_id != source.boot_id || status.restore_pending {
        return Err(invalid());
    }
    volumes.verify(&attachment).await
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

fn invalid() -> AdapterError {
    AdapterError::Rejected("retained recovery runtime or home authority changed".into())
}

fn refused() -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction("original recovery authority is incomplete")
}

fn store_failure(_: AdapterError) -> SandboxStoreError {
    SandboxStoreError::Unavailable("original runtime recovery evidence is unavailable".into())
}
