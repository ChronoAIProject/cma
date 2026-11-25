use std::time::{Duration, SystemTime};

use tokio::time::Instant;

use layer2_core::agent_ownership::ForeignReconciliationOwners;
use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    AgentConnectionRecoveryCandidate, AgentConnectionRecoveryOutcome, PageRequest,
    SandboxStoreError, MAX_PAGE_SIZE,
};

use super::{AgentWorker, ReconnectAction};
use crate::sandbox::live_agents::LiveAgentTarget;

mod maintenance;
pub(super) use maintenance::MaintenanceChecks;

const MANAGED_GUIDANCE_REVISION: u64 = 1;
const SLICE_PAGE_SIZE: u16 = 32;
// Check between candidates. The background deadline still bounds a single slow
// operation and its cancellation must leave the cursor at the preceding Agent.
const SLICE_BUDGET: Duration = Duration::from_secs(1);
const MAX_STALLED_ADMISSIONS: u8 = 2;

/// Per Scheduler progress. Durable reads and ownership are revalidated on each
/// admission. Restarting with no cursor safely begins at the first Agent.
#[derive(Default)]
pub(crate) struct ConnectionSweep {
    cursor: Option<String>,
    started: Option<Instant>,
    pages: u64,
    candidates: u64,
    stalled_admissions: u8,
    rewinds: u64,
    counts: ReconcileCounts,
}

impl ConnectionSweep {
    #[cfg(test)]
    pub(crate) fn visited(&self) -> (u64, u64) {
        (self.pages, self.candidates)
    }

    fn start_slice(&mut self) {
        if self.stalled_admissions == MAX_STALLED_ADMISSIONS {
            // Revisit earlier IDs without skipping the interrupted Agent or
            // starting a new sweep with a fresh migration allowance.
            self.cursor = None;
            self.stalled_admissions = 0;
            self.rewinds += 1;
        }
        if self.cursor.is_some() {
            self.stalled_admissions += 1;
        }
    }

    fn finish(&mut self) -> usize {
        let sweep = std::mem::take(self);
        let counts = sweep.counts;
        tracing::info!(
            parent: None,
            pages = sweep.pages,
            candidates = sweep.candidates,
            rewinds = sweep.rewinds,
            ownership_attempted = counts.ownership_attempted,
            ownership_skipped = counts.ownership_skipped,
            foreign_ownership_deferred = counts.foreign_ownership_deferred,
            recovery_deferred = counts.recovery_deferred,
            context_runtime_deferred = counts.context_runtime_deferred,
            context_current = counts.context_current,
            context_deferred = counts.context_deferred,
            context_checks = counts.context_checks,
            context_throttled = counts.context_throttled,
            transport_attempts = counts.transport_attempts,
            scheduled = counts.scheduled,
            elapsed_ms = sweep.started.expect("admitted sweep").elapsed().as_millis() as u64,
            "Agent connection reconciliation completed"
        );
        counts.scheduled
    }
}

impl AgentWorker {
    /// Reconcile every durable live conversation into the process-local registry.
    pub(crate) async fn reconcile_connections(
        &self,
        now: SystemTime,
    ) -> Result<usize, SandboxStoreError> {
        let mut sweep = ConnectionSweep::default();
        while !self
            .reconcile_page(now, &mut sweep, MAX_PAGE_SIZE, None)
            .await?
        {}
        Ok(sweep.finish())
    }

    pub(crate) async fn reconcile_connection_slice(
        &self,
        now: SystemTime,
        sweep: &mut ConnectionSweep,
    ) -> Result<bool, SandboxStoreError> {
        sweep.start_slice();
        if self
            .reconcile_page(now, sweep, SLICE_PAGE_SIZE, Some(SLICE_BUDGET))
            .await?
        {
            sweep.finish();
            return Ok(true);
        }
        Ok(false)
    }

    async fn reconcile_page(
        &self,
        now: SystemTime,
        sweep: &mut ConnectionSweep,
        limit: u16,
        budget: Option<Duration>,
    ) -> Result<bool, SandboxStoreError> {
        let started = Instant::now();
        sweep.started.get_or_insert(started);
        let page = self
            .dependencies
            .store
            .live_agent_connections(
                &PageRequest::new(limit, sweep.cursor.clone())
                    .expect("valid reconciliation page size"),
            )
            .await?;
        sweep.pages += 1;
        let mut owners = match &self.config.ownership {
            Some(coordinator) if self.config.defer_foreign_reconciliation => {
                let eligible: Vec<_> = page
                    .items
                    .iter()
                    .filter_map(|candidate| {
                        let target = Self::reconciliation_target(candidate);
                        (!self.skip_reconciliation_ownership(candidate, &target))
                            .then_some(&candidate.agent_id)
                    })
                    .collect();
                coordinator
                    .observe_foreign_reconciliation_owners(eligible)
                    .await
            }
            _ => ForeignReconciliationOwners::default(),
        };
        let mut candidates = page.items.into_iter().peekable();
        while let Some(candidate) = candidates.next() {
            sweep.candidates += 1;
            let cursor = budget.map(|_| candidate.agent_id.to_string());
            self.reconcile_candidate_with_ownership(candidate, now, &mut sweep.counts, &mut owners)
                .await?;
            // Resume from the last settled item, including after timeout or cancellation.
            if let Some(cursor) = cursor {
                sweep.cursor = Some(cursor);
                sweep.stalled_admissions = 0;
            }
            if candidates.peek().is_some()
                && budget.is_some_and(|budget| started.elapsed() >= budget)
            {
                return Ok(false);
            }
        }
        sweep.cursor = page.next;
        Ok(sweep.cursor.is_none())
    }

