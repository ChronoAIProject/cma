use std::future::Future;
use std::time::{Duration, SystemTime};

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    SandboxCheckpointAgentGuard, SandboxCheckpointMutation, SandboxMutationGuard, SandboxStore,
    SandboxStoreError,
};
use layer2_core::sandbox::{
    Agent, Sandbox, SandboxCheckpointConversation, SandboxCheckpointRef, SandboxLifecycleOperation,
};
use layer3_adapter::{
    writer_resume_tokens_match, AdapterError, AgentSlot, CheckpointHoldReceipt,
    CheckpointHoldRequest, CheckpointRequest, CheckpointWriterResumeRequest, RuntimeHandle,
    WriterState,
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
        if Self::uses_retained_home(&sandbox) {
            let stamp = rfc3339_at(now);
            let lease = super::support::lifecycle_lease(&sandbox, &self.config.holder)?;
            let mut next = sandbox.clone();
            next.complete_lifecycle_persistent_pause(&lease.holder, lease.fence, stamp.clone())
                .map_err(model_error)?;
            return self.complete_sandbox_only_phase(sandbox, next, stamp).await;
        }
        let started = tokio::time::Instant::now();
        let work = Box::pin(self.capture_and_publish_bounded(sandbox.clone(), now, started));
        self.checkpoint_capture_deadline(&sandbox, now, started, work)
            .await
    }

    pub(crate) async fn checkpoint_capture_deadline(
        &self,
        sandbox: &Sandbox,
        now: SystemTime,
        started: tokio::time::Instant,
        work: impl Future<Output = Result<(), SandboxStoreError>>,
    ) -> Result<(), SandboxStoreError> {
        match tokio::time::timeout(Duration::from_secs(1_800), work).await {
            Ok(result) => result,
            Err(_) => {
                // A timed-out publication may still commit. Fence it with a durable
                // retry while preserving the exact hold token, never release on a read.
                let recovery = self.handle_lifecycle_failure(
                    sandbox.clone(),
                    WorkerFailure::retryable("lifecycle checkpoint operation deadline elapsed"),
                    now + started.elapsed(),
                );
                tokio::time::timeout(Duration::from_secs(30), recovery)
                    .await
                    .map_err(|_| {
                        SandboxStoreError::Unavailable(
                            "lifecycle checkpoint retry persistence is unconfirmed".into(),
                        )
                    })?
            }
        }
    }

    async fn capture_and_publish_bounded(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
        started: tokio::time::Instant,
    ) -> Result<(), SandboxStoreError> {
        let Some(runtime) = runtime(&sandbox) else {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::terminal_security("lifecycle runtime binding is absent"),
                    now + started.elapsed(),
                )
                .await;
        };
        let agents = self.lifecycle_agents(&sandbox).await?;
        for agent in agents.iter().filter(|agent| agent.status.is_live()) {
            if let Err(failure) = require_quiet_agent(agent) {
                return self
                    .handle_lifecycle_failure(sandbox, failure, now + started.elapsed())
                    .await;
            }
        }
        let epoch = sandbox
            .next_lifecycle_checkpoint_epoch()
            .map_err(model_error)?;
        let token = match lifecycle_writer_token(&sandbox) {
            Ok(token) => token,
            Err(failure) => {
                return self
                    .handle_lifecycle_failure(sandbox, failure, now + started.elapsed())
                    .await
            }
        };
        let request = match checkpoint_hold_request(
            self.dependencies.adapter.as_ref(),
            epoch,
            token.clone(),
            &agents,
        ) {
            Ok(request) => request,
            Err(error) => {
                return self
                    .handle_lifecycle_failure(
                        sandbox,
                        adapter_failure(error),
                        now + started.elapsed(),
                    )
                    .await
            }
        };
        if !self.lifecycle_state_matches(&sandbox).await? {
            return Err(SandboxStoreError::Conflict);
        }
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
                        now + started.elapsed(),
                    )
                    .await
            }
            Err(error) => {
                return self
                    .fail_after_writer_resume(
                        sandbox,
                        checkpoint_hold_failure(error),
                        now + started.elapsed(),
                    )
                    .await
            }
        };
        // The home archive and conversation captures must share one writer hold.
        let captures = match self
            .capture_lifecycle_conversations(&runtime, &agents)
            .await
        {
            Ok(captures) => captures,
            Err(failure) => {
                return self
                    .fail_after_writer_resume(sandbox, failure, now + started.elapsed())
                    .await
            }
        };
        let now = now + started.elapsed();
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
            agents: agents
                .iter()
                .filter(|agent| agent.status.is_live())
                .map(SandboxCheckpointAgentGuard::capture)
                .collect(),
            nonblocking: false,
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
        let agents = self
            .lifecycle_agents(sandbox)
            .await
            .map_err(|_| WorkerFailure::retryable("checkpoint agent inventory unavailable"))?;
        let request = checkpoint_hold_request(
            self.dependencies.adapter.as_ref(),
            checkpoint.epoch,
            lifecycle_writer_token(sandbox)?,
            &agents,
        )
        .map_err(adapter_failure)?;
        if !self
            .lifecycle_state_matches(sandbox)
            .await
            .map_err(|_| WorkerFailure::retryable("checkpoint lifecycle state unavailable"))?
        {
            return Err(WorkerFailure::retryable(
                "checkpoint lifecycle authority was superseded",
            ));
        }
        let receipt = self
            .dependencies
            .adapter
            .verify_checkpoint_hold(&runtime, &request)
            .await
            .map_err(checkpoint_hold_failure)?;
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

    /// A retained agent-sandbox home already provides the pause boundary. Its
    /// PVC is the durable source of truth, so no runtime writer hold or CMA
    /// checkpoint object is involved.
    pub(super) fn uses_retained_home(sandbox: &Sandbox) -> bool {
        sandbox.retained_home_recovery().is_some()
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
        // An ambiguous commit can arrive after this read. Leave exact-token debt
        // with the durable lifecycle so its next claim replays or continues the hold.
        Err(error)
    }
}

fn checkpoint_hold_failure(error: AdapterError) -> WorkerFailure {
    // A restart may retain authority that the runtime already resumed and retired.
    if matches!(&error, AdapterError::Rejected(detail)
        if detail.starts_with("checkpoint_hold_retired: ")
            || detail == "CheckpointForTeardown: checkpoint_hold_retired")
    {
        return WorkerFailure::retryable("checkpoint writer authority was already resumed");
    }
    adapter_failure(error)
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

fn checkpoint_hold_request(
    adapter: &dyn layer3_adapter::SandboxAdapter,
    epoch: u64,
    writer_resume_token: String,
    agents: &[Agent],
) -> Result<CheckpointHoldRequest, AdapterError> {
    // Admission fences and retained tombstones keep this complete slot set stable during a hold.
    let slots = agents
        .iter()
        .map(|agent| AgentSlot::new(agent.id.to_string()))
        .collect::<Vec<_>>();
    Ok(CheckpointHoldRequest {
        checkpoint: CheckpointRequest {
            epoch,
            exclusions: adapter.checkpoint_exclusions(&slots)?,
        },
        writer_resume_token,
    })
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
