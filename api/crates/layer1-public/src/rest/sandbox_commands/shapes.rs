use layer2_core::sandbox::control::*;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SandboxCommandKindView {
    Pause,
    Revive,
    Retire,
}
impl From<SandboxCommandKindView> for SandboxControl {
    fn from(value: SandboxCommandKindView) -> Self {
        match value {
            SandboxCommandKindView::Pause => Self::Pause,
            SandboxCommandKindView::Revive => Self::Revive,
            SandboxCommandKindView::Retire => Self::Retire,
        }
    }
}
impl From<SandboxControl> for SandboxCommandKindView {
    fn from(value: SandboxControl) -> Self {
        match value {
            SandboxControl::Pause => Self::Pause,
            SandboxControl::Revive => Self::Revive,
            SandboxControl::Retire => Self::Retire,
        }
    }
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SandboxCommandRequest {
    pub command: SandboxCommandKindView,
    /// Revision from a current authorized Sandbox detail read.
    #[schema(minimum = 1)]
    pub expected_revision: u64,
}
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SandboxCommandDispositionView {
    Scheduled,
    AlreadyScheduled,
    AlreadySatisfied,
}
impl From<SandboxControlDisposition> for SandboxCommandDispositionView {
    fn from(value: SandboxControlDisposition) -> Self {
        match value {
            SandboxControlDisposition::Scheduled => Self::Scheduled,
            SandboxControlDisposition::AlreadyScheduled => Self::AlreadyScheduled,
            SandboxControlDisposition::AlreadySatisfied => Self::AlreadySatisfied,
        }
    }
}
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SandboxCommandFailureView {
    AuthorityRevoked,
    QueueExpired,
    TargetChanged,
    NotEligible,
    ConnectionsRequired,
    CredentialSetupRequired,
}
impl From<SandboxControlFailure> for SandboxCommandFailureView {
    fn from(value: SandboxControlFailure) -> Self {
        match value {
            SandboxControlFailure::AuthorityRevoked => Self::AuthorityRevoked,
            SandboxControlFailure::QueueExpired => Self::QueueExpired,
            SandboxControlFailure::TargetChanged => Self::TargetChanged,
            SandboxControlFailure::NotEligible => Self::NotEligible,
            SandboxControlFailure::ConnectionsRequired => Self::ConnectionsRequired,
            SandboxControlFailure::CredentialSetupRequired => Self::CredentialSetupRequired,
        }
    }
}
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum SandboxCommandStateView {
    Queued,
    /// The transition is committed; current Sandbox status reports runtime progress.
    Applied {
        at: String,
        sandbox_revision: u64,
        disposition: SandboxCommandDispositionView,
    },
    Rejected {
        at: String,
        reason: SandboxCommandFailureView,
    },
}
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SandboxCommandLifecycleView {
    pub result: super::super::instances_openapi_shapes::SandboxLifecycleResultView,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SandboxCommandReceiptView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<SandboxCommandLifecycleView>,
    pub command_id: String,
    pub sandbox_id: String,
    pub command: SandboxCommandKindView,
    pub expected_revision: u64,
    pub admitted_at: String,
    pub queue_deadline: String,
    pub revision: u64,
    pub state: SandboxCommandStateView,
}
impl From<SandboxControlRecord> for SandboxCommandReceiptView {
    fn from(record: SandboxControlRecord) -> Self {
        Self {
            lifecycle: layer2_core::sandbox::SandboxLifecycleResult::queued_control(
                &record,
                std::time::SystemTime::now(),
            )
            .map(|result| SandboxCommandLifecycleView {
                result: result.into(),
            }),
            command_id: record.envelope.command_id.to_string(),
            sandbox_id: record.envelope.sandbox_id.to_string(),
            command: record.envelope.command.into(),
            expected_revision: record.envelope.expected_revision,
            admitted_at: record.admitted_at,
            queue_deadline: record.queue_deadline,
            revision: record.revision,
            state: match record.status {
                SandboxControlStatus::Queued => SandboxCommandStateView::Queued,
                SandboxControlStatus::Applied {
                    at,
                    sandbox_revision,
                    disposition,
                } => SandboxCommandStateView::Applied {
                    at,
                    sandbox_revision,
                    disposition: disposition.into(),
                },
                SandboxControlStatus::Rejected { at, reason } => {
                    SandboxCommandStateView::Rejected {
                        at,
                        reason: reason.into(),
                    }
                }
            },
        }
    }
}