    fn reconciliation_target(candidate: &AgentConnectionRecoveryCandidate) -> LiveAgentTarget {
        LiveAgentTarget {
            agent_id: candidate.agent_id.clone(),
            conversation_id: candidate.conversation_id.clone(),
            runtime_generation: candidate.runtime_generation,
        }
    }

    fn skip_reconciliation_ownership(
        &self,
        candidate: &AgentConnectionRecoveryCandidate,
        target: &LiveAgentTarget,
    ) -> bool {
        // Discovery proves recovery ineligibility for this observation. Only a
        // matching attachment can still need maintenance.
        self.config.skip_ineligible_reconciliation
            && !candidate.recovery_eligible
            && !self.dependencies.registry.target_is_attached(target)
    }

    async fn reconcile_candidate_with_ownership(
        &self,
        candidate: AgentConnectionRecoveryCandidate,
        now: SystemTime,
        counts: &mut ReconcileCounts,
        owners: &mut ForeignReconciliationOwners,
    ) -> Result<(), SandboxStoreError> {
        let target = Self::reconciliation_target(&candidate);
        let Some(coordinator) = &self.config.ownership else {
            return self
                .reconcile_candidate(candidate, &target, now, counts)
                .await;
        };
        if self.skip_reconciliation_ownership(&candidate, &target) {
            counts.ownership_skipped += 1;
            counts.recovery_deferred += 1;
            return Ok(());
        }
        if coordinator.defer_reconciliation_for_foreign_owner(owners, &candidate.agent_id) {
            counts.foreign_ownership_deferred += 1;
            return Ok(());
        }
        if self
            .dependencies
            .store
            .agent_inspection_parked(&candidate.agent_id)
            .await?
        {
            counts.ownership_skipped += 1;
            return Ok(());
        }
        counts.ownership_attempted += 1;
        let Some(authority) = coordinator
            .acquire(&candidate.agent_id)
            .await
            .map_err(|_| SandboxStoreError::Conflict)?
        else {
            return Ok(());
        };
        authority
            .run(self.reconcile_candidate(candidate, &target, now, counts))
            .await
            .map_err(|_| SandboxStoreError::Conflict)?
    }

    async fn reconcile_candidate(
        &self,
        candidate: layer2_core::sandbox::store::AgentConnectionRecoveryCandidate,
        target: &LiveAgentTarget,
        now: SystemTime,
        counts: &mut ReconcileCounts,
    ) -> Result<(), SandboxStoreError> {
        if self.dependencies.registry.target_is_attached(target) {
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
                    // Explicit transport migration can prove native-goal
                    // absence after reattachment without a replayed goal
                    // snapshot. Leave that proof behind fresh admission.
                    if migration.is_none() && !connection.input.managed_context_may_be_ready().await
                    {
                        counts.context_runtime_deferred += 1;
                        return Ok(());
                    }
                }
                // Do not record a maintenance throttle for a local
                // refusal: the attachment may become ready before the
                // retry interval, and the next sweep must observe it.
                if !self.maintenance.should_check(&candidate, desired_revision) {
                    counts.context_throttled += 1;
                    return Ok(());
                }
                counts.context_checks += 1;
                // Retain the charge when cancellation leaves the outcome uncertain.
                if migration.is_some() {
                    counts.transport_attempts += 1;
                }
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
                if migration.is_some() && matches!(&result, Ok(false)) {
                    counts.transport_attempts -= 1;
                }
                if let Err(error) = result {
                    tracing::debug!(agent_id=%candidate.agent_id, error=%error, "managed context refresh deferred");
                }
            }
            return Ok(());
        }
        // The same sweep already validated the entire Agent. A paused,
        // stopping or already scheduled candidate cannot be recovered
        // by this observation. Changes remain visible on the next sweep.
        if !candidate.recovery_eligible {
            counts.recovery_deferred += 1;
            return Ok(());
        }
        if let Some(recovery) = self.capacity_restart.get() {
            match recovery.route(&candidate.agent_id).await {
                Ok(ReconnectAction::Ordinary) => {}
                Ok(ReconnectAction::Recovered) => {
                    counts.scheduled += 1;
                    return Ok(());
                }
                Ok(ReconnectAction::Deferred) | Err(_) => {
                    counts.recovery_deferred += 1;
                    return Ok(());
                }
            }
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
    ownership_attempted: u64,
    ownership_skipped: u64,
    foreign_ownership_deferred: u64,
    scheduled: usize,
    context_runtime_deferred: u64,
    recovery_deferred: u64,
    context_current: u64,
    context_deferred: u64,
    context_checks: u64,
    context_throttled: u64,
    transport_attempts: u64,
}
