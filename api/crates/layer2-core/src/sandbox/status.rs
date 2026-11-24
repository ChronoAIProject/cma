use serde::{Deserialize, Serialize};

use super::SandboxModelError;

macro_rules! transition_error {
    ($resource:literal, $from:expr, $to:expr) => {
        SandboxModelError::InvalidTransition {
            resource: $resource,
            from: $from.name(),
            to: $to.name(),
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxStatus {
    Provisioning,
    Installing,
    Active,
    Pausing,
    Paused,
    Reviving,
    CredentialRotating,
    Rotating,
    Failed,
    Retiring,
    Retired,
}

impl SandboxStatus {
    pub fn transition(self, next: Self) -> Result<Self, SandboxModelError> {
        let allowed = matches!(
            (self, next),
            (Self::Provisioning, Self::Installing)
                | (Self::Installing, Self::Active)
                | (
                    Self::Active,
                    Self::Pausing
                        | Self::Reviving
                        | Self::CredentialRotating
                        | Self::Rotating
                        | Self::Retiring
                )
                | (Self::CredentialRotating, Self::Reviving)
                | (Self::Provisioning | Self::Installing, Self::Retiring)
                | (Self::Pausing, Self::Paused | Self::Active | Self::Reviving)
                | (Self::Reviving | Self::Rotating, Self::Pausing)
                | (Self::Rotating, Self::Paused)
                | (Self::Installing | Self::Provisioning, Self::Reviving)
                | (
                    Self::Paused,
                    Self::Reviving | Self::Rotating | Self::Retiring
                )
                | (
                    Self::Pausing | Self::Reviving | Self::CredentialRotating | Self::Rotating,
                    Self::Retiring
                )
                | (
                    Self::Reviving | Self::CredentialRotating | Self::Rotating,
                    Self::Active
                )
                | (Self::Retiring, Self::Retired)
        ) || (!matches!(self, Self::Failed | Self::Retired) && next == Self::Failed)
            || (!matches!(self, Self::Retiring | Self::Retired) && next == Self::Retiring);
        allowed
            .then_some(next)
            .ok_or_else(|| transition_error!("sandbox", self, next))
    }

    pub const fn accepts_work(self) -> bool {
        matches!(self, Self::Active)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Installing => "installing",
            Self::Active => "active",
            Self::Pausing => "pausing",
            Self::Paused => "paused",
            Self::Reviving => "reviving",
            Self::CredentialRotating => "credential_rotating",
            Self::Rotating => "rotating",
            Self::Failed => "failed",
            Self::Retiring => "retiring",
            Self::Retired => "retired",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    Preparing,
    Ready,
    Failed,
    Removing,
    Removed,
}

impl WorkspaceStatus {
    pub fn transition(self, next: Self) -> Result<Self, SandboxModelError> {
        let allowed = matches!(
            (self, next),
            (Self::Preparing, Self::Ready | Self::Failed | Self::Removing)
                | (Self::Ready | Self::Failed, Self::Removing)
                | (Self::Removing, Self::Removed | Self::Failed)
        );
        allowed
            .then_some(next)
            .ok_or_else(|| transition_error!("workspace", self, next))
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Removing => "removing",
            Self::Removed => "removed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeStatus {
    Creating,
    Ready,
    Failed,
    Removing,
    Removed,
}

impl WorktreeStatus {
    pub fn transition(self, next: Self) -> Result<Self, SandboxModelError> {
        let allowed = matches!(
            (self, next),
            (Self::Creating, Self::Ready | Self::Failed)
                | (Self::Ready | Self::Failed, Self::Removing)
                | (Self::Removing, Self::Removed)
        );
        allowed
            .then_some(next)
            .ok_or_else(|| transition_error!("worktree", self, next))
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Removing => "removing",
            Self::Removed => "removed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Starting,
    Ready,
    Working,
    WaitingApproval,
    Paused,
    Stopped,
    Deleted,
    Failed,
}

impl AgentStatus {
    pub const fn is_live(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Ready | Self::Working | Self::WaitingApproval | Self::Paused
        )
    }

    pub fn transition(self, next: Self) -> Result<Self, SandboxModelError> {
        let live = self.is_live();
        let allowed = matches!(
            (self, next),
            (Self::Starting, Self::Ready | Self::Failed)
                | (Self::Ready, Self::Working | Self::Paused)
                | (
                    Self::Working,
                    Self::Ready | Self::WaitingApproval | Self::Paused
                )
                | (
                    Self::WaitingApproval,
                    Self::Working | Self::Ready | Self::Paused
                )
                | (Self::Paused, Self::Ready)
                | (Self::Stopped | Self::Failed, Self::Deleted)
        ) || (live && next == Self::Stopped);
        allowed
            .then_some(next)
            .ok_or_else(|| transition_error!("agent", self, next))
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Working => "working",
            Self::WaitingApproval => "waiting_approval",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
            Self::Deleted => "deleted",
            Self::Failed => "failed",
        }
    }
}
