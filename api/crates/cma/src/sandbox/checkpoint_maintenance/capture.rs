use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    SandboxCheckpointAgentGuard, SandboxCheckpointMutation, SandboxMutationGuard, SandboxStoreError,
};
use layer2_core::sandbox::{Sandbox, SandboxCheckpointConversation, SandboxCheckpointRef};
use layer3_adapter::{
    writer_resume_tokens_match, AgentSlot, CheckpointHoldRequest, CheckpointRequest, WriterState,
};

use super::{adopt_bound_runtime, model_error, SandboxCheckpointMaintenance};

impl SandboxCheckpointMaintenance {
    pub(super) async fn capture_and_publish(
        &self,
        claimed: &Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let result = self.capture(claimed, now).await;
        match result {
            Ok((checkpoint, guards)) => {
                let capture = claimed
                    .lifecycle
                    .checkpoint_capture
                    .as_ref()
                    .ok_or(SandboxStoreError::Conflict)?;
                let lease = capture
                    .work
                    .lease
                    .as_ref()
                    .ok_or(SandboxStoreError::Conflict)?;
                let stamp = rfc3339_at(now);
                let mut next = claimed.clone();
                next.publish_captured_checkpoint(
                    checkpoint.clone(),
                    &lease.holder,
                    lease.fence,
                    stamp.clone(),
                )
                .map_err(model_error)?;
                let result = self
                    .store
                    .publish_sandbox_checkpoint(SandboxCheckpointMutation {
                        guard: SandboxMutationGuard::capture(claimed),
                        agents: guards,
                        checkpoint,
                        sandbox: next.clone(),
                        appended_at: stamp,
                    })
                    .await;
                if result.is_ok()
                    || self.store.get_sandbox(&claimed.id).await?.as_ref() == Some(&next)
                {
                    return Ok(());
                }
            }
            Err(error) => {
                tracing::debug!(sandbox_id = %claimed.id, error = %error, "periodic checkpoint capture requires writer resumption");
            }
        }
        self.require_resume(claimed, now).await
    }

    async fn capture(
        &self,
        claimed: &Sandbox,
        now: SystemTime,
    ) -> Result<(SandboxCheckpointRef, Vec<SandboxCheckpointAgentGuard>), SandboxStoreError> {
        let agents = self.agents(claimed).await?;
        if agents
            .iter()
            .any(|agent| agent.sandbox_id != claimed.id || agent.owner != claimed.owner)
        {
            return Err(SandboxStoreError::Corrupt(
                "checkpoint agent lineage is inconsistent".into(),
            ));
        }
        let slots = agents
            .iter()
            .map(|agent| AgentSlot::new(agent.id.to_string()))
            .collect::<Vec<_>>();
        let exclusions = self
            .adapter
            .checkpoint_exclusions(&slots)
            .map_err(provider_error)?;
        let guards = agents
            .iter()
            .filter(|agent| agent.status.is_live())
            .map(SandboxCheckpointAgentGuard::capture)
            .collect();
        let capture = claimed
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let token = capture
            .authority
            .writer_resume_token
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let handle = adopt_bound_runtime(self.adapter.as_ref(), claimed, &self.deployment_id)
            .await
            .map_err(provider_error)?;
        if self.store.get_sandbox(&claimed.id).await?.as_ref() != Some(claimed) {
            return Err(SandboxStoreError::Conflict);
        }
        let held = self
            .adapter
            .checkpoint_for_teardown(
                &handle,
                &CheckpointHoldRequest {
                    checkpoint: CheckpointRequest {
                        epoch: capture.checkpoint_epoch,
                        exclusions,
                    },
                    writer_resume_token: token.clone(),
                },
            )
            .await
            .map_err(provider_error)?;
        if held.checkpoint.epoch != capture.checkpoint_epoch
            || held.checkpoint.writers != WriterState::Held
            || !writer_resume_tokens_match(&held.writer_resume_token, token)
        {
            return Err(SandboxStoreError::Corrupt(
                "periodic checkpoint hold receipt is not exact".into(),
            ));
        }
        let mut snapshots = Vec::new();
        for agent in agents.iter().filter(|agent| agent.status.is_live()) {
            let conversation_id = agent
                .conversation
                .conversation_id
                .clone()
                .ok_or(SandboxStoreError::Conflict)?;
            let snapshot = self
                .adapter
                .capture_agent(&handle, &AgentSlot::new(agent.id.to_string()))
                .await
                .map_err(provider_error)?;
            snapshots.push(
                SandboxCheckpointConversation::new(agent.id.clone(), conversation_id, snapshot)
                    .map_err(model_error)?,
            );
        }
        let checkpoint = SandboxCheckpointRef::seal(&held.checkpoint, &snapshots, rfc3339_at(now))
            .map_err(model_error)?;
        Ok((checkpoint, guards))
    }
}

fn provider_error(error: layer3_adapter::AdapterError) -> SandboxStoreError {
    SandboxStoreError::Corrupt(error.to_string())
}
