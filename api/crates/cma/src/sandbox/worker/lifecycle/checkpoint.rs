use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    SandboxCheckpointMutation, SandboxMutationGuard, SandboxStore, SandboxStoreError,
};
use layer2_core::sandbox::{
    Agent, Sandbox, SandboxCheckpointConversation, SandboxCheckpointRef, SandboxImageRotationPhase,
    SandboxLifecycleOperation, SandboxPausePhase,
};
use layer3_adapter::{
    writer_resume_tokens_match, AgentSlot, CheckpointHoldReceipt, CheckpointHoldRequest,
    CheckpointRequest, CheckpointWriterResumeRequest, RuntimeHandle, WriterState,
};

use super::super::support::{adapter_failure, runtime, WorkerFailure};
use super::super::SandboxWorker;
use super::support::model_error;

impl SandboxWorker {
    pub(super) async fn capture_and_publish(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let Some(runtime) = runtime(&sandbox) else {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::terminal_security("lifecycle runtime binding is absent"),
                    now,
                )
                .await;
        };
        let agents = self.lifecycle_agents(&sandbox).await?;
        let captures = match self
            .capture_lifecycle_conversations(&runtime, &agents)
            .await
        {
            Ok(captures) => captures,
            Err(failure) => return self.handle_lifecycle_failure(sandbox, failure, now).await,
        };
        let epoch = sandbox
            .next_lifecycle_checkpoint_epoch()
            .map_err(model_error)?;
        let token = match lifecycle_writer_token(&sandbox) {
            Ok(token) => token,
            Err(failure) => return self.handle_lifecycle_failure(sandbox, failure, now).await,
        };
        let request = checkpoint_hold_request(epoch, token.clone());
        let held = match self
            .dependencies
            .adapter
            .checkpoint_for_teardown(&runtime, &request)
            .await
        {
            Ok(receipt) if valid_held_receipt(&receipt, &request) => receipt,
            Ok(_) => {
                return self
                    .fail_after_writer_resume(
                        sandbox,
                        WorkerFailure::terminal_security(
                            "runtime returned invalid held checkpoint evidence",
                        ),
                        now,
                    )
                    .await
            }
            Err(error) => {
                return self
                    .fail_after_writer_resume(sandbox, adapter_failure(error), now)
                    .await
            }
        };
        let stamp = rfc3339_at(now);
        let checkpoint =
            match SandboxCheckpointRef::seal(&held.checkpoint, &captures, stamp.clone()) {
                Ok(checkpoint) => checkpoint,
                Err(_) => {
                    return self
                        .fail_after_writer_resume(
                            sandbox,
                            WorkerFailure::terminal_security(
                                "captured checkpoint evidence could not be sealed",
                            ),
                            now,
                        )
                        .await
                }
            };
        let lease = super::support::lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.publish_lifecycle_checkpoint(
            checkpoint.clone(),
            &lease.holder,
            lease.fence,
            stamp.clone(),
        )
        .map_err(model_error)?;
        let mutation = SandboxCheckpointMutation {
            guard: SandboxMutationGuard::capture(&sandbox),
            checkpoint,
            sandbox: next.clone(),
            appended_at: stamp,
        };
        match self
            .dependencies
            .store
            .publish_sandbox_checkpoint(mutation)
            .await
        {
            Ok(()) => Ok(()),
            Err(error) => {
                self.reconcile_checkpoint_publication(&sandbox, &next, error)
                    .await
            }
        }
    }

    async fn capture_lifecycle_conversations(
        &self,
        runtime: &RuntimeHandle,
        agents: &[Agent],
    ) -> Result<Vec<SandboxCheckpointConversation>, WorkerFailure> {
        let mut captures = Vec::new();
        for agent in agents.iter().filter(|agent| agent.status.is_live()) {
            require_quiet_agent(agent)?;
            let conversation_id = agent.conversation.conversation_id.as_ref().ok_or_else(|| {
                WorkerFailure::terminal_security("live agent conversation identity is incomplete")
            })?;
            let snapshot = self
                .dependencies
                .adapter
                .capture_agent(runtime, &AgentSlot::new(agent.id.to_string()))
                .await
                .map_err(adapter_failure)?;
            captures.push(
                SandboxCheckpointConversation::new(
                    agent.id.clone(),
                    conversation_id.clone(),
                    snapshot,
                )
                .map_err(|_| {
                    WorkerFailure::terminal_security(
                        "runtime returned invalid conversation checkpoint evidence",
                    )
                })?,
            );
        }
        Ok(captures)
    }

    pub(super) async fn ensure_lifecycle_checkpoint_hold(
        &self,
        sandbox: &Sandbox,
    ) -> Result<(), WorkerFailure> {
        let runtime = runtime(sandbox).ok_or_else(|| {
            WorkerFailure::terminal_security("lifecycle runtime binding is absent")
        })?;
        let checkpoint = pinned_checkpoint(sandbox)?;
        let request = checkpoint_hold_request(checkpoint.epoch, lifecycle_writer_token(sandbox)?);
        let receipt = self
            .dependencies
            .adapter
            .checkpoint_for_teardown(&runtime, &request)
            .await
            .map_err(adapter_failure)?;
        if !valid_held_receipt(&receipt, &request)
            || receipt.checkpoint.object_reference != checkpoint.object_reference
            || receipt.checkpoint.sha256 != checkpoint.sha256
        {
            return Err(WorkerFailure::terminal_security(
                "runtime checkpoint hold does not match the pinned checkpoint",
            ));
        }
        Ok(())
    }

    pub(super) async fn resume_lifecycle_writers(
        &self,
        sandbox: &Sandbox,
    ) -> Result<(), WorkerFailure> {
        let runtime = runtime(sandbox).ok_or_else(|| {
            WorkerFailure::terminal_security("lifecycle runtime binding is absent")
        })?;
        let token = lifecycle_writer_token(sandbox)?;
        let receipt = self
            .dependencies
            .adapter
            .resume_checkpoint_writers(
                &runtime,
                &CheckpointWriterResumeRequest {
                    writer_resume_token: token.clone(),
                },
            )
            .await
            .map_err(adapter_failure)?;
        if !receipt.writers_resumed
            || !writer_resume_tokens_match(&receipt.writer_resume_token, &token)
        {
            return Err(WorkerFailure::terminal_security(
                "runtime did not confirm checkpoint writer resumption",
            ));
        }
        Ok(())
    }

    async fn fail_after_writer_resume(
        &self,
        sandbox: Sandbox,
        failure: WorkerFailure,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if let Err(resume_failure) = self.resume_lifecycle_writers(&sandbox).await {
            return self
                .handle_lifecycle_failure(sandbox, resume_failure, now)
                .await;
        }
        self.handle_lifecycle_failure(sandbox, failure, now).await
    }

    async fn reconcile_checkpoint_publication(
        &self,
        current: &Sandbox,
        expected: &Sandbox,
        error: SandboxStoreError,
    ) -> Result<(), SandboxStoreError> {
        let stored =
            SandboxStore::get_sandbox(self.dependencies.store.as_ref(), &current.id).await?;
        if stored.as_ref() == Some(expected) {
            return Ok(());
        }
        if stored
            .as_ref()
            .is_some_and(|sandbox| lifecycle_keeps_writers_held(sandbox, current))
        {
            return Err(error);
        }
        if stored
            .as_ref()
            .is_some_and(|sandbox| sandbox.runtime == current.runtime)
        {
            if let Err(resume_error) = self.resume_lifecycle_writers(current).await {
                tracing::warn!(sandbox_id = %current.id, error = ?resume_error, "checkpoint publication failed and writer resumption could not be confirmed");
            }
        }
        Err(error)
    }
}

