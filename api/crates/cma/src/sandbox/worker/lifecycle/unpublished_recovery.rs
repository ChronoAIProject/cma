use std::time::{Instant, SystemTime};

use layer2_core::{
    rfc3339_at,
    sandbox::{
        store::SandboxLifecycleAgentScope,
        work_retry::{WorkRetryFailure, WORK_RETRY_BUDGET, WORK_RETRY_EXTERNAL_BUDGET},
        Sandbox, SandboxEventKind, SandboxStoreError, SandboxUnpublishedRecoveryQuiescence,
        SandboxWorkStep,
    },
};
use layer3_adapter::{
    AdapterError, RecoveryConversation, RuntimeHandle, RuntimeRecoveryInspectionRequest,
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
            Err(error) => {
                if error == SandboxStoreError::Conflict {
                    return Err(error);
                }
                // The ordinary parked-update transition preserves the admitted
                // recovery plan, source binding, and any unverified history.
                let failure = quiescence_failure(&error, sandbox.work.attempts);
                return self
                    .handle_lifecycle_failure(sandbox, failure, completed)
                    .await;
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
                .map_err(|error| observed_recovery_failure(sandbox, error))?;
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
        .map_err(|error| observed_recovery_failure(sandbox, error))?;
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
            .map_err(|error| observed_recovery_failure(sandbox, error))?;
        crate::sandbox::unpublished_recovery::verify_source(
            self.dependencies.adapter.as_ref(),
            sandbox,
            &source,
            &self.config.deployment_id,
        )
        .await
        .map_err(|error| observed_recovery_failure(sandbox, error))?;
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

/// The mapped store error is deliberately coarse, so a stalled recovery would
/// otherwise retry with no visible cause. Keep the provider's reason observable.
fn observed_recovery_failure(sandbox: &Sandbox, error: AdapterError) -> SandboxStoreError {
    let detail: String = error
        .to_string()
        .chars()
        .filter(|character| !character.is_control())
        .take(300)
        .collect();
    tracing::warn!(
        sandbox_id = %sandbox.id,
        detail,
        "original runtime recovery step failed; all data retained"
    );
    recovery_failure(error)
}

fn quiescence_failure(error: &SandboxStoreError, attempts: u32) -> WorkerFailure {
    let (kind, budget) = if matches!(error, SandboxStoreError::Corrupt(_)) {
        (WorkRetryFailure::ExternalAction, WORK_RETRY_EXTERNAL_BUDGET)
    } else {
        (WorkRetryFailure::Transient, WORK_RETRY_BUDGET)
    };
    if attempts >= budget {
        return WorkerFailure::terminal_validation(
            "original conversation persistence could not be verified; original runtime and home retained",
        );
    }
    WorkerFailure::retryable(
        "original conversation persistence is not yet verified; recovery will retry with all data retained",
    )
    .with_retry_kind(kind)
}

// A refused original-history or identity proof uses the smaller retry budget.
fn recovery_failure(error: AdapterError) -> SandboxStoreError {
    match error {
        AdapterError::NativeClaimNotReady(_)
        | AdapterError::RuntimeStartupHold(_)
        | AdapterError::Unavailable(_)
        | AdapterError::RuntimeCapacity(_)
        | AdapterError::AtCapacity(_)
        | AdapterError::InputAuthorityUnavailable(_)
        | AdapterError::PromptNotWritten(_)
        | AdapterError::RuntimeLedgerHold(_) => unavailable(),
        _ => SandboxStoreError::Corrupt(
            "original runtime recovery evidence requires external action".into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_retry_original_history_refusal_keeps_external_and_transient_failures_distinct() {
        assert!(matches!(
            recovery_failure(AdapterError::Rejected("original history refused".into())),
            SandboxStoreError::Corrupt(_)
        ));
        for error in [
            AdapterError::Unavailable("network".into()),
            AdapterError::RuntimeLedgerHold("hold".into()),
        ] {
            assert!(matches!(
                recovery_failure(error),
                SandboxStoreError::Unavailable(_)
            ));
        }
    }

    #[test]
    fn original_quiescence_retry_budget_ends_in_a_retained_hold() {
        for (error, budget, kind) in [
            (
                recovery_failure(AdapterError::Rejected("fixture refusal".into())),
                WORK_RETRY_EXTERNAL_BUDGET,
                WorkRetryFailure::ExternalAction,
            ),
            (
                recovery_failure(AdapterError::Unavailable("fixture timeout".into())),
                WORK_RETRY_BUDGET,
                WorkRetryFailure::Transient,
            ),
        ] {
            let retry = quiescence_failure(&error, budget - 1);
            assert!(retry.is_retryable());
            assert_eq!(retry.retry_kind(), kind);
            for attempts in [budget, u32::MAX] {
                let hold = quiescence_failure(&error, attempts);
                assert!(!hold.is_retryable());
                assert_eq!(hold.typed().class, "terminal_validation");
            }
        }
    }
}
