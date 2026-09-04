use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use layer3_adapter::wire::{
    Acknowledgement, AgentOpenReceipt, CredentialReceipt, RequestFrame, ResponseFrame,
    WireOperation,
};
use layer3_adapter::{AgentLaunch, AgentSlot, InstallRequest};
use tokio::sync::{Mutex, RwLock};

use crate::bridge::Bridge;
use crate::checkpoint::CheckpointStore;
use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::paths::{path_is_directory, safe_component};
use crate::upstream::{CredentialHolder, PreparedProxies};
use crate::writer_fence::{SupervisorBoundary, WriterFence};

const MAX_AGENT_SLOTS: usize = 16;

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
    pub(crate) fn new(
        config: RuntimeConfig,
        writer_boundary: SupervisorBoundary,
    ) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&config.home).map_err(|error| {
            RuntimeError::unavailable("home_unavailable", format!("create sandbox home: {error}"))
        })?;
        if !path_is_directory(&config.home) {
            return Err(RuntimeError::rejected(
                "home_invalid",
                "sandbox home is not a directory",
            ));
        }
        crate::install::recover(&config)?;
        let checkpoint = CheckpointStore::new(&config);
        Ok(Arc::new(Self {
            config,
            phase: RwLock::new(Phase::Provisioned),
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
        if !matches!(
            request,
            RequestFrame::Install(_) | RequestFrame::Credential(_) | RequestFrame::Resources
        ) {
            if let Err(error) = crate::workspace_runtime::recover(
                self.config.clone(),
                Arc::clone(&self.operation_gate),
            )
            .await
            {
                return error.response(operation);
            }
        }
        let _operation = self.operation_gate.lock().await;
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
            RequestFrame::AddWorktree(request) => Ok(ResponseFrame::AddWorktree(
                crate::worktree::add(&self.config.home, self.config.allow_local_git, &request)
                    .await?,
            )),
            RequestFrame::RemoveWorktree(reference) => {
                let path = crate::worktree::expected_path(
                    &self.config.home,
                    &reference.workspace,
                    &reference.name,
                );
                self.ensure_worktree_idle(&path).await?;
                crate::worktree::remove(&self.config.home, self.config.allow_local_git, &reference)
                    .await?;
                Ok(ResponseFrame::RemoveWorktree(Acknowledgement::default()))
            }
            RequestFrame::MaterializeAgentHome(plan) => {
                if self.agents.read().await.contains_key(&plan.agent_id) {
                    return Err(RuntimeError::rejected(
                        "agent_running",
                        "agent process is already open",
                    ));
                }
                crate::agent_home::materialize(&self.config, &self.http, &plan).await?;
                Ok(ResponseFrame::MaterializeAgentHome(
                    Acknowledgement::default(),
                ))
            }
            RequestFrame::OpenAgent(launch) => self.open_agent(launch).await,
            RequestFrame::CloseAgent(reference) => self.close_agent(&reference.agent_id).await,
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

    async fn open_agent(&self, launch: AgentLaunch) -> Result<ResponseFrame> {
        self.validate_launch(&launch).await?;
        let mut environment = self.config.process_env.clone();
        for (name, value) in &launch.env {
            validate_environment(name, value)?;
            if reserved(name) && environment.get(name) != Some(value) {
                return Err(RuntimeError::rejected(
                    "agent_environment_conflict",
                    format!("agent environment cannot override reserved variable {name}"),
                ));
            }
            environment.insert(name.clone(), value.clone());
        }
        environment.insert("HOME".into(), launch.home.clone());
        let bridge = Arc::new(Bridge::start(&launch, &environment).map_err(|error| {
            RuntimeError::unavailable("agent_spawn_failed", format!("start agent bridge: {error}"))
        })?);
        self.agents.write().await.insert(
            launch.agent_id.clone(),
            AgentProcess {
                launch: launch.clone(),
                bridge,
            },
        );
        Ok(ResponseFrame::OpenAgent(AgentOpenReceipt {
            agent_id: launch.agent_id,
            conversation_id: None,
        }))
    }

    async fn validate_launch(&self, launch: &AgentLaunch) -> Result<()> {
        safe_component(launch.agent_id.as_str(), "agent id")?;
        let agents = self.agents.read().await;
        if agents.contains_key(&launch.agent_id) {
            return Err(RuntimeError::rejected(
                "agent_already_open",
                "agent slot already has a process",
            ));
        }
        if agents.len() >= MAX_AGENT_SLOTS {
            return Err(RuntimeError::rejected(
                "agent_capacity_reached",
                format!("runtime already has its maximum {MAX_AGENT_SLOTS} agent slots"),
            ));
        }
        drop(agents);
        let expected_home = self.config.agent_home(launch.agent_id.as_str());
        if Path::new(&launch.home) != expected_home || !path_is_directory(&expected_home) {
            return Err(RuntimeError::rejected(
                "agent_home_invalid",
                "agent launch does not name its materialized home",
            ));
        }
        let cwd = Path::new(&launch.cwd);
        if crate::worktree::parse_path(&self.config.home, cwd).is_none()
            || !path_is_directory(cwd)
            || !cwd.join(".git").is_file()
        {
            return Err(RuntimeError::rejected(
                "agent_worktree_invalid",
                "agent launch cwd is not a prepared worktree",
            ));
        }
        if self
            .agents
            .read()
            .await
            .values()
            .any(|agent| agent.launch.cwd == launch.cwd)
        {
            return Err(RuntimeError::rejected(
                "worktree_agent_exists",
                "worktree already has an agent process",
            ));
        }
        if launch.program.is_empty()
            || launch.program.contains('\0')
            || launch.args.iter().any(|argument| argument.contains('\0'))
        {
            return Err(RuntimeError::rejected(
                "agent_program_invalid",
                "agent program or argument contains an invalid value",
            ));
        }
        Ok(())
    }

    async fn close_agent(&self, agent_id: &AgentSlot) -> Result<ResponseFrame> {
        safe_component(agent_id.as_str(), "agent id")?;
        let bridge = self
            .agents
            .read()
            .await
            .get(agent_id)
            .map(|process| Arc::clone(&process.bridge));
        if let Some(bridge) = bridge {
            bridge
                .terminate()
                .await
                .map_err(|error| RuntimeError::unavailable("agent_termination_failed", error))?;
        }
        let process = self.agents.write().await.remove(agent_id);
        let removed_home = crate::agent_home::remove(&self.config, agent_id.as_str())?;
        if process.is_none() && !removed_home {
            return Err(RuntimeError::rejected(
                "agent_not_found",
                "agent process and home were not found",
            ));
        }
        Ok(ResponseFrame::CloseAgent(Acknowledgement::default()))
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

pub(crate) fn operation_for_tag(tag: &str) -> Option<WireOperation> {
    Some(match tag {
        "install" => WireOperation::Install,
        "prepare_workspace" => WireOperation::PrepareWorkspace,
        "workspace_setup" => WireOperation::WorkspaceSetup,
        "remove_workspace" => WireOperation::RemoveWorkspace,
        "add_worktree" => WireOperation::AddWorktree,
        "remove_worktree" => WireOperation::RemoveWorktree,
        "materialize_agent_home" => WireOperation::MaterializeAgentHome,
        "open_agent" => WireOperation::OpenAgent,
        "close_agent" => WireOperation::CloseAgent,
        "capture_agent" => WireOperation::CaptureAgent,
        "checkpoint" => WireOperation::Checkpoint,
        "restore" => WireOperation::Restore,
        "credential" => WireOperation::Credential,
        "resources" => WireOperation::Resources,
        _ => return None,
    })
}

#[cfg(test)]
#[path = "supervisor_tests.rs"]
mod tests;
