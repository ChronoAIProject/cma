//! Keep a completed Open recoverable when the combined Ready snapshot changed.
use super::*;
use crate::sandbox::agent_worker::startup_publication::{
    holds_startup_authority, only_startup_metadata_changed,
};
use layer2_core::sandbox::Sandbox;
use layer3_adapter::AgentOpenRequest;

impl AgentWorker {
    pub(in crate::sandbox) async fn retain_completed_open_after_conflict(
        &self,
        opened: &Agent,
        sandbox: &Sandbox,
        request: &AgentOpenRequest,
        attachment: &LiveAgentAttachment,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let current = self
            .dependencies
            .store
            .get_agent(&opened.id)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        // Conflict means the combined publication was not committed. A fresh
        // read must still prove our exact intent and lease, never a successor's.
        if (current != *opened && !only_startup_metadata_changed(opened, &current))
            || current.work.step != AgentWorkStep::OpenConversation
            || !holds_startup_authority(&current, &self.config.holder, now + started.elapsed())
        {
            return Err(SandboxStoreError::Conflict);
        }
        let identity = attachment.identity();
        let fence = claimed_fence(&current, &self.config.holder)?;
        let target = LiveAgentTarget {
            agent_id: current.id.clone(),
            conversation_id: identity.conversation_id.clone(),
            runtime_generation: identity.runtime_generation,
        };
        let staged = self
            .dependencies
            .registry
            .staged_attachment(&target, fence)
            .map_err(|_| SandboxStoreError::Conflict)?;
        if staged
            .as_ref()
            .is_none_or(|staged| staged.identity() != identity)
        {
            return Err(SandboxStoreError::Conflict);
        }
        // Record the completed effect, not readiness. The ordinary PublishReady
        // step rechecks current parents/runtime before exposing this channel.
        // Reusing it avoids another Open and releases the otherwise 18m lease.
        self.startup_progress(
            current,
            AgentWorkStep::PublishReady,
            now + started.elapsed(),
            RequestEvidence {
                step: AgentWorkStep::OpenConversation,
                operation_id: request.operation_id.clone(),
                request_sha256: request.request_sha256.clone(),
            },
            Some((identity.conversation_id, identity.runtime_generation)),
            sandbox,
        )
        .await
    }
}
