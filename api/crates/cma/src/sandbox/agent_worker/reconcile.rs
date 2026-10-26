use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    AgentConnectionRecoveryOutcome, PageRequest, SandboxStoreError, MAX_PAGE_SIZE,
};

use super::AgentWorker;
use crate::sandbox::live_agents::LiveAgentTarget;

impl AgentWorker {
    /// Reconcile every durable live conversation into the process-local registry.
    pub(crate) async fn reconcile_connections(
        &self,
        now: SystemTime,
    ) -> Result<usize, SandboxStoreError> {
        let mut cursor = None;
        let mut scheduled = 0;
        loop {
            let page = self
                .dependencies
                .store
                .live_agent_connections(&PageRequest::new(MAX_PAGE_SIZE, cursor).expect("valid"))
                .await?;
            for candidate in page.items {
                let target = LiveAgentTarget {
                    agent_id: candidate.agent_id.clone(),
                    conversation_id: candidate.conversation_id.clone(),
                    runtime_generation: candidate.runtime_generation,
                };
                if self.dependencies.registry.target_is_attached(&target) {
                    // The service cheaply skips applied revisions and active work.
                    // Admission and the ordered writer recheck every mutable fence.
                    if self.config.managed_documents_enabled {
                        if let Err(error) = self
                            .dependencies
                            .operations
                            .refresh_managed_context(
                                &candidate.agent_id,
                                1,
                                crate::agent_lifecycle::guidance::managed_v3(),
                                &[
                                    crate::agent_lifecycle::guidance::managed_v3().into(),
                                    crate::agent_lifecycle::guidance::managed_v2().into(),
                                    crate::agent_lifecycle::guidance::GITHUB_API_V1.into(),
                                ],
                            )
                            .await
                        {
                            tracing::debug!(agent_id=%candidate.agent_id, error=%error, "managed context refresh deferred");
                        }
                    }
                    continue;
                }
                if self
                    .dependencies
                    .store
                    .schedule_agent_connection_recovery(candidate.restart_request(rfc3339_at(now)))
                    .await?
                    == AgentConnectionRecoveryOutcome::Scheduled
                {
                    scheduled += 1;
                }
            }
            let Some(next) = page.next else {
                return Ok(scheduled);
            };
            cursor = Some(next);
        }
    }
}
