use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::{AgentLifecycleStore, SandboxStoreError};
use layer2_core::sandbox::{Agent, AgentUpdateRecorder, AgentWorkStep};
use layer2_core::{rfc3339_at, AgentId};
use layer3_adapter::SandboxAdapter;

use super::agent_skills::AgentSkillResolver;
use super::live_agents::LiveAgentRegistry;

mod cleanup;
mod connection;
mod startup;
mod startup_support;
mod support;

const DEFAULT_LEASE: Duration = Duration::from_secs(15 * 60);
const DEFAULT_RETRY: Duration = Duration::from_secs(30);
const DEFAULT_OPEN_TIMEOUT: Duration = Duration::from_secs(120);
const WORK_BATCH_SIZE: u16 = 50;

pub(crate) struct AgentWorkerDependencies {
    pub store: Arc<dyn AgentLifecycleStore>,
    pub adapter: Arc<dyn SandboxAdapter>,
    pub skills: Arc<dyn AgentSkillResolver>,
    pub registry: Arc<LiveAgentRegistry>,
    pub updates: Arc<dyn AgentUpdateRecorder>,
}

#[derive(Clone)]
pub(crate) struct AgentWorkerConfig {
    pub holder: String,
    pub deployment_id: String,
    pub lease_duration: Duration,
    pub retry_delay: Duration,
    pub open_timeout: Duration,
}

impl AgentWorkerConfig {
    pub fn new(holder: impl Into<String>, deployment_id: impl Into<String>) -> Self {
        Self {
            holder: holder.into(),
            deployment_id: deployment_id.into(),
            lease_duration: DEFAULT_LEASE,
            retry_delay: DEFAULT_RETRY,
            open_timeout: DEFAULT_OPEN_TIMEOUT,
        }
    }
}

pub(crate) struct AgentWorker {
    dependencies: AgentWorkerDependencies,
    config: AgentWorkerConfig,
}

impl AgentWorker {
    pub fn new(dependencies: AgentWorkerDependencies, config: AgentWorkerConfig) -> Arc<Self> {
        Arc::new(Self {
            dependencies,
            config,
        })
    }

    pub async fn run_due(&self, now: SystemTime) -> Result<usize, SandboxStoreError> {
        let due = self
            .dependencies
            .store
            .due_agent_work(&rfc3339_at(now), WORK_BATCH_SIZE)
            .await?;
        let mut completed = 0;
        let mut first_error = None;
        for id in due {
            match self.run_one(&id, now).await {
                Ok(true) => completed += 1,
                Ok(false) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(completed), Err)
    }

    pub async fn run_one(&self, id: &AgentId, now: SystemTime) -> Result<bool, SandboxStoreError> {
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
        self.execute(agent, now).await?;
        Ok(true)
    }

    async fn execute(&self, agent: Agent, now: SystemTime) -> Result<(), SandboxStoreError> {
        match agent.work.step {
            AgentWorkStep::AddWorktree => self.add_worktree(agent, now).await,
            AgentWorkStep::MaterializeHome => self.materialize_home(agent, now).await,
            AgentWorkStep::OpenConversation => self.open_conversation(agent, now).await,
            AgentWorkStep::PublishReady => self.publish_ready(agent, now).await,
            AgentWorkStep::PublishConnection => self.publish_connection(agent, now).await,
            AgentWorkStep::DispatchFirstMessage => {
                self.defer_first_message_dispatch(agent, now).await
            }
            AgentWorkStep::FenceInteraction => self.fence_interaction(agent, now).await,
            AgentWorkStep::CloseProcess => self.close_process(agent, now).await,
            AgentWorkStep::PublishStopped => self.publish_stopped(agent, now).await,
            AgentWorkStep::EnsureProcessClosed => self.ensure_process_closed(agent, now).await,
            AgentWorkStep::RemoveHome => self.remove_home(agent, now).await,
            AgentWorkStep::RemoveWorktree => self.remove_worktree(agent, now).await,
            AgentWorkStep::PublishDeleted => self.publish_deleted(agent, now).await,
            AgentWorkStep::None => Err(SandboxStoreError::Conflict),
        }
    }
}
