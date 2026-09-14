use serde::{Deserialize, Serialize};

use super::SandboxModelError;

/// One durable sandbox lifecycle step. Each worker claim executes at most one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "index", rename_all = "snake_case")]
pub enum SandboxWorkStep {
    ProvisionRuntime,
    RunInstall,
    MintInitialCredential,
    DeliverInitialCredential,
    CreateProfileWorkspace(u16),
    Activate,
    WaitForCredentialRotation,
    MintRotatedCredential,
    DeliverRotatedCredential,
    RevokeRotatedCredential,
    FinishCredentialRotation,
    RunLifecycleOperation,
    StopAgents,
    RevokeRetirementCredentials,
    TeardownRuntime,
    MarkRetired,
    RetryRetiredRevocations,
    WaitForPurge,
    Purge,
    None,
}

impl SandboxWorkStep {
    pub const fn is_pending(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Finite ownership of one lifecycle step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxWorkLease {
    pub holder: String,
    pub fence: u64,
    pub expires_at: String,
}

/// Durable scheduling and fencing state for lifecycle work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxWorkState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<super::ProvisioningExecution>,
    pub step: SandboxWorkStep,
    pub ready_at: String,
    pub next_fence: u64,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<SandboxWorkLease>,
}

impl SandboxWorkState {
    pub fn new(step: SandboxWorkStep, ready_at: String) -> Self {
        Self {
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
            return Err(SandboxModelError::invalid_value("sandbox.work.lease"));
        }
        let fence = self.next_fence;
        self.next_fence = fence
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("sandbox.work.next_fence"))?;
        self.lease = Some(SandboxWorkLease {
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

    pub fn schedule(&mut self, step: SandboxWorkStep, ready_at: String) {
        self.step = step;
        self.ready_at = ready_at;
        self.attempts = 0;
        self.lease = None;
    }

    pub fn advance(
        &mut self,
        holder: &str,
        fence: u64,
        next: SandboxWorkStep,
        ready_at: String,
    ) -> Result<(), SandboxModelError> {
        if !self.owns(holder, fence) {
            return Err(SandboxModelError::invalid_value("sandbox.work.fence"));
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
            return Err(SandboxModelError::invalid_value("sandbox.work.fence"));
        }
        self.attempts = self
            .attempts
            .checked_add(1)
            .ok_or_else(|| SandboxModelError::invalid_value("sandbox.work.attempts"))?;
        self.ready_at = ready_at;
        self.lease = None;
        Ok(())
    }
}
