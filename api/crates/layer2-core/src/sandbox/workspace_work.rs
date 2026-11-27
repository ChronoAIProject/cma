use serde::{Deserialize, Serialize};

use super::SandboxModelError;

/// One durable workspace lifecycle step. A claim executes at most one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceWorkStep {
    Prepare,
    RecoverPrepare,
    Setup,
    Remove,
    None,
}

impl WorkspaceWorkStep {
    pub const fn is_pending(self) -> bool {
        !matches!(self, Self::None)
    }

    pub const fn operation_name(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::RecoverPrepare => "recover-prepare",
            Self::Setup => "setup",
            Self::Remove => "remove",
            Self::None => "none",
        }
    }
}

/// Finite ownership of one workspace lifecycle step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceWorkLease {
    pub holder: String,
    pub fence: u64,
    pub expires_at: String,
}

/// Durable scheduling and fencing state for workspace work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceWorkState {
    /// Retained after switch-off; older strict readers cannot interpret this work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_protocol: Option<super::store::WorkAuthorityProtocol>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepare_recovery: Option<Box<super::WorkspacePrepareRecovery>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepare_observations: Option<Box<super::WorkspacePrepareObservations>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<super::ProvisioningExecution>,
    pub step: WorkspaceWorkStep,
    pub ready_at: String,
    pub next_fence: u64,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<WorkspaceWorkLease>,
}

impl WorkspaceWorkState {
    pub fn new(step: WorkspaceWorkStep, ready_at: String) -> Self {
        Self {
            authority_protocol: None,
            prepare_recovery: None,
            prepare_observations: None,
            execution: None,
            step,
            ready_at,
            next_fence: 1,
            attempts: 0,
            lease: None,
        }
    }

    pub fn is_due(&self, now: &str) -> bool {
        self.step.is_pending()
            && self.ready_at.as_str() <= now
            && self
                .lease
                .as_ref()
                .is_none_or(|lease| lease.expires_at.as_str() <= now)
    }

    pub fn claim(
        &mut self,
        holder: String,
        now: &str,
        expires_at: String,
    ) -> Result<u64, SandboxModelError> {
        if holder.trim().is_empty() || expires_at.as_str() <= now || !self.is_due(now) {
            return Err(SandboxModelError::invalid_value("workspace.work.lease"));
        }
        let fence = self.next_fence;
        self.next_fence = fence
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("workspace.work.next_fence"))?;
        self.lease = Some(WorkspaceWorkLease {
            holder,
            fence,
            expires_at,
        });
        Ok(fence)
    }

    pub fn owns(&self, holder: &str, fence: u64) -> bool {
        self.lease
            .as_ref()
            .is_some_and(|lease| lease.holder == holder && lease.fence == fence)
    }

    pub fn schedule(&mut self, step: WorkspaceWorkStep, ready_at: String) {
        self.step = step;
        self.ready_at = ready_at;
        self.attempts = 0;
        self.lease = None;
    }

    pub fn advance(
        &mut self,
        holder: &str,
        fence: u64,
        next: WorkspaceWorkStep,
        ready_at: String,
    ) -> Result<(), SandboxModelError> {
        if !self.owns(holder, fence) {
            return Err(SandboxModelError::invalid_value("workspace.work.fence"));
        }
        self.schedule(next, ready_at);
        Ok(())
    }

    pub fn retry(
        &mut self,
        holder: &str,
        fence: u64,
        ready_at: String,
    ) -> Result<(), SandboxModelError> {
        if !self.owns(holder, fence) {
            return Err(SandboxModelError::invalid_value("workspace.work.fence"));
        }
        self.attempts = self
            .attempts
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("workspace.work.attempts"))?;
        self.ready_at = ready_at;
        self.lease = None;
        Ok(())
    }
}
