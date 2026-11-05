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
        let mut candidates = 0_u64;
        let mut counts = ReconcileCounts::default();
        loop {
            let page = self
                .dependencies
                .store
                .live_agent_connections(&PageRequest::new(MAX_PAGE_SIZE, cursor).expect("valid"))
                .await?;
            for candidate in page.items {
                candidates += 1;
                let operation = self.reconcile_candidate(candidate.clone(), now, &mut counts);
                if let Some(coordinator) = &self.config.ownership {
                    let Some(authority) = coordinator
                        .acquire(&candidate.agent_id)
                        .await
                        .map_err(|_| SandboxStoreError::Conflict)?
                    else {
                        continue;
                    };
                    authority
                        .run(operation)
                        .await
                        .map_err(|_| SandboxStoreError::Conflict)??;
                } else {
                    operation.await?;
                }
            }
            let Some(next) = page.next else {
                // One content-free summary per completed scheduler sweep (at most
                // once per second), not one record per Agent or database call.
                tracing::info!(
                    parent: None,
                    candidates,
                    context_current = counts.context_current,
                    context_deferred = counts.context_deferred,
                    context_checks = counts.context_checks,
                    context_throttled = counts.context_throttled,
                    transport_attempts = counts.transport_attempts,
                    scheduled = counts.scheduled,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "Agent connection reconciliation completed"
                );
                return Ok(counts.scheduled);
            };
            cursor = Some(next);
        }
    }
    async fn reconcile_candidate(
        &self,
        candidate: layer2_core::sandbox::store::AgentConnectionRecoveryCandidate,
        now: SystemTime,
        counts: &mut ReconcileCounts,
    ) -> Result<(), SandboxStoreError> {
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
                let migration = self
                    .config
                    .access_transport_migration
                    .as_ref()
                    .filter(|migration| migration.includes(candidate.agent_id.as_str()));
                let desired_revision =
                    migration.map_or(MANAGED_GUIDANCE_REVISION, |migration| migration.revision);
                if candidate.managed_guidance_revision >= desired_revision {
                    counts.context_current += 1;
                    return Ok(());
                }
                // Discovery already read and validated the complete Agent.
                // Deferring a busy/unconfigured Agent grants no authority:
                // the next sweep observes changes, and admission still
                // rereads all current state before any maintenance write.
                if !candidate.managed_context_eligible {
                    counts.context_deferred += 1;
                    return Ok(());
                }
                if migration.is_some() && counts.transport_attempts >= 2 {
                    counts.context_deferred += 1;
                    return Ok(());
                }
                if !self.maintenance.should_check(&candidate, desired_revision) {
                    counts.context_throttled += 1;
                    return Ok(());
                }
                counts.context_checks += 1;
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
                    counts.transport_attempts += 1;
                }
                if let Err(error) = result {
                    tracing::debug!(agent_id=%candidate.agent_id, error=%error, "managed context refresh deferred");
                }
            }
            return Ok(());
        }
        if self
            .dependencies
            .store
            .schedule_agent_connection_recovery(candidate.restart_request(rfc3339_at(now)))
            .await?
            == AgentConnectionRecoveryOutcome::Scheduled
        {
            counts.scheduled += 1;
        }
        Ok(())
    }
}

#[derive(Default)]
struct ReconcileCounts {
    scheduled: usize,
    context_current: u64,
    context_deferred: u64,
    context_checks: u64,
    context_throttled: u64,
    transport_attempts: u64,
}
