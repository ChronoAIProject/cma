//! Typed RFC 9457 failures for owner-scoped instance routes.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use layer2_core::sandbox::{AgentOperationError, AgentReadError, AgentServiceError};
use layer2_core::RuntimeCapability;

use super::{problem_document, problem_response};

pub(crate) enum InstanceProblem {
    ResourceAccess(layer2_core::resource_access::ResourceAccessError),
    Connection(layer2_core::sandbox::SandboxCredentialPrerequisiteError),
    Forbidden,
    AgentNotFound,
    WorkspaceNotFound,
    AgentProfileNotFound,
    InvalidQuery,
    InvalidRequest,
    WorkspaceNotReady,
    AgentNotReady,
    AgentNotStopped,
    AgentLimitReached { limit: u8, live: u8 },
    MessageQueueFull { limit: usize },
    ApprovalNotPending { approval_id: String },
    CapabilityUnsupported { capability: RuntimeCapability },
    ContentUnsupported,
    SkillVersionUpgradeUnsupported {
        id: String,
        installed: String,
        requested: String,
    },
    OperationInFlight,
    CheckpointInProgress,
    DeliveryAmbiguous,
    ResourcesUnsupported,
    Unavailable,
}

impl InstanceProblem {
    pub(crate) fn from_create(error: AgentServiceError) -> Self {
        match error {
            AgentServiceError::Connection(error) => Self::Connection(error),
            AgentServiceError::Forbidden => Self::Forbidden,
            AgentServiceError::ProfileNotFound => Self::AgentProfileNotFound,
            AgentServiceError::WorkspaceNotFound => Self::WorkspaceNotFound,
            AgentServiceError::NotFound => Self::AgentNotFound,
            AgentServiceError::WorkspaceNotReady | AgentServiceError::SandboxNotReady => {
                Self::WorkspaceNotReady
            }
            AgentServiceError::CapabilityUnsupported { capability } => {
                Self::CapabilityUnsupported { capability }
            }
            AgentServiceError::LimitReached { limit, live } => {
                Self::AgentLimitReached { limit, live }
            }
            AgentServiceError::InvalidRequest(_) => Self::InvalidRequest,
            AgentServiceError::CreationConflict
            | AgentServiceError::StatusConflict { .. }
            | AgentServiceError::WorkConflict { .. }
            | AgentServiceError::StopReasonConflict
            | AgentServiceError::ControlConflict => Self::OperationInFlight,
            AgentServiceError::LogCapacityExceeded
            | AgentServiceError::Corrupt(_)
            | AgentServiceError::Unavailable(_) => Self::Unavailable,
        }
    }

    pub(crate) fn from_stop(error: AgentServiceError) -> Self {
        match error {
            AgentServiceError::Forbidden => Self::Forbidden,
            AgentServiceError::NotFound => Self::AgentNotFound,
            AgentServiceError::StatusConflict { .. }
            | AgentServiceError::WorkConflict { .. }
            | AgentServiceError::StopReasonConflict
            | AgentServiceError::ControlConflict => Self::AgentNotReady,
            other => Self::from_create(other),
        }
    }

    pub(crate) fn from_delete(error: AgentServiceError) -> Self {
        match error {
            AgentServiceError::Forbidden => Self::Forbidden,
            AgentServiceError::NotFound => Self::AgentNotFound,
            AgentServiceError::StatusConflict { .. }
            | AgentServiceError::WorkConflict { .. }
            | AgentServiceError::StopReasonConflict
            | AgentServiceError::ControlConflict => Self::AgentNotStopped,
            other => Self::from_create(other),
        }
    }
}

impl From<AgentReadError> for InstanceProblem {
    fn from(error: AgentReadError) -> Self {
        match error {
            AgentReadError::Access(error) => Self::ResourceAccess(error),
            AgentReadError::Forbidden => Self::Forbidden,
            AgentReadError::NotFound => Self::AgentNotFound,
            AgentReadError::InvalidPage => Self::InvalidQuery,
            AgentReadError::CapabilityUnsupported { capability } => {
                Self::CapabilityUnsupported { capability }
            }
            AgentReadError::ResourcesUnsupported(_) => Self::ResourcesUnsupported,
            AgentReadError::Corrupt(_) | AgentReadError::Unavailable(_) => Self::Unavailable,
        }
    }
}

