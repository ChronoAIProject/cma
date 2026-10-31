use std::collections::BTreeMap;
use std::sync::Arc;

use layer3_adapter::wire::{CredentialReceipt, RequestFrame, ResponseFrame};
use layer3_adapter::{
    AgentLaunch, AgentSlot, CheckpointHoldReceipt, CheckpointHoldRequest, InstallRequest,
    RestoreBootstrapReceipt, RestoreBootstrapRequest,
};
use tokio::sync::{Mutex, RwLock};

use crate::bridge::Bridge;
use crate::checkpoint::CheckpointStore;
use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::install_state::RecoveryState;
use crate::paths::path_is_directory;
use crate::upstream::{CredentialHolder, PreparedProxies};
use crate::writer_fence::{SupervisorBoundary, WriterFence};

const MAX_AGENT_SLOTS: usize = 16;

#[path = "supervisor_agent.rs"]
mod agent;
#[path = "supervisor_agent_recovery.rs"]
mod agent_recovery;
#[path = "supervisor_agent_resources.rs"]
mod agent_resources;
#[path = "supervisor_agent_validation.rs"]
mod agent_validation;
#[path = "supervisor_attachment.rs"]
mod attachment;
#[path = "supervisor_checkpoint.rs"]
mod checkpoint_lifecycle;
#[path = "supervisor_execute.rs"]
mod execute;
#[path = "supervisor_guards.rs"]
mod guards;
#[path = "supervisor_mutations.rs"]
mod mutations;
#[path = "supervisor_recovery.rs"]
mod recovery;
#[path = "supervisor_status.rs"]
mod status;
#[path = "supervisor_workspace_reconciliation.rs"]
mod workspace_reconciliation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Provisioned,
    Installed,
    RestorePending,
    RestoreCredentialed,
    Credentialed,
}

struct AgentProcess {
    launch: AgentLaunch,
    bridge: Arc<Bridge>,
}

pub(crate) struct HeldCheckpoint {
    request: CheckpointHoldRequest,
    receipt: CheckpointHoldReceipt,
    fence: WriterFence,
}

pub(crate) struct Supervisor {
    config: RuntimeConfig,
    boot_id: String,
    phase: RwLock<Phase>,
    // Only a fresh runtime may prepare unowned work. Recovered homes require
    // renewed owner credentials even when their installation receipt survives.
    allow_unowned_preparation: bool,
    restore_bootstrap: RwLock<Option<RestoreBootstrapReceipt>>,
    operation_gate: Arc<Mutex<()>>,
    credential: CredentialHolder,
    agents: RwLock<BTreeMap<AgentSlot, AgentProcess>>,
    checkpoint: CheckpointStore,
    held_checkpoint: Arc<Mutex<Option<HeldCheckpoint>>>,
    resources: crate::resources::Sampler,
    writer_boundary: SupervisorBoundary,
    http: reqwest::Client,
    // Dropping the supervisor terminates the isolated desktop session.
    desktop_process: Option<crate::desktop_process::DesktopProcess>,
}

impl Supervisor {
    pub(crate) fn desktop_websocket_token(&self) -> Option<&str> {
        self.desktop_process
            .as_ref()
            .map(crate::desktop_process::DesktopProcess::websocket_token)
    }

