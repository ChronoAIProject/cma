use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use layer3_adapter::wire::{Acknowledgement, CredentialReceipt, RequestFrame, ResponseFrame};
use layer3_adapter::{AgentLaunch, AgentSlot, InstallRequest};
use tokio::sync::{Mutex, RwLock};

use crate::bridge::Bridge;
use crate::checkpoint::CheckpointStore;
use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::install_state::RecoveryState;
use crate::paths::{path_is_directory, safe_component};
use crate::upstream::{CredentialHolder, PreparedProxies};
use crate::writer_fence::{SupervisorBoundary, WriterFence};

const MAX_AGENT_SLOTS: usize = 16;

#[path = "supervisor_agent.rs"]
mod agent;
#[path = "supervisor_agent_recovery.rs"]
mod agent_recovery;
#[path = "supervisor_agent_validation.rs"]
mod agent_validation;
#[path = "supervisor_mutations.rs"]
mod mutations;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Provisioned,
    Installed,
    Credentialed,
}

struct AgentProcess {
    launch: AgentLaunch,
    bridge: Arc<Bridge>,
}

pub(crate) struct Supervisor {
    config: RuntimeConfig,
    phase: RwLock<Phase>,
    operation_gate: Arc<Mutex<()>>,
    credential: CredentialHolder,
    agents: RwLock<BTreeMap<AgentSlot, AgentProcess>>,
    checkpoint: CheckpointStore,
    resources: crate::resources::Sampler,
    writer_boundary: SupervisorBoundary,
    http: reqwest::Client,
}

impl Supervisor {
    pub(crate) async fn start(
        config: RuntimeConfig,
        writer_boundary: SupervisorBoundary,
    ) -> Result<Arc<Self>> {
        let supervisor = Self::new(config, writer_boundary)?;
        supervisor.recover_journals().await?;
        Ok(supervisor)
    }

    pub(crate) fn new(
        config: RuntimeConfig,
        writer_boundary: SupervisorBoundary,
    ) -> Result<Arc<Self>> {
        crate::fs_durability::create_dir_all(&config.home).map_err(|error| {
            RuntimeError::unavailable("home_unavailable", format!("create sandbox home: {error}"))
        })?;
        if !path_is_directory(&config.home) {
            return Err(RuntimeError::rejected(
                "home_invalid",
                "sandbox home is not a directory",
            ));
        }
        let install_state = crate::install::recover(&config)?;
        let checkpoint = CheckpointStore::new(&config);
        Ok(Arc::new(Self {
            config,
            phase: RwLock::new(match install_state {
                RecoveryState::Completed => Phase::Installed,
                RecoveryState::Absent | RecoveryState::Terminal => Phase::Provisioned,
            }),
            operation_gate: Arc::new(Mutex::new(())),
            credential: CredentialHolder::new(),
            agents: RwLock::new(BTreeMap::new()),
            checkpoint,
            resources: crate::resources::Sampler::new(
                crate::resources::Roots::default(),
                std::time::Instant::now(),
            ),
            writer_boundary,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("skill HTTP client builds"),
        }))
    }

    pub(crate) async fn dispatch(&self, request: RequestFrame) -> ResponseFrame {
        let operation = request.operation();
        if matches!(
            request,
            RequestFrame::PrepareWorkspace(_)
                | RequestFrame::WorkspaceSetup(_)
                | RequestFrame::RemoveWorkspace(_)
        ) {
            if let Err(error) = self.recover_journals().await {
                return error.response(operation);
            }
            if let Err(error) = self.require_credentialed().await {
                return error.response(operation);
            }
            if let RequestFrame::RemoveWorkspace(reference) = &request {
                if let Err(error) = self.ensure_workspace_idle(&reference.name).await {
                    return error.response(operation);
                }
            }
            return match crate::workspace_runtime::dispatch(
                self.config.clone(),
                Arc::clone(&self.operation_gate),
                request,
            )
            .await
            {
                Ok(response) => response,
                Err(error) => error.response(operation),
            };
        }
        let _operation = self.operation_gate.lock().await;
        if let Err(error) = self.authorize_mutation(&request).await {
            return error.response(operation);
        }
        if let Err(error) = self.recover_journals_locked().await {
            return error.response(operation);
        }
        match self.execute(request).await {
            Ok(response) => response,
            Err(error) => error.response(operation),
        }
    }