impl From<AgentOperationError> for InstanceProblem {
    fn from(error: AgentOperationError) -> Self {
        match error {
            AgentOperationError::Connection(error) => Self::Connection(error),
            AgentOperationError::Forbidden => Self::Forbidden,
            AgentOperationError::NotFound => Self::AgentNotFound,
            AgentOperationError::StatusConflict { .. }
            | AgentOperationError::NoActiveTurn
            | AgentOperationError::FirstMessageReserved { .. }
            | AgentOperationError::StaleWorkAuthority => Self::AgentNotReady,
            AgentOperationError::ApprovalNotPending { approval_id } => {
                Self::ApprovalNotPending { approval_id }
            }
            AgentOperationError::CapabilityUnsupported { capability } => {
                Self::CapabilityUnsupported { capability }
            }
            AgentOperationError::ContentUnsupported => Self::ContentUnsupported,
            AgentOperationError::SkillVersionUpgradeUnsupported {
                id,
                installed,
                requested,
            } => Self::SkillVersionUpgradeUnsupported {
                id,
                installed,
                requested,
            },
            AgentOperationError::QueueCapacityExceeded { limit } => {
                Self::MessageQueueFull { limit }
            }
            AgentOperationError::InvalidRequest(_) => Self::InvalidRequest,
            AgentOperationError::DispatchInProgress { .. }
            | AgentOperationError::DispatchRecoveryRequired { .. }
            | AgentOperationError::PreviousTurnConflict
            | AgentOperationError::Conflict => Self::OperationInFlight,
            AgentOperationError::CheckpointInProgress => Self::CheckpointInProgress,
            AgentOperationError::DeliveryAmbiguous { .. } => Self::DeliveryAmbiguous,
            AgentOperationError::LogCapacityExceeded
            | AgentOperationError::Corrupt(_)
            | AgentOperationError::RuntimeUnavailable(_)
            | AgentOperationError::OutcomeUnavailable(_)
            | AgentOperationError::Unavailable(_) => Self::Unavailable,
        }
    }
}