    pub(crate) async fn start(
        config: RuntimeConfig,
        writer_boundary: SupervisorBoundary,
    ) -> Result<Arc<Self>> {
        let supervisor = Self::new(config, writer_boundary)?;
        if supervisor.config.machine_included {
            let ready = async {
                loop {
                    if tokio::net::TcpStream::connect("127.0.0.1:6080")
                        .await
                        .is_ok()
                    {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            };
            tokio::time::timeout(std::time::Duration::from_secs(30), ready)
                .await
                .map_err(|_| {
                    RuntimeError::unavailable(
                        "desktop_start_failed",
                        "desktop did not become ready",
                    )
                })?;
        }
        supervisor.recover_journals().await?;
        Ok(supervisor)
    }

    pub(crate) fn new(
        config: RuntimeConfig,
        writer_boundary: SupervisorBoundary,
    ) -> Result<Arc<Self>> {
        if config.home_identity.is_none() {
            crate::fs_durability::create_dir_all(&config.home).map_err(|error| {
                RuntimeError::unavailable(
                    "home_unavailable",
                    format!("create sandbox home: {error}"),
                )
            })?;
        }
        if !path_is_directory(&config.home) {
            return Err(RuntimeError::rejected(
                "home_invalid",
                "sandbox home is not a directory",
            ));
        }
        let boot_id = uuid::Uuid::new_v4().simple().to_string();
        let _removal_writer = crate::workspace_removal_writer::claim(&config, &boot_id)?;
        let home = crate::persistent_home::initialize(&config)?;
        crate::workspace_removal_writer::publish_claim(&config, &boot_id)?;
        let install_state = crate::install::recover(&config)?;
        let restored = home
            .as_ref()
            .map(|home| home.restore_completed(&install_state))
            .transpose()?
            .unwrap_or(false);
        let (phase, restore_bootstrap) = match install_state {
            RecoveryState::Completed => (Phase::Installed, None),
            RecoveryState::Inherited(receipt) => (
                if restored {
                    Phase::Installed
                } else {
                    Phase::RestorePending
                },
                Some(receipt),
            ),
            RecoveryState::Absent | RecoveryState::Terminal => (Phase::Provisioned, None),
        };
        let checkpoint = CheckpointStore::new(&config);
        let desktop_process = if config.machine_included {
            Some(crate::desktop_process::DesktopProcess::start(&config)?)
        } else {
            None
        };
        Ok(Arc::new(Self {
            config,
            boot_id,
            phase: RwLock::new(phase),
            allow_unowned_preparation: phase == Phase::Provisioned,
            restore_bootstrap: RwLock::new(restore_bootstrap),
            operation_gate: Arc::new(Mutex::new(())),
            credential: CredentialHolder::new(),
            agents: RwLock::new(BTreeMap::new()),
            checkpoint,
            held_checkpoint: Arc::new(Mutex::new(None)),
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
            desktop_process,
        }))
    }

    pub(crate) async fn dispatch(&self, request: RequestFrame) -> ResponseFrame {
        let operation = request.operation();
        // Observation must not repair journals or wait for archive capture/export.
        if let RequestFrame::ToolProxyStatus(request) = request {
            return match self.tool_proxy_status(request).await {
                Ok(status) => ResponseFrame::ToolProxyStatus(status),
                Err(error) => error.response(operation),
            };
        }
        if let RequestFrame::RuntimeStatus(request) = request {
            return match self.runtime_status(request).await {
                Ok(status) => ResponseFrame::RuntimeStatus(status),
                Err(error) => error.response(operation),
            };
        }
        if let RequestFrame::WorkspaceRemovalSupport(request) = request {
            return match self.workspace_removal_support(request).await {
                Ok(support) => ResponseFrame::WorkspaceRemovalSupport(support),
                Err(error) => error.response(operation),
            };
        }
        if let RequestFrame::ReconcileWorkspaceRemoval(request) = request {
            return match self.reconcile_workspace_removal(request).await {
                Ok(observation) => ResponseFrame::ReconcileWorkspaceRemoval(observation),
                Err(error) => error.response(operation),
            };
        }
        if matches!(
            request,
            RequestFrame::RecoverWorkspacePrepare(_)
                | RequestFrame::PrepareWorkspace(_)
                | RequestFrame::WorkspaceSetup(_)
                | RequestFrame::RemoveWorkspace(_)
        ) {
            if self.held_checkpoint.lock().await.is_some() {
                return writers_held_error().response(operation);
            }
            // Keep recovery, the idle check and the detached mutation in one
            // critical section so an Agent cannot open between them.
            let operation_guard = Arc::clone(&self.operation_gate).lock_owned().await;
            if self.held_checkpoint.lock().await.is_some() {
                return writers_held_error().response(operation);
            }
            if let Err(error) = self.recover_journals_locked().await {
                return error.response(operation);
            }
            if let Err(error) = self.authorize_preparation(&request).await {
                return error.response(operation);
            }
            let removal_writer = match self.removal_writer_guard() {
                Ok(guard) => guard,
                Err(error) => return error.response(operation),
            };
            if let RequestFrame::RemoveWorkspace(reference) = &request {
                if let Err(error) = self.ensure_workspace_idle(&reference.name).await {
                    return error.response(operation);
                }
            }
            if let RequestFrame::RecoverWorkspacePrepare(frame) = &request {
                if let Err(error) = self.authorize_workspace_recovery(frame).await {
                    return error.response(operation);
                }
            }
            return match crate::workspace_runtime::dispatch(
                self.config.clone(),
                operation_guard,
                Arc::clone(&self.held_checkpoint),
                removal_writer,
                request,
            )
            .await
            {
                Ok(response) => response,
                Err(error) => error.response(operation),
            };
        }
        let _operation = self.operation_gate.lock().await;
        if let Err(error) = crate::persistent_home::verify(&self.config) {
            return error.response(operation);
        }
        let writers_held = self.held_checkpoint.lock().await.is_some();
        if writers_held
            && !matches!(
                &request,
                RequestFrame::CheckpointForTeardown(_)
                    | RequestFrame::ResumeCheckpointWriters(_)
                    | RequestFrame::CaptureAgent(_)
                    | RequestFrame::Resources
            )
        {
            return writers_held_error().response(operation);
        }
        if let Err(error) = self.authorize_mutation(&request).await {
            return error.response(operation);
        }
        if !writers_held && !matches!(&request, RequestFrame::ResumeCheckpointWriters(_)) {
            if let Err(error) = self.recover_journals_locked().await {
                return error.response(operation);
            }
        }
        match self.execute(request).await {
            Ok(response) => response,
            Err(error) => error.response(operation),
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

    async fn bootstrap_restore(&self, request: RestoreBootstrapRequest) -> Result<ResponseFrame> {
        let receipt = crate::restore_bootstrap::admit(&self.config, &request)?;
        let mut phase = self.phase.write().await;
        let mut bootstrap = self.restore_bootstrap.write().await;
        if bootstrap
            .as_ref()
            .is_some_and(|existing| existing != &receipt)
        {
            return Err(RuntimeError::rejected(
                "restore_bootstrap_conflict",
                "runtime lifecycle carries another restore bootstrap identity",
            ));
        }
        match *phase {
            Phase::Provisioned => *phase = Phase::RestorePending,
            Phase::RestorePending | Phase::RestoreCredentialed | Phase::Credentialed => {}
            Phase::Installed
                if self.config.home_identity.is_some() && bootstrap.as_ref() == Some(&receipt) => {}
            Phase::Installed => {
                return Err(RuntimeError::rejected(
                    "install_mode_conflict",
                    "ordinary installation already completed for this runtime",
                ))
            }
        }
        *bootstrap = Some(receipt.clone());
        Ok(ResponseFrame::BootstrapRestore(receipt))
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
            Phase::RestorePending => {
                let proxies = PreparedProxies::bind(&self.config).await?;
                self.credential.install(&frame).await?;
                proxies.start(self.credential.clone());
                *self.phase.write().await = Phase::RestoreCredentialed;
            }
            Phase::RestoreCredentialed | Phase::Credentialed => {
                self.credential.install(&frame).await?
            }
        }
        Ok(ResponseFrame::Credential(CredentialReceipt {
            operation_id: frame.operation_id,
            installed: true,
        }))
    }

    pub(crate) async fn bridge(&self, agent_id: &str) -> Option<Arc<Bridge>> {
        self.agents
            .read()
            .await
            .get(&AgentSlot::new(agent_id))
            .map(|agent| Arc::clone(&agent.bridge))
    }

    pub(crate) async fn export_history(&self, agent_id: &str) -> Result<Vec<u8>> {
        crate::history::export(&self.config, agent_id).await
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

fn writers_held_error() -> RuntimeError {
    RuntimeError::rejected(
        "checkpoint_writers_held",
        "operation is not permitted while checkpoint writers are held",
    )
}

#[cfg(test)]
#[path = "supervisor_authority_tests.rs"]
mod authority_tests;
#[cfg(test)]
#[path = "supervisor_checkpoint_http_tests.rs"]
mod checkpoint_http_tests;
#[cfg(test)]
#[path = "supervisor_mutation_tests.rs"]
mod mutation_tests;
#[cfg(test)]
#[path = "supervisor_persistent_home_tests.rs"]
mod persistent_home_tests;
#[cfg(test)]
#[path = "supervisor_restore_tests.rs"]
mod restore_tests;
#[cfg(test)]
#[path = "supervisor_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "supervisor_tool_proxy_tests.rs"]
mod tool_proxy_tests;

#[cfg(test)]
#[path = "supervisor_hot_rotation_tests.rs"]
mod hot_rotation_tests;
