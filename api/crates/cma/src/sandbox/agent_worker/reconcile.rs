use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    AgentConnectionRecoveryOutcome, PageRequest, SandboxStoreError, MAX_PAGE_SIZE,
};

use super::AgentWorker;
use crate::sandbox::live_agents::LiveAgentTarget;

mod maintenance;
pub(super) use maintenance::MaintenanceChecks;

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
        let mut context_checks = 0_u64;
        let mut context_throttled = 0_u64;
        let mut transport_attempts = 0_u64;
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
                    if let Some(agent) = self
                        .dependencies
                        .store
                        .get_agent(&candidate.agent_id)
                        .await?
                    {
                        let _ = self.reconcile_native_completion(&agent, now).await;
                    }
                    // The scan already validated this monotonic applied revision.
                    // Skip completed maintenance without rereading the entire Agent.
                    // Pending refreshes still reread and fence all mutable authority.
                    if self.config.managed_documents_enabled {
                        let migration = self
                            .config
                            .access_transport_migration
                            .as_ref()
                            .filter(|migration| migration.includes(candidate.agent_id.as_str()));
                        let desired_revision = migration
                            .map_or(MANAGED_GUIDANCE_REVISION, |migration| migration.revision);
                        if candidate.managed_guidance_revision >= desired_revision {
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
                        if migration.is_some() && transport_attempts >= 2 {
                            context_deferred += 1;
                            continue;
                        }
                        if !self.maintenance.should_check(&candidate, desired_revision) {
                            context_throttled += 1;
                            continue;
                        }
                        context_checks += 1;
                        let result = self
                            .dependencies
                            .operations
                            .refresh_managed_context_at_origin(
                                &candidate.agent_id,
                                desired_revision,
                                crate::agent_lifecycle::guidance::managed_v3(),
                                &[
                                    crate::agent_lifecycle::guidance::managed_v3().into(),
                                    crate::agent_lifecycle::guidance::managed_v2().into(),
                                    crate::agent_lifecycle::guidance::GITHUB_API_V1.into(),
                                ],
                                migration.map(|migration| migration.origin.as_str()),
                            )
                            .await;
                        if migration.is_some() && !matches!(&result, Ok(false)) {
                            transport_attempts += 1;
                        }
                        if let Err(error) = result {
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
                    context_checks,
                    context_throttled,
                    transport_attempts,
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
