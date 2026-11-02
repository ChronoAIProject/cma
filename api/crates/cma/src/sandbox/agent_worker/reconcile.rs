use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    AgentConnectionRecoveryOutcome, PageRequest, SandboxStoreError, MAX_PAGE_SIZE,
};

use super::AgentWorker;
use crate::sandbox::live_agents::LiveAgentTarget;

const MANAGED_GUIDANCE_REVISION: u64 = 1;

impl AgentWorker {
    /// Reconcile every durable live conversation into the process-local registry.
    pub(crate) async fn reconcile_connections(
        &self,
        now: SystemTime,
    ) -> Result<usize, SandboxStoreError> {
        let started = std::time::Instant::now();
        let mut cursor = None;
        let mut scheduled = 0;
        let mut candidates = 0_u64;
        let mut context_current = 0_u64;
        let mut context_deferred = 0_u64;
        let mut context_runtime_deferred = 0_u64;
        let mut context_checks = 0_u64;
        loop {
            let page = self
                .dependencies
                .store
                .live_agent_connections(&PageRequest::new(MAX_PAGE_SIZE, cursor).expect("valid"))
                .await?;
            for candidate in page.items {
                candidates += 1;
                let target = LiveAgentTarget {
                    agent_id: candidate.agent_id.clone(),
                    conversation_id: candidate.conversation_id.clone(),
                    runtime_generation: candidate.runtime_generation,
                };
                if self.dependencies.registry.target_is_attached(&target) {
                    // The scan already validated this monotonic applied revision.
                    // Skip completed maintenance without rereading the entire Agent.
                    // Pending refreshes still reread and fence all mutable authority.
                    if self.config.managed_documents_enabled {
                        if candidate.managed_guidance_revision >= MANAGED_GUIDANCE_REVISION {
                            context_current += 1;
                            continue;
                        }
                        // Discovery already read and validated the complete Agent.
                        // Deferring a busy/unconfigured Agent grants no authority:
                        // the next sweep observes changes, and admission still
                        // rereads all current state before any maintenance write.
                        if !candidate.managed_context_eligible {
                            context_deferred += 1;
                            continue;
                        }
                        let runtime_target = layer2_core::sandbox::AgentRuntimeTarget {
                            agent_id: candidate.agent_id.clone(),
                            conversation_id: candidate.conversation_id.clone(),
                            runtime_generation: candidate.runtime_generation,
                        };
                        if let Ok(connection) = layer2_core::sandbox::AgentRuntimeRegistry::resolve(
                            self.dependencies.registry.as_ref(),
                            &runtime_target,
                        )
                        .await
                        {
                            // A local refusal grants no authority and performs no
                            // runtime IO. Recheck it every sweep; a positive hint
                            // still takes the ordinary fresh admission/write path.
                            if !connection.input.managed_context_may_be_ready().await {
                                context_runtime_deferred += 1;
                                continue;
                            }
                        }
                        context_checks += 1;
                        if let Err(error) = self
                            .dependencies
                            .operations
                            .refresh_managed_context(
                                &candidate.agent_id,
                                MANAGED_GUIDANCE_REVISION,
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
                // One content-free summary per completed scheduler sweep (at most
                // once per second), not one record per Agent or database call.
                tracing::info!(
                    parent: None,
                    candidates,
                    context_current,
                    context_deferred,
                    context_runtime_deferred,
                    context_checks,
                    scheduled,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "Agent connection reconciliation completed"
                );
                return Ok(scheduled);
            };
            cursor = Some(next);
        }
    }
}
