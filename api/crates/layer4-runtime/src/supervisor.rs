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
#[path = "supervisor_agent_process.rs"]
mod agent_process;
#[path = "supervisor_agent_recovery.rs"]
mod agent_recovery;
#[path = "supervisor_agent_resources.rs"]
mod agent_resources;
#[path = "supervisor_agent_validation.rs"]
mod agent_validation;
#[path = "supervisor_attachment.rs"]
mod attachment;
#[path = "supervisor_browser.rs"]
mod browser;
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
#[path = "supervisor_startup.rs"]
mod startup;
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
    pub(crate) desktop_ownership: Arc<crate::desktop_ownership::DesktopReceiver>,
    pub(crate) manager_ownership: crate::manager_ownership::ManagerOwnership,
    config: RuntimeConfig,
    mutations: crate::mutation_store::Store,
    boot_id: String,
    phase: RwLock<Phase>,
    // Only a fresh runtime may prepare unowned work. Recovered homes require
    // renewed owner credentials even when their installation receipt survives.
    allow_unowned_preparation: bool,
    restore_bootstrap: RwLock<Option<RestoreBootstrapReceipt>>,
    operation_gate: Arc<Mutex<()>>,
    credential: CredentialHolder,
    agents: RwLock<BTreeMap<AgentSlot, AgentProcess>>,
    browsers: Mutex<BTreeMap<AgentSlot, crate::browser::Browser>>,
    checkpoint: CheckpointStore,
    held_checkpoint: Arc<Mutex<Option<HeldCheckpoint>>>,
    resources: crate::resources::Sampler,
    writer_boundary: SupervisorBoundary,
    http: reqwest::Client,
    // Dropping the supervisor terminates the isolated desktop session.
    desktop_process: Option<crate::desktop_process::DesktopSession>,
}

impl Supervisor {
    pub(crate) fn stale_turn_seal_enabled(&self) -> bool {
        self.config
            .process_env
            .get(crate::bridge::STALE_TURN_SEAL_FLAG)
            .is_some_and(|value| value == "1")
    }

    pub(crate) async fn stale_turn_bridge(
        &self,
        target: &layer3_adapter::stale_turn::StaleTurnTarget,
    ) -> std::result::Result<Arc<Bridge>, layer3_adapter::stale_turn::StaleTurnHold> {
        use layer3_adapter::stale_turn::StaleTurnHold;
        if !self.stale_turn_seal_enabled() {
            return Err(StaleTurnHold::Unsupported);
        }
        if !target.is_valid()
            || target.sandbox_id != self.config.sandbox_id
            || target.runtime_boot != self.boot_id
            || target.runtime_generation != self.config.generation
        {
            return Err(StaleTurnHold::Unconfirmed);
        }
        self.require_credentialed()
            .await
            .map_err(|_| StaleTurnHold::Unconfirmed)?;
        self.bridge(&target.slot.0)
            .await
            .ok_or(StaleTurnHold::Unconfirmed)
    }
}

impl Supervisor {
    pub(crate) fn desktop_generation(
        &self,
    ) -> Option<
        std::result::Result<
            crate::desktop_process::DesktopGeneration,
            crate::desktop_process::DesktopUnavailable,
        >,
    > {
        self.desktop_process
            .as_ref()
            .map(crate::desktop_process::DesktopSession::generation)
    }

    pub(crate) async fn shutdown_desktop(&self) {
        if let Some(desktop) = &self.desktop_process {
            desktop.shutdown().await;
        }
    }

    pub(crate) async fn dispatch(&self, request: RequestFrame) -> ResponseFrame {
        let operation = request.operation();
        if let Err(error) = self.mutations.verify() {
            return error.response(operation);
        }
        if let RequestFrame::BrowserControl(request) = request {
            return match self.browser_control(request).await {
                Ok(response) => response,
                Err(error) => error.response(operation),
            };
        }
        if let RequestFrame::Browser(request) = request {
            return match self.browser(request).await {
                Ok(response) => response,
                Err(error) => error.response(operation),
            };
        }
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
            if let Err(error) = self.mutations.require_serving().await {
                return error.response(operation);
            }
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
        if let Err(error) = self.mutations.require_serving().await {
            return error.response(operation);
        }
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
        if let RequestFrame::ProveAgentProcessClosed(proof) = request {
            return match self.prove_agent_process_closed(proof).await {
                Ok(proof) => ResponseFrame::ProveAgentProcessClosed(proof),
                Err(error) => error.response(operation),
            };
        }
        if let RequestFrame::FenceAgentRestart(request) = request {
            return match self.fence_agent_restart(request).await {
                Ok(receipt) => ResponseFrame::FenceAgentRestart(receipt),
                Err(error) => error.response(operation),
            };
        }
        let _restart_writer = match &request {
            RequestFrame::OpenAgent(open) if open.restart_runtime.is_some() => {
                match self.removal_writer_guard() {
                    Ok(writer) => writer,
                    Err(error) => return error.response(operation),
                }
            }
            _ => None,
        };
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
        if frame.github_route.is_some() && !self.supports_exact_github_credential_route() {
            return Err(RuntimeError::rejected(
                "credential_github_route_unavailable",
                "runtime does not have a GitHub credential proxy",
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
#[path = "supervisor_filesystem_http_tests.rs"]
mod filesystem_http_tests;
#[cfg(test)]
#[path = "supervisor_indexed_tests.rs"]
mod indexed_tests;

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

#[cfg(test)]
#[path = "supervisor_ledger_status_tests.rs"]
mod ledger_status_tests;
