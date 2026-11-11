use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::{AgentLifecycleStore, SandboxStoreError};
use layer2_core::sandbox::{Agent, AgentOperationService, AgentUpdateRecorder, AgentWorkStep};
use layer2_core::{rfc3339_at, AgentId};
use layer3_adapter::SandboxAdapter;

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
    pub parent_not_ready_pacing_enabled: bool,
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
            parent_not_ready_pacing_enabled: false,
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

    pub(crate) fn with_managed_documents(mut self, enabled: bool) -> Self {
        self.managed_documents_enabled = enabled;
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
        })
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

    /// Follow committed startup steps immediately, keeping each durable claim.
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
            if !matches!(
                agent.work.step,
                AgentWorkStep::AddWorktree
                    | AgentWorkStep::MaterializeHome
                    | AgentWorkStep::OpenConversation
                    | AgentWorkStep::PublishReady
                    | AgentWorkStep::PublishConnection
                    | AgentWorkStep::DispatchFirstMessage
            ) || !agent.work.is_due(&rfc3339_at(now + started.elapsed()))
            {
                break;
            }
            if !self.run_one(id, now + started.elapsed()).await? {
                break;
            }
        }
        Ok(true)
    }

    async fn execute(&self, agent: Agent, now: SystemTime) -> Result<(), SandboxStoreError> {
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
