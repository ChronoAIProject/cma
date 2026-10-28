use std::time::{Instant, SystemTime};

use layer2_core::{
    rfc3339_at,
    sandbox::{
        store::SandboxLifecycleAgentScope, Sandbox, SandboxEventKind, SandboxStoreError,
        SandboxUnpublishedRecoveryQuiescence, SandboxWorkStep,
    },
};
use layer3_adapter::{
    RecoveryConversation, RuntimeHandle, RuntimeRecoveryInspectionRequest,
    RuntimeRecoveryQuiescenceRequest, RuntimeStatusRequest,
};

use super::super::{support::WorkerFailure, SandboxWorker};
use super::support::{lifecycle_lease, model_error};

impl SandboxWorker {
    /// This phase records the provider's durable receipt before the existing
    /// retained-home lifecycle is allowed to suspend or replace any runtime.
    pub(super) async fn quiesce_unpublished_runtime(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = Instant::now();
        let result = self.quiescence_receipt(&sandbox, now).await;
        let completed = now + started.elapsed();
        let receipt = match result {
            Ok(mut receipt) => {
                receipt.completed_at = rfc3339_at(completed);
                receipt
            }
            Err(_) => {
                // Keep the admitted plan and its original runtime. A failed
                // drain must never become permission to force termination.
                return self.handle_lifecycle_failure(sandbox, WorkerFailure::retryable(
                    "original conversation persistence is not yet verified; recovery will retry with all data retained",
                ), completed).await;
            }
        };
        if !self.lifecycle_state_matches(&sandbox).await? {
            return Err(SandboxStoreError::Conflict);
        }
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let stamp = rfc3339_at(completed);
        let mut next = sandbox.clone();
        let event = SandboxEventKind::RuntimeRecoveryQuiesced {
            receipt: Box::new(receipt.clone()),
        };
        next.complete_unpublished_runtime_quiescence(
            receipt,
            &lease.holder,
            lease.fence,
            stamp.clone(),
        )
        .map_err(model_error)?;
        self.commit_lifecycle_phase(
            &sandbox,
            next,
            vec![],
            SandboxLifecycleAgentScope::Touched,
            stamp,
            vec![
                event,
                SandboxEventKind::WorkStepCompleted {
                    step: SandboxWorkStep::RunLifecycleOperation,
                },
            ],
        )
        .await
    }

    async fn quiescence_receipt(
        &self,
        sandbox: &Sandbox,
        now: SystemTime,
    ) -> Result<SandboxUnpublishedRecoveryQuiescence, SandboxStoreError> {
        let state = sandbox
            .lifecycle
            .unpublished_recovery
            .as_ref()
            .ok_or_else(unavailable)?;
        state.validate_for(sandbox).map_err(model_error)?;
        if state.quiescence.is_some() || !self.lifecycle_state_matches(sandbox).await? {
            return Err(SandboxStoreError::Conflict);
        }
        let proof = &state.proof;
        if proof.boot_id.is_none() {
            let request = self.nonexecuting_recovery_request(sandbox).await?;
            self.dependencies
                .adapter
                .runtime_recovery()
                .ok_or_else(unavailable)?
                .verify_nonexecuting_recovery(&request)
                .await
                .map_err(|_| unavailable())?;
            return Ok(SandboxUnpublishedRecoveryQuiescence {
                operation_id: state.operation_id.clone(),
                runtime_id: proof.runtime_id.clone(),
                runtime_generation: proof.runtime_generation,
                home_identity: proof.home_identity.clone(),
                boot_id: None,
                provider_plan_sha256: proof.provider_plan_sha256.clone(),
                runtime_process_absent: true,
                conversations: vec![],
                completed_at: rfc3339_at(now),
            });
        }
        let source = RuntimeRecoveryInspectionRequest {
            runtime: RuntimeHandle::new(&proof.runtime_id),
            identity: RuntimeStatusRequest {
                sandbox_id: sandbox.id.to_string(),
                generation: proof.runtime_generation,
                home_identity: proof.home_identity.clone(),
            },
            boot_id: proof.boot_id.clone().ok_or_else(unavailable)?,
            conversations: proof
                .conversations
                .iter()
                .map(|item| RecoveryConversation {
                    agent_id: item.agent_id.to_string(),
                    conversation_id: item.conversation_id.clone(),
                })
                .collect(),
        };
        crate::sandbox::unpublished_recovery::verify_source(
            self.dependencies.adapter.as_ref(),
            sandbox,
            &source,
            &self.config.deployment_id,
        )
        .await
        .map_err(|_| unavailable())?;
        if !self.lifecycle_state_matches(sandbox).await? {
            return Err(SandboxStoreError::Conflict);
        }
        let evidence = self
            .dependencies
            .adapter
            .runtime_recovery()
            .ok_or_else(unavailable)?
            .quiesce_recovery(&RuntimeRecoveryQuiescenceRequest {
                source: source.clone(),
                operation_id: state.operation_id.clone(),
                provider_plan: proof.provider_plan.clone(),
                provider_plan_sha256: proof.provider_plan_sha256.clone(),
            })
            .await
            .map_err(|_| unavailable())?;
        crate::sandbox::unpublished_recovery::verify_source(
            self.dependencies.adapter.as_ref(),
            sandbox,
            &source,
            &self.config.deployment_id,
        )
        .await
        .map_err(|_| unavailable())?;
        Ok(SandboxUnpublishedRecoveryQuiescence {
            operation_id: state.operation_id.clone(),
            runtime_id: proof.runtime_id.clone(),
            runtime_generation: proof.runtime_generation,
            home_identity: proof.home_identity.clone(),
            boot_id: proof.boot_id.clone(),
            provider_plan_sha256: proof.provider_plan_sha256.clone(),
            runtime_process_absent: true,
            conversations: crate::sandbox::unpublished_recovery::histories(evidence.histories)?,
            completed_at: rfc3339_at(now),
        })
    }

    pub(super) async fn nonexecuting_recovery_request(
        &self,
        sandbox: &Sandbox,
    ) -> Result<layer3_adapter::NonexecutingRecoveryVerification, SandboxStoreError> {
        let state = sandbox
            .lifecycle
            .unpublished_recovery
            .as_ref()
            .ok_or_else(unavailable)?;
        state.validate_for(sandbox).map_err(model_error)?;
        let proof = &state.proof;
        if proof.boot_id.is_some()
            || !proof.runtime_process_absent
            || !proof.conversations.is_empty()
            || !self.lifecycle_agents(sandbox).await?.is_empty()
            || !self.lifecycle_state_matches(sandbox).await?
        {
            return Err(unavailable());
        }
        let binding = sandbox
            .lifecycle
            .suspended_runtime
            .as_deref()
            .ok_or_else(unavailable)?;
        let source = crate::sandbox::runtime_attachment::recovery_runtime_request(
            sandbox,
            binding,
            &self.config.deployment_id,
        )
        .map_err(|_| unavailable())?;
        Ok(layer3_adapter::NonexecutingRecoveryVerification {
            source,
            provider_plan: proof.provider_plan.clone(),
            provider_plan_sha256: proof.provider_plan_sha256.clone(),
        })
    }
}

fn unavailable() -> SandboxStoreError {
    SandboxStoreError::Unavailable("original runtime quiescence is incomplete".into())
}
