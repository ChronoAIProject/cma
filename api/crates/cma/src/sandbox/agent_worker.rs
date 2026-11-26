use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::{AgentCreateAdmission, AgentLifecycleStore, SandboxStoreError};
use layer2_core::sandbox::{Agent, AgentOperationService, AgentUpdateRecorder, AgentWorkStep};
use layer2_core::{rfc3339_at, AgentId};
use layer3_adapter::SandboxAdapter;
use tokio::sync::Semaphore;

use super::agent_skills::AgentSkillResolver;
use super::live_agents::{DurableAgentConnectionLossHandler, LiveAgentRegistry};

mod acquisition_backoff;
mod attempt;
mod capacity;
#[cfg(test)]
pub(crate) use capacity::reconnect::RecoveryReconnect;
pub(crate) use capacity::reconnect::{CapacityRecoveryReconnect, ReconnectAction};
mod cleanup;
mod cleanup_publication;
mod cleanup_retry;
#[cfg(test)]
pub(super) use cleanup_retry::same_cleanup_claim;
mod completion_lookup;
mod connection;
mod connection_output;
mod connection_recovery_progress;
mod connection_retry;
#[cfg(test)]
pub(super) use connection_recovery_progress::same_recovery_claim;
mod create_completion;
mod first_message;
mod first_message_publication;
mod inspection_park;
mod ledger_retry;
mod ledger_telemetry;
mod live_skills;
mod ownership_pacing;
mod prepared_open_ready;
mod prepared_replay;
mod reconcile;
mod restart;
mod restart_teardown;
pub(crate) use reconcile::ConnectionSweep;
mod startup;
mod startup_hold_work;
mod startup_holds;
mod startup_open;
mod startup_publication;
mod startup_support;
mod startup_timing;
mod startup_wait;
mod support;
#[cfg(test)]
pub(super) use support::WorkerFailure;

const DEFAULT_LEASE: Duration = Duration::from_secs(18 * 60);
const DEFAULT_RETRY: Duration = Duration::from_secs(30);
const DEFAULT_OPEN_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_SIGN_IN_TIMEOUT: Duration = Duration::from_secs(17 * 60);
const CREATE_COMPLETION_ADMISSION_SLOTS: usize = 32;
const CREATE_COMPLETION_ADMISSION_TIMEOUT: Duration = Duration::from_millis(250);

type CompletionAdmissionFuture =
    Pin<Box<dyn Future<Output = Result<Option<AgentCreateAdmission>, SandboxStoreError>> + Send>>;

#[cfg(test)]
type CompletionAdmissionLookup = Arc<dyn Fn(Agent) -> CompletionAdmissionFuture + Send + Sync>;

pub(crate) struct AgentWorkerDependencies {
    pub store: Arc<dyn AgentLifecycleStore>,
    pub adapter: Arc<dyn SandboxAdapter>,
    pub skills: Arc<dyn AgentSkillResolver>,
    pub registry: Arc<LiveAgentRegistry>,
    pub updates: Arc<dyn AgentUpdateRecorder>,
    pub operations: Arc<AgentOperationService>,
}

#[derive(Clone)]
pub(crate) struct AgentWorkerConfig {
    pub capture_delivery: Option<super::native_retirement_delivery::CaptureDelivery>,
    pub ownership_retry_hints: Option<Arc<dyn super::ownership_retry_hint::OwnershipRetryHints>>,
    pub retirement_reporter:
        Option<Arc<dyn crate::store::native_retirement_action::RetirementRecords>>,
    pub ownership: Option<Arc<layer2_core::agent_ownership::OwnershipCoordinator>>,
    skip_ineligible_reconciliation: bool,
    defer_foreign_reconciliation: bool,
    pub parent_not_ready_pacing_enabled: bool,
    startup_sibling_hold_enabled: bool,
    pub retry_single_marker_enabled: bool,
    pub work_retry_policy_enabled: bool,
    pub cleanup_continuation_enabled: bool,
    pub owner_stop_settlement_enabled: bool,
    pub access_transport_migration: Option<crate::config::AgentAccessMigration>,
    pub managed_documents_enabled: bool,
    pub holder: String,
    pub deployment_id: String,
    pub lease_duration: Duration,
    pub retry_delay: Duration,
    pub open_timeout: Duration,
    pub sign_in_timeout: Duration,
    parent_readiness: Option<Arc<tokio::sync::Notify>>,
}