pub(super) fn require_quiet_agent(agent: &Agent) -> Result<(), WorkerFailure> {
    if agent.conversation.active_turn.is_some()
        || agent
            .conversation
            .operation
            .as_ref()
            .is_some_and(|operation| operation.blocks_dispatch())
        || agent.work.step.is_pending()
        || agent.work.lease.is_some()
        || agent.requested_stop_reason.is_some()
    {
        return Err(WorkerFailure::retryable(
            "a live agent is not quiet enough for sandbox lifecycle work",
        ));
    }
    Ok(())
}

pub(super) fn pinned_checkpoint(sandbox: &Sandbox) -> Result<&SandboxCheckpointRef, WorkerFailure> {
    let expected_epoch = sandbox
        .lifecycle
        .operation
        .as_ref()
        .and_then(SandboxLifecycleOperation::checkpoint_epoch)
        .ok_or_else(|| WorkerFailure::terminal_security("lifecycle checkpoint epoch is absent"))?;
    let checkpoint = sandbox
        .lifecycle
        .checkpoints
        .latest
        .as_ref()
        .filter(|checkpoint| checkpoint.epoch == expected_epoch)
        .ok_or_else(|| {
            WorkerFailure::terminal_security("lifecycle checkpoint is not the pinned latest epoch")
        })?;
    checkpoint.validate().map_err(|_| {
        WorkerFailure::terminal_security("lifecycle checkpoint metadata is invalid")
    })?;
    Ok(checkpoint)
}

fn checkpoint_hold_request(epoch: u64, writer_resume_token: String) -> CheckpointHoldRequest {
    CheckpointHoldRequest {
        checkpoint: CheckpointRequest {
            epoch,
            exclusions: Vec::new(),
        },
        writer_resume_token,
    }
}

fn lifecycle_writer_token(sandbox: &Sandbox) -> Result<String, WorkerFailure> {
    sandbox
        .lifecycle
        .operation
        .as_ref()
        .and_then(|operation| operation.authority().writer_resume_token.clone())
        .ok_or_else(|| WorkerFailure::terminal_security("lifecycle writer resume token is absent"))
}

fn valid_held_receipt(receipt: &CheckpointHoldReceipt, request: &CheckpointHoldRequest) -> bool {
    receipt.checkpoint.epoch == request.checkpoint.epoch
        && receipt.checkpoint.writers == WriterState::Held
        && writer_resume_tokens_match(&receipt.writer_resume_token, &request.writer_resume_token)
}

fn lifecycle_keeps_writers_held(stored: &Sandbox, attempted: &Sandbox) -> bool {
    let same_operation = stored
        .lifecycle
        .operation
        .as_ref()
        .zip(attempted.lifecycle.operation.as_ref())
        .is_some_and(|(stored, attempted)| {
            stored.authority().operation_id == attempted.authority().operation_id
        });
    same_operation
        && matches!(
            stored.lifecycle.operation,
            Some(SandboxLifecycleOperation::Pause {
                phase: SandboxPausePhase::PauseAgents | SandboxPausePhase::TeardownRuntime,
                ..
            }) | Some(SandboxLifecycleOperation::ImageRotation {
                phase: SandboxImageRotationPhase::PauseAgents
                    | SandboxImageRotationPhase::TeardownRuntime,
                ..
            })
        )
}