    async fn execute(&self, request: RequestFrame) -> Result<ResponseFrame> {
        if !matches!(
            request,
            RequestFrame::Install(_) | RequestFrame::Credential(_) | RequestFrame::Resources
        ) {
            self.require_credentialed().await?;
        }
        match request {
            RequestFrame::Install(request) => self.install(request).await,
            RequestFrame::Credential(frame) => self.credential(frame).await,
            RequestFrame::PrepareWorkspace(_)
            | RequestFrame::WorkspaceSetup(_)
            | RequestFrame::RemoveWorkspace(_) => unreachable!("workspace requests detach first"),
            RequestFrame::AddWorktree(request) => {
                self.add_worktree(request.authority_fence, request.request)
                    .await
            }
            RequestFrame::RemoveWorktree(request) => {
                self.remove_worktree(request.authority_fence, request.request)
                    .await
            }
            RequestFrame::MaterializeAgentHome(request) => {
                self.materialize_agent_home(request.authority_fence, request.request)
                    .await
            }
            RequestFrame::RemoveAgentHome(request) => {
                self.remove_agent_home(request.authority_fence, request.request)
                    .await
            }
            RequestFrame::OpenAgent(request) => {
                self.open_agent(request.authority_fence, request.request)
                    .await
            }
            RequestFrame::CloseAgent(request) => {
                self.close_agent(request.authority_fence, request.request)
                    .await
            }
            RequestFrame::CaptureAgent(reference) => {
                safe_component(reference.agent_id.as_str(), "agent id")?;
                let home = self.config.agent_home(reference.agent_id.as_str());
                if !path_is_directory(&home) {
                    return Err(RuntimeError::rejected(
                        "agent_home_not_found",
                        "agent home was not found",
                    ));
                }
                Ok(ResponseFrame::CaptureAgent(crate::conversation::capture(
                    &home,
                )?))
            }
            RequestFrame::Checkpoint(request) => self.checkpoint(&request).await,
            RequestFrame::Restore(request) => self.restore(&request).await,
            RequestFrame::Resources => Ok(ResponseFrame::Resources(
                self.resources.sample(std::time::Instant::now()).into(),
            )),
        }
    }

    async fn recover_journals(&self) -> Result<()> {
        let _operation = self.operation_gate.lock().await;
        self.recover_journals_locked().await
    }

    async fn recover_journals_locked(&self) -> Result<()> {
        let durable = crate::install::recover(&self.config)?;
        let mut phase = self.phase.write().await;
        match (durable, *phase) {
            (RecoveryState::Completed, Phase::Provisioned) => *phase = Phase::Installed,
            (RecoveryState::Completed, Phase::Installed | Phase::Credentialed)
            | (RecoveryState::Absent | RecoveryState::Terminal, Phase::Provisioned) => {}
            (RecoveryState::Absent | RecoveryState::Terminal, _) => {
                return Err(RuntimeError::rejected(
                    "install_state_invalid",
                    "runtime lifecycle disagrees with its durable installation state",
                ))
            }
        }
        drop(phase);
        crate::workspace_runtime::recover_locked(&self.config).await?;
        self.recover_mutations_locked().await
    }

    async fn authorize_mutation(&self, request: &RequestFrame) -> Result<()> {
        use crate::mutation_journal::Operation;

        let fenced = match request {
            RequestFrame::AddWorktree(request) => {
                crate::worktree::validate_add_request(&request.request)?;
                Some((
                    request.authority_fence,
                    Operation::add_worktree(&request.request),
                ))
            }
            RequestFrame::RemoveWorktree(request) => {
                crate::worktree::validate_remove_request(&request.request)?;
                Some((
                    request.authority_fence,
                    Operation::remove_worktree(&request.request),
                ))
            }
            RequestFrame::MaterializeAgentHome(request) => {
                agent_validation::materialize_home(&request.request)?;
                Some((
                    request.authority_fence,
                    Operation::create_agent_home(&request.request),
                ))
            }
            RequestFrame::RemoveAgentHome(request) => {
                agent_validation::remove_home(&request.request)?;
                Some((
                    request.authority_fence,
                    Operation::remove_agent_home(&request.request),
                ))
            }
            RequestFrame::OpenAgent(request) => {
                agent_validation::open(&request.request)?;
                Some((
                    request.authority_fence,
                    Operation::open_agent(&request.request),
                ))
            }
            RequestFrame::CloseAgent(request) => {
                agent_validation::close(&request.request)?;
                Some((
                    request.authority_fence,
                    Operation::close_agent(&request.request),
                ))
            }
            _ => None,
        };
        let Some((authority_fence, operation)) = fenced else {
            return Ok(());
        };
        self.require_credentialed().await?;
        crate::mutation_authority::admit(&self.config, authority_fence, &operation)
    }

    async fn install(&self, request: InstallRequest) -> Result<ResponseFrame> {
        let phase = *self.phase.read().await;
        let receipt = crate::install::run(&self.config, &request).await?;
        if phase == Phase::Provisioned {
            *self.phase.write().await = Phase::Installed;
        }
        Ok(ResponseFrame::Install(receipt))
    }

    async fn credential(&self, frame: layer3_adapter::CredentialFrame) -> Result<ResponseFrame> {
        if frame.operation_id.is_empty() || frame.operation_id.len() > 256 {
            return Err(RuntimeError::rejected(
                "credential_operation_invalid",
                "credential operation id is empty or too large",
            ));
        }
        let phase = *self.phase.read().await;
        match phase {
            Phase::Provisioned => {
                return Err(RuntimeError::rejected(
                    "install_not_completed",
                    "credential cannot be installed before software installation completes",
                ))
            }
            Phase::Installed => {
                let proxies = PreparedProxies::bind(&self.config).await?;
                self.credential.install(&frame).await?;
                proxies.start(self.credential.clone());
                *self.phase.write().await = Phase::Credentialed;
            }
            Phase::Credentialed => self.credential.install(&frame).await?,
        }
        Ok(ResponseFrame::Credential(CredentialReceipt {
            operation_id: frame.operation_id,
            installed: true,
        }))
    }