impl AgentWorkerConfig {
    pub fn new(holder: impl Into<String>, deployment_id: impl Into<String>) -> Self {
        Self {
            capture_delivery: None,
            retirement_reporter: None,
            ownership: None,
            skip_ineligible_reconciliation: false,
            defer_foreign_reconciliation: false,
            ownership_retry_hints: None,
            parent_not_ready_pacing_enabled: false,
            startup_sibling_hold_enabled: false,
            retry_single_marker_enabled: false,
            work_retry_policy_enabled: false,
            cleanup_continuation_enabled: false,
            owner_stop_settlement_enabled: false,
            managed_documents_enabled: false,
            access_transport_migration: None,
            holder: holder.into(),
            deployment_id: deployment_id.into(),
            lease_duration: DEFAULT_LEASE,
            retry_delay: DEFAULT_RETRY,
            open_timeout: DEFAULT_OPEN_TIMEOUT,
            sign_in_timeout: DEFAULT_SIGN_IN_TIMEOUT,
            parent_readiness: None,
        }
    }

    pub(crate) fn with_access_transport_migration(
        mut self,
        value: Option<crate::config::AgentAccessMigration>,
    ) -> Self {
        self.access_transport_migration = value;
        self
    }

    pub(crate) fn with_ownership(
        mut self,
        ownership: Option<Arc<layer2_core::agent_ownership::OwnershipCoordinator>>,
    ) -> Self {
        self.ownership = ownership;
        self
    }

    pub(crate) fn with_ownership_retry_hints(
        mut self,
        hints: Option<Arc<dyn super::ownership_retry_hint::OwnershipRetryHints>>,
    ) -> Self {
        self.ownership_retry_hints = hints;
        self
    }

    pub(crate) fn with_managed_documents(mut self, enabled: bool) -> Self {
        self.managed_documents_enabled = enabled;
        self
    }

    pub(crate) fn with_skip_ineligible_reconciliation(mut self, enabled: bool) -> Self {
        self.skip_ineligible_reconciliation = enabled;
        self
    }

    pub(crate) fn with_defer_foreign_reconciliation(mut self, enabled: bool) -> Self {
        self.defer_foreign_reconciliation = enabled;
        self
    }

    pub(crate) fn with_startup_sibling_hold(mut self, enabled: bool) -> Self {
        self.startup_sibling_hold_enabled = enabled;
        self
    }

    pub(crate) fn with_retry_single_marker(mut self, enabled: bool) -> Self {
        self.retry_single_marker_enabled = enabled;
        self
    }

    pub(crate) fn with_work_retry_policy(mut self, enabled: bool) -> Self {
        self.work_retry_policy_enabled = enabled;
        self
    }

    pub(crate) fn with_cleanup_continuation(mut self, enabled: bool) -> Self {
        self.cleanup_continuation_enabled = enabled;
        self
    }

    pub(crate) fn with_parent_readiness(mut self, readiness: Arc<tokio::sync::Notify>) -> Self {
        self.parent_readiness = Some(readiness);
        self
    }
}

pub(crate) struct AgentWorker {
    dependencies: AgentWorkerDependencies,
    config: AgentWorkerConfig,
    acquisition_backoff: acquisition_backoff::Backoff,
    startup_holds: Option<startup_holds::StartupHolds>,
    maintenance: reconcile::MaintenanceChecks,
    capacity_restart: Arc<OnceLock<Arc<dyn CapacityRecoveryReconnect>>>,
    startup_timings: Mutex<HashMap<AgentId, startup_timing::StartupPhaseDurations>>,
    completion_admission_slots: Arc<Semaphore>,
    #[cfg(test)]
    completion_admission_lookup: Mutex<Option<CompletionAdmissionLookup>>,
}

impl AgentWorker {
    pub fn new(dependencies: AgentWorkerDependencies, config: AgentWorkerConfig) -> Arc<Self> {
        let capacity_restart = Arc::new(OnceLock::new());
        dependencies.registry.install_connection_loss_handler(
            DurableAgentConnectionLossHandler::with_capacity_recovery(
                dependencies.store.clone(),
                capacity_restart.clone(),
            ),
        );
        Arc::new(Self {
            dependencies,
            startup_holds: config
                .startup_sibling_hold_enabled
                .then(startup_holds::StartupHolds::default),
            config,
            acquisition_backoff: acquisition_backoff::Backoff::default(),
            maintenance: reconcile::MaintenanceChecks::default(),
            capacity_restart,
            startup_timings: Mutex::new(HashMap::new()),
            completion_admission_slots: Arc::new(Semaphore::new(CREATE_COMPLETION_ADMISSION_SLOTS)),
            #[cfg(test)]
            completion_admission_lookup: Mutex::new(None),
        })
    }

    fn retain_startup_timing(
        &self,
        agent: &AgentId,
        timing: startup_timing::StartupPhaseDurations,
    ) {
        let mut timings = match self.startup_timings.lock() {
            Ok(timings) => timings,
            Err(poisoned) => {
                tracing::error!("agent startup timing state was poisoned; recovering it");
                poisoned.into_inner()
            }
        };
        // Startup phase timing is deliberately process-local. Bound this
        // cache so abandoned or failed creates cannot grow it without limit;
        // an eviction leaves the eventual event explicitly phase-partial.
        if timings.len() >= 1024 && !timings.contains_key(agent) {
            if let Some(oldest) = timings.keys().next().cloned() {
                timings.remove(&oldest);
            }
        }
        timings.insert(agent.clone(), timing);
    }

