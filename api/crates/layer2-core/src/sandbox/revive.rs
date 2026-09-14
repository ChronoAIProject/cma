//! Durable vocabulary for restart-resumable sandbox revival.

use serde::{Deserialize, Serialize};

use super::TypedFailure;

/// Every event that may request revival of a paused sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxReviveCause {
    AgentMessage,
    WorkspaceCreation,
    AgentCreation,
    ExplicitRequest,
    TriggerLaunch,
    ImageDrift,
    RuntimeLost,
}

/// One durable, idempotent revival step. Home restore makes installation absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxRevivePhase {
    CheckpointVerification,
    RuntimeProvisioning,
    CredentialDelivery,
    HomeRestore,
    ConversationReopen,
    QueuedMessageDelivery,
    PublishActive,
}

pub const SANDBOX_REVIVE_PHASE_ORDER: [SandboxRevivePhase; 7] = [
    SandboxRevivePhase::CheckpointVerification,
    SandboxRevivePhase::RuntimeProvisioning,
    SandboxRevivePhase::CredentialDelivery,
    SandboxRevivePhase::HomeRestore,
    SandboxRevivePhase::ConversationReopen,
    SandboxRevivePhase::QueuedMessageDelivery,
    SandboxRevivePhase::PublishActive,
];

impl SandboxRevivePhase {
    pub const fn first() -> Self {
        Self::CheckpointVerification
    }

    pub const fn next(self) -> Option<Self> {
        match self {
            Self::CheckpointVerification => Some(Self::RuntimeProvisioning),
            Self::RuntimeProvisioning => Some(Self::CredentialDelivery),
            Self::CredentialDelivery => Some(Self::HomeRestore),
            Self::HomeRestore => Some(Self::ConversationReopen),
            Self::ConversationReopen => Some(Self::QueuedMessageDelivery),
            Self::QueuedMessageDelivery => Some(Self::PublishActive),
            Self::PublishActive => None,
        }
    }

    /// Classify a failure at this phase without exposing adapter error strings as codes.
    pub const fn failure_class(self) -> SandboxReviveFailureClass {
        match self {
            Self::RuntimeProvisioning | Self::CredentialDelivery => {
                SandboxReviveFailureClass::ProvisioningFailed
            }
            Self::CheckpointVerification
            | Self::HomeRestore
            | Self::ConversationReopen
            | Self::QueuedMessageDelivery
            | Self::PublishActive => SandboxReviveFailureClass::RestoreFailed,
        }
    }
}

/// Stable failure codes stored on the Sandbox aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxReviveFailureClass {
    RestoreFailed,
    ProvisioningFailed,
    RuntimeLost,
}

impl SandboxReviveFailureClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RestoreFailed => "restore_failed",
            Self::ProvisioningFailed => "provisioning_failed",
            Self::RuntimeLost => "runtime_lost",
        }
    }

    pub fn typed_failure(self, detail: impl Into<String>) -> TypedFailure {
        TypedFailure {
            class: self.as_str().into(),
            detail: detail.into(),
        }
    }
}
