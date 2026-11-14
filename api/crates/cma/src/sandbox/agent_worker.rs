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

mod capacity;
#[cfg(test)]
pub(crate) use capacity::reconnect::RecoveryReconnect;
pub(crate) use capacity::reconnect::{CapacityRecoveryReconnect, ReconnectAction};
mod cleanup;
mod cleanup_publication;
mod connection;
mod connection_output;
mod connection_retry;
mod create_completion;
mod first_message;
mod first_message_publication;
mod ledger_retry;
mod ledger_telemetry;
mod live_skills;
mod prepared_open_ready;
mod prepared_replay;
mod reconcile;
mod startup;
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
    pub ownership: Option<Arc<layer2_core::agent_ownership::OwnershipCoordinator>>,
    pub parent_not_ready_pacing_enabled: bool,
    pub retry_single_marker_enabled: bool,
    pub work_retry_policy_enabled: bool,
    pub cleanup_continuation_enabled: bool,
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
            ownership: None,
            parent_not_ready_pacing_enabled: false,
            retry_single_marker_enabled: false,
            work_retry_policy_enabled: false,
            cleanup_continuation_enabled: false,
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

    pub(crate) fn with_managed_documents(mut self, enabled: bool) -> Self {
        self.managed_documents_enabled = enabled;
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
            config,
            maintenance: reconcile::MaintenanceChecks::default(),
            capacity_restart,
            startup_timings: Mutex::new(HashMap::new()),
            completion_admission_slots: Arc::new(Semaphore::new(CREATE_COMPLETION_ADMISSION_SLOTS)),
            #[cfg(test)]
            completion_admission_lookup: Mutex::new(None),
        })
    }

    fn completion_admission(&self, agent: Agent) -> CompletionAdmissionFuture {
        #[cfg(test)]
        if let Some(lookup) = self
            .completion_admission_lookup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            return lookup(agent);
        }
        let store = self.dependencies.store.clone();
        Box::pin(async move { store.agent_create_admission(&agent).await })
    }

    #[cfg(test)]
    pub(crate) fn set_completion_admission_lookup<F, Fut>(&self, lookup: F)
    where
        F: Fn(Agent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<AgentCreateAdmission>, SandboxStoreError>>
            + Send
            + 'static,
    {
        let lookup: CompletionAdmissionLookup = Arc::new(move |agent| Box::pin(lookup(agent)));
        *self
            .completion_admission_lookup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(lookup);
    }

    #[cfg(test)]
    pub(crate) fn schedule_completion_event_for_test(&self, agent: Agent) {
        self.schedule_completion_event(
            agent,
            "ordinary",
            "unknown",
            create_completion::CompletionTiming {
                admission_ms: None,
                admitted_desktop: None,
                admitted_machine_included: None,
                ready_at: SystemTime::now(),
                publish_ready_ms: 0,
                startup_phases: None,
            },
        );
    }

    fn schedule_completion_event(
        &self,
        agent: Agent,
        ready_path: &'static str,
        runtime_path: &'static str,
        timing: create_completion::CompletionTiming,
    ) {
        let permit = match Arc::clone(&self.completion_admission_slots).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                tracing::warn!(
                    target: "agent_create_completion_lookup",
                    parent: None,
                    outcome = "admission_lookup_saturated",
                    "Agent create completion admission lookup was skipped"
                );
                create_completion::emit(&agent, ready_path, runtime_path, timing);
                return;
            }
        };
        let admission_lookup = self.completion_admission(agent.clone());
        tokio::spawn(async move {
            let _permit = permit;
            let admission =
                match tokio::time::timeout(CREATE_COMPLETION_ADMISSION_TIMEOUT, admission_lookup)
                    .await
                {
                    Ok(Ok(admission)) => admission,
                    Ok(Err(_)) => {
                        tracing::debug!(
                            target: "agent_create_completion_lookup",
                            parent: None,
                            outcome = "admission_lookup_failed",
                            "Agent create completion admission lookup failed"
                        );
                        None
                    }
                    Err(_) => {
                        tracing::debug!(
                            target: "agent_create_completion_lookup",
                            parent: None,
                            outcome = "admission_lookup_timeout",
                            "Agent create completion admission lookup timed out"
                        );
                        None
                    }
                };
            create_completion::emit(
                &agent,
                ready_path,
                runtime_path,
                create_completion::CompletionTiming {
                    admission_ms: admission.as_ref().map(|value| value.admitted_at_ms),
                    admitted_desktop: admission.as_ref().and_then(|value| value.desktop),
                    admitted_machine_included: admission
                        .as_ref()
                        .and_then(|value| value.machine_included),
                    ..timing
                },
            );
        });
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
        self.dependencies
            .store
            .due_agent_work(&rfc3339_at(now), limit)
            .await
    }

    pub async fn run_one(&self, id: &AgentId, now: SystemTime) -> Result<bool, SandboxStoreError> {
        if let Some(coordinator) = &self.config.ownership {
            let Some(authority) = coordinator
                .acquire(id)
                .await
                .map_err(|_| SandboxStoreError::Conflict)?
            else {
                return Ok(false);
            };
            let result = authority
                .run(self.run_claimed(id, now))
                .await
                .map_err(|_| SandboxStoreError::Conflict)?;
            if result.is_ok()
                && self
                    .dependencies
                    .store
                    .get_agent(id)
                    .await?
                    .is_none_or(|agent| {
                        !agent.status.is_live() && agent.work.step == AgentWorkStep::None
                    })
            {
                self.dependencies.registry.detach_owner(&authority.token);
                coordinator
                    .handoff(&authority, authority.revoke_transports())
                    .await
                    .map_err(|_| SandboxStoreError::Conflict)?;
            }
            return result;
        }
        self.run_claimed(id, now).await
    }

    async fn run_claimed(&self, id: &AgentId, now: SystemTime) -> Result<bool, SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let Some(agent) = self
            .dependencies
            .store
            .claim_agent_work(
                id,
                &self.config.holder,
                rfc3339_at(now),
                rfc3339_at(now + self.config.lease_duration),
            )
            .await?
        else {
            return Ok(false);
        };
        let claim_ms = started.elapsed().as_millis() as u64;
        let step = agent.work.step;
        let executing = tokio::time::Instant::now();
        let result = Box::pin(self.execute(agent, now + started.elapsed())).await;
        tracing::info!(
            agent_id = %id,
            ?step,
            claim_ms,
            execution_ms = executing.elapsed().as_millis() as u64,
            published = result.is_ok(),
            "Agent work attempt completed"
        );
        result?;
        Ok(true)
    }

    /// Follow bounded, committed progress immediately, keeping each durable claim.
    pub(crate) async fn run_scheduled(
        &self,
        id: &AgentId,
        now: SystemTime,
    ) -> Result<bool, SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let progressed = self.run_one(id, now).await?;
        if !progressed {
            return Ok(false);
        }
        for _ in 0..6 {
            let Some(agent) = self.dependencies.store.get_agent(id).await? else {
                break;
            };
            let startup = matches!(
                agent.work.step,
                AgentWorkStep::AddWorktree
                    | AgentWorkStep::MaterializeHome
                    | AgentWorkStep::OpenConversation
                    | AgentWorkStep::PublishReady
                    | AgentWorkStep::PublishConnection
                    | AgentWorkStep::DispatchFirstMessage
            );
            let cleanup = self.config.cleanup_continuation_enabled
                && agent.work.attempts == 0
                && matches!(
                    agent.work.step,
                    AgentWorkStep::FenceInteraction
                        | AgentWorkStep::CloseProcess
                        | AgentWorkStep::PublishStopped
                        | AgentWorkStep::EnsureProcessClosed
                        | AgentWorkStep::RemoveHome
                        | AgentWorkStep::RemoveWorktree
                        | AgentWorkStep::PublishDeleted
                );
            // A published retry is not progress, even when execution exceeded its
            // deadline. Reclaim only committed successful cleanup transitions.
            if !(startup || cleanup) || !agent.work.is_due(&rfc3339_at(now + started.elapsed())) {
                break;
            }
            if !self.run_one(id, now + started.elapsed()).await? {
                break;
            }
        }
        Ok(true)
    }

    async fn execute(&self, agent: Agent, now: SystemTime) -> Result<(), SandboxStoreError> {
        if matches!(
            agent.work.step,
            AgentWorkStep::RemoveHome
                | AgentWorkStep::RemoveWorktree
                | AgentWorkStep::EnsureProcessClosed
        ) {
            let parent = self
                .dependencies
                .store
                .get_sandbox(&agent.sandbox_id)
                .await?
                .ok_or(SandboxStoreError::NotFound)?;
            if parent.retirement_archive_pending() {
                return Err(SandboxStoreError::Conflict);
            }
        }
        let started = tokio::time::Instant::now();
        let claimed = agent.clone();
        let result = match agent.work.step {
            AgentWorkStep::InstallSkill => self.install_skill(agent, now).await,
            AgentWorkStep::WaitForWorkspace => self.wait_for_workspace(agent, now).await,
            AgentWorkStep::AddWorktree => self.add_worktree(agent, now).await,
            AgentWorkStep::MaterializeHome => self.materialize_home(agent, now).await,
            AgentWorkStep::OpenConversation => self.open_conversation(agent, now).await,
            AgentWorkStep::PublishReady => self.publish_ready(agent, now).await,
            AgentWorkStep::PublishConnection => self.publish_connection(agent, now).await,
            AgentWorkStep::DispatchFirstMessage => self.dispatch_first_message(agent, now).await,
            AgentWorkStep::FenceInteraction => self.fence_interaction(agent, now).await,
            AgentWorkStep::CloseProcess => self.close_process(agent, now).await,
            AgentWorkStep::PublishStopped => self.publish_stopped(agent, now).await,
            AgentWorkStep::EnsureProcessClosed => self.ensure_process_closed(agent, now).await,
            AgentWorkStep::RemoveHome => self.remove_home(agent, now).await,
            AgentWorkStep::RemoveWorktree => self.remove_worktree(agent, now).await,
            AgentWorkStep::PublishDeleted => self.publish_deleted(agent, now).await,
            AgentWorkStep::None => Err(SandboxStoreError::Conflict),
        };
        self.finish_capacity_attempt(&claimed, result, now + started.elapsed())
            .await
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