    async fn checkpoint(
        &self,
        request: &layer3_adapter::CheckpointRequest,
    ) -> Result<ResponseFrame> {
        let roots = self.writer_roots().await;
        let mut fence = WriterFence::quiesce(roots, self.writer_boundary)
            .map_err(|error| RuntimeError::unavailable("writer_fence_failed", error))?;
        let receipt = self.checkpoint.capture(request).await?;
        fence
            .resume()
            .map_err(|error| RuntimeError::unavailable("writer_resume_failed", error))?;
        tracing::info!(writers = fence.writer_count(), "checkpoint writers resumed");
        Ok(ResponseFrame::Checkpoint(receipt))
    }

    async fn restore(&self, request: &layer3_adapter::RestoreRequest) -> Result<ResponseFrame> {
        if !self.agents.read().await.is_empty() {
            return Err(RuntimeError::rejected(
                "restore_agents_open",
                "checkpoint restore requires every agent process to be closed",
            ));
        }
        let mut fence = WriterFence::quiesce([], self.writer_boundary)
            .map_err(|error| RuntimeError::unavailable("writer_fence_failed", error))?;
        self.checkpoint.restore(request).await?;
        fence
            .resume()
            .map_err(|error| RuntimeError::unavailable("writer_resume_failed", error))?;
        Ok(ResponseFrame::Restore(Acknowledgement::default()))
    }

    async fn writer_roots(&self) -> Vec<i32> {
        self.agents
            .read()
            .await
            .values()
            .filter_map(|agent| agent.bridge.writer_session_root())
            .collect()
    }

    async fn ensure_workspace_idle(&self, workspace: &str) -> Result<()> {
        safe_component(workspace, "workspace name")?;
        let root = self.config.home.join(workspace);
        if self
            .agents
            .read()
            .await
            .values()
            .any(|agent| Path::new(&agent.launch.cwd).starts_with(&root))
        {
            return Err(RuntimeError::rejected(
                "workspace_agent_open",
                "workspace still has an open agent",
            ));
        }
        if crate::mutation_journal::active_agents(&self.config)?
            .iter()
            .any(|agent| Path::new(&agent.cwd).starts_with(&root))
        {
            return Err(RuntimeError::rejected(
                "workspace_agent_open",
                "workspace has a durable agent open without a completed close",
            ));
        }
        Ok(())
    }

    async fn ensure_worktree_idle(&self, worktree: &Path) -> Result<()> {
        if self
            .agents
            .read()
            .await
            .values()
            .any(|agent| Path::new(&agent.launch.cwd) == worktree)
        {
            return Err(RuntimeError::rejected(
                "worktree_agent_open",
                "worktree still has an open agent",
            ));
        }
        if crate::mutation_journal::active_agents(&self.config)?
            .iter()
            .any(|agent| Path::new(&agent.cwd) == worktree)
        {
            return Err(RuntimeError::rejected(
                "worktree_agent_open",
                "worktree has a durable agent open without a completed close",
            ));
        }
        Ok(())
    }

    async fn require_phase(&self, expected: Phase, code: &'static str) -> Result<()> {
        if *self.phase.read().await != expected {
            return Err(RuntimeError::rejected(
                code,
                "operation is not valid in the runtime's current lifecycle phase",
            ));
        }
        Ok(())
    }

    async fn require_credentialed(&self) -> Result<()> {
        self.require_phase(Phase::Credentialed, "credential_not_installed")
            .await
    }

    pub(crate) async fn bridge(&self, agent_id: &str) -> Option<Arc<Bridge>> {
        self.agents
            .read()
            .await
            .get(&AgentSlot::new(agent_id))
            .map(|agent| Arc::clone(&agent.bridge))
    }

    pub(crate) fn checkpoint_store(&self) -> &CheckpointStore {
        &self.checkpoint
    }

    pub(crate) async fn phase(&self) -> Phase {
        *self.phase.read().await
    }
}

fn validate_environment(name: &str, value: &str) -> Result<()> {
    if name.is_empty()
        || name.contains('=')
        || name.contains('\0')
        || value.contains('\0')
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(RuntimeError::rejected(
            "agent_environment_invalid",
            "agent environment contains an invalid name or value",
        ));
    }
    Ok(())
}

fn reserved(name: &str) -> bool {
    name == "HOME" || name == "PATH" || name.starts_with("CHRONO_") || name.starts_with("NYXID_")
}

#[cfg(test)]
#[path = "supervisor_authority_tests.rs"]
mod authority_tests;
#[cfg(test)]
#[path = "supervisor_mutation_tests.rs"]
mod mutation_tests;
#[cfg(test)]
#[path = "supervisor_tests.rs"]
mod tests;