impl IntoResponse for InstanceProblem {
    fn into_response(self) -> Response {
        let (status, slug, detail, code): (_, _, &str, _) = match &self {
            Self::ResourceAccess(error) => return super::resource_access_problem::ResourceProblem(error.clone()).into_response(),
            Self::Connection(error) => return super::instances_problem::HierarchyProblem::connection(error.clone()).into_response(),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "sandbox-use-forbidden",
                "This operation requires authority to use the caller's sandboxes.",
                "forbidden",
            ),
            Self::AgentNotFound => (
                StatusCode::NOT_FOUND,
                "agent-not-found",
                "No agent matches this request.",
                "agent_not_found",
            ),
            Self::WorkspaceNotFound => (
                StatusCode::NOT_FOUND,
                "workspace-not-found",
                "No workspace matches this request.",
                "workspace_not_found",
            ),
            Self::AgentProfileNotFound => (
                StatusCode::NOT_FOUND,
                "agent-profile-not-found",
                "No usable Agent Profile revision matches this request.",
                "agent_profile_not_found",
            ),
            Self::InvalidQuery => (
                StatusCode::BAD_REQUEST,
                "invalid-instance-query",
                "The instance query parameters are not valid.",
                "invalid_instance_query",
            ),
            Self::InvalidRequest => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid-agent-request",
                "The agent request does not satisfy its validation contract.",
                "invalid_agent_request",
            ),
            Self::ContentUnsupported => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "agent-content-unsupported",
                "This Agent interface accepts text content only.",
                "agent_content_unsupported",
            ),
            Self::SkillVersionUpgradeUnsupported {
                id,
                installed,
                requested,
            } => (
                StatusCode::CONFLICT,
                "agent-skill-version-upgrade-unsupported",
                &format!(
                    "Skill {id:?} is already installed at version {installed:?}; changing it to version {requested:?} is unsupported on an existing Agent. Create a replacement Agent with the requested profile."
                ),
                "agent_skill_version_upgrade_unsupported",
            ),
            Self::WorkspaceNotReady => (
                StatusCode::CONFLICT,
                "workspace-not-ready",
                "The workspace is not ready to create an agent.",
                "workspace_not_ready",
            ),
            Self::AgentNotReady => (
                StatusCode::CONFLICT,
                "agent-not-ready",
                "The agent is not ready for this operation.",
                "agent_not_ready",
            ),
            Self::AgentNotStopped => (
                StatusCode::CONFLICT,
                "agent-not-stopped",
                "The agent must be stopped before it can be deleted.",
                "agent_not_stopped",
            ),
            Self::AgentLimitReached { .. } => (
                StatusCode::TOO_MANY_REQUESTS,
                "agent-limit-reached",
                "The workspace has reached its live-agent limit.",
                "agent_limit_reached",
            ),
            Self::MessageQueueFull { .. } => (
                StatusCode::TOO_MANY_REQUESTS,
                "agent-message-queue-full",
                "The paused agent's message queue is full.",
                "agent_message_queue_full",
            ),
            Self::ApprovalNotPending { .. } => (
                StatusCode::CONFLICT,
                "agent-approval-not-pending",
                "The named approval is not pending for this agent.",
                "agent_approval_not_pending",
            ),
            Self::CapabilityUnsupported { .. } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "agent-capability-unsupported",
                "The agent runtime does not support this operation.",
                "agent_capability_unsupported",
            ),
            Self::OperationInFlight => (
                StatusCode::CONFLICT,
                "agent-operation-in-flight",
                "Another durable operation currently owns this agent.",
                "agent_operation_in_flight",
            ),
            Self::CheckpointInProgress => (
                StatusCode::CONFLICT,
                "sandbox-checkpoint-in-progress",
                "The Sandbox is saving a checkpoint. Input was not accepted; retry after the checkpoint completes.",
                "sandbox_checkpoint_in_progress",
            ),
            Self::DeliveryAmbiguous => (
                StatusCode::SERVICE_UNAVAILABLE,
                "agent-delivery-ambiguous",
                "The operation outcome is ambiguous and requires durable reconciliation.",
                "agent_delivery_ambiguous",
            ),
            Self::ResourcesUnsupported => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "agent-resources-unsupported",
                "This agent's sandbox does not report resource readings.",
                "agent_resources_unsupported",
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "instance-service-unavailable",
                "An instance dependency could not complete this request.",
                "instance_service_unavailable",
            ),
        };
        let mut body = problem_document(status, slug, detail, code);
        if let Some(object) = body.as_object_mut() {
            match self {
                Self::AgentLimitReached { limit, live } => {
                    object.insert("limit".into(), limit.into());
                    object.insert("live".into(), live.into());
                }
                Self::MessageQueueFull { limit } => {
                    object.insert("limit".into(), limit.into());
                }
                Self::ApprovalNotPending { approval_id } => {
                    object.insert("request_id".into(), approval_id.into());
                }
                Self::CapabilityUnsupported { capability } => {
                    object.insert("capability".into(), capability_name(capability).into());
                }
                _ => {}
            }
        }
        problem_response(status, body)
    }
}

fn capability_name(capability: RuntimeCapability) -> &'static str {
    match capability {
        RuntimeCapability::Conversation => "conversation",
        RuntimeCapability::Snapshot => "snapshot",
        RuntimeCapability::Steer => "steer",
        RuntimeCapability::Interrupt => "interrupt",
        RuntimeCapability::Usage => "usage",
        RuntimeCapability::Resources => "resources",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn skill_version_upgrade_refusal_has_a_named_conflict_code() {
        let response = InstanceProblem::from(
            AgentOperationError::SkillVersionUpgradeUnsupported {
                id: "research".into(),
                installed: "1.0".into(),
                requested: "1.1".into(),
            },
        )
        .into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "agent_skill_version_upgrade_unsupported");
        assert!(body["detail"]
            .as_str()
            .unwrap()
            .contains("replacement Agent"));
    }
}