    fn take_startup_timing(
        &self,
        agent: &AgentId,
    ) -> Option<startup_timing::StartupPhaseDurations> {
        let mut timings = match self.startup_timings.lock() {
            Ok(timings) => timings,
            Err(poisoned) => {
                tracing::error!("agent startup timing state was poisoned; recovering it");
                poisoned.into_inner()
            }
        };
        timings.remove(agent)
    }

    pub(crate) async fn due_work(
        &self,
        now: SystemTime,
        limit: u16,
    ) -> Result<Vec<AgentId>, SandboxStoreError> {
        let holds = match &self.config.ownership {
            Some(coordinator) => coordinator.background_cleanup_holds().await,
            None => Default::default(),
        };
        self.due_with_startup_holds(&rfc3339_at(now), limit, &holds)
            .await
    }

    async fn execute(&self, agent: Agent, now: SystemTime) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let claimed = agent.clone();
        let result = if matches!(
            agent.work.step,
            AgentWorkStep::RemoveHome
                | AgentWorkStep::RemoveWorktree
                | AgentWorkStep::EnsureProcessClosed
        ) {
            let preflight = self.prepare_retired_cleanup(&agent).await;
            let at = now + started.elapsed();
            match preflight {
                Ok(Some(parents)) => self.complete_retired_cleanup(&agent, parents, at).await,
                Ok(None) => self.execute_step(agent, at).await,
                Err(error) => return self.finish_cleanup_preflight(&agent, error, at).await,
            }
        } else {
            self.execute_step(agent, now).await
        };
        let result = self
            .finish_capacity_attempt(&claimed, result, now + started.elapsed())
            .await;
        self.finish_cleanup_attempt(&claimed, result, now + started.elapsed())
            .await
    }

    #[cfg(test)]
    pub(in crate::sandbox) async fn execute_claimed_for_test(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.execute(agent, now).await
    }

    /// Keep the per-step futures behind individual heap boundaries. The
    /// combined match future otherwise contains every startup and cleanup
    /// state machine and overflows the default test/runtime thread stack before
    /// the selected step is even polled.
    fn execute_step(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Pin<Box<dyn Future<Output = Result<(), SandboxStoreError>> + Send + '_>> {
        match agent.work.step {
            AgentWorkStep::InstallSkill => Box::pin(self.install_skill(agent, now)),
            AgentWorkStep::WaitForWorkspace => Box::pin(self.wait_for_workspace(agent, now)),
            AgentWorkStep::AddWorktree => Box::pin(self.add_worktree(agent, now)),
            AgentWorkStep::MaterializeHome => Box::pin(self.materialize_home(agent, now)),
            AgentWorkStep::OpenConversation => Box::pin(self.open_conversation(agent, now)),
            AgentWorkStep::PublishReady => Box::pin(self.publish_ready(agent, now)),
            AgentWorkStep::PublishConnection => Box::pin(self.publish_connection(agent, now)),
            AgentWorkStep::DispatchFirstMessage => {
                Box::pin(self.dispatch_first_message(agent, now))
            }
            AgentWorkStep::FenceInteraction => Box::pin(self.fence_interaction(agent, now)),
            AgentWorkStep::CloseProcess => Box::pin(self.close_process(agent, now)),
            AgentWorkStep::PublishStopped => Box::pin(self.publish_stopped(agent, now)),
            AgentWorkStep::EnsureProcessClosed => Box::pin(self.ensure_process_closed(agent, now)),
            AgentWorkStep::RemoveHome => Box::pin(self.remove_home(agent, now)),
            AgentWorkStep::RemoveWorktree => Box::pin(self.remove_worktree(agent, now)),
            AgentWorkStep::PublishDeleted => Box::pin(self.publish_deleted(agent, now)),
            AgentWorkStep::RestartConversation => Box::pin(self.restart_conversation(agent, now)),
            AgentWorkStep::RestartTeardown => Box::pin(self.restart_teardown(agent, now)),
            AgentWorkStep::None => Box::pin(async { Err(SandboxStoreError::Conflict) }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_admission_slots_are_bounded_and_nonwaiting() {
        let slots = Arc::new(Semaphore::new(CREATE_COMPLETION_ADMISSION_SLOTS));
        let mut permits = Vec::new();
        for _ in 0..CREATE_COMPLETION_ADMISSION_SLOTS {
            permits.push(Arc::clone(&slots).try_acquire_owned().unwrap());
        }
        assert!(Arc::clone(&slots).try_acquire_owned().is_err());
        drop(permits.pop());
        assert!(Arc::clone(&slots).try_acquire_owned().is_ok());
    }
}
