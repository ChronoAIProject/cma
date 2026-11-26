use super::*;

pub(super) fn creation_key(
    origin: &AgentOrigin,
    agent_id: &AgentId,
) -> Result<String, AgentServiceError> {
    match origin {
        AgentOrigin::Manual => Ok(format!("manual:{agent_id}")),
        AgentOrigin::Lifecycle { operation_id } => {
            if !valid_key_part(operation_id) {
                return Err(AgentServiceError::InvalidRequest(
                    "lifecycle origin is invalid".into(),
                ));
            }
            content_digest(origin)
                .map(|digest| format!("lifecycle:{digest}"))
                .map_err(|error| AgentServiceError::InvalidRequest(error.to_string()))
        }
        AgentOrigin::WorkspaceProfile { .. } => Err(AgentServiceError::InvalidRequest(
            "workspace profile origin requires automatic admission".into(),
        )),
        AgentOrigin::Trigger {
            trigger_id,
            launch_id,
        } => {
            if !valid_key_part(trigger_id) || !valid_key_part(launch_id) {
                return Err(AgentServiceError::InvalidRequest(
                    "trigger origin is invalid".into(),
                ));
            }
            content_digest(origin)
                .map(|digest| format!("trigger:{digest}"))
                .map_err(|error| AgentServiceError::InvalidRequest(error.to_string()))
        }
    }
}

pub(super) fn valid_key_part(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_TRIGGER_KEY_BYTES
        && !value.chars().any(char::is_control)
}

pub(super) fn validate_title(title: Option<&str>) -> Result<(), AgentServiceError> {
    if title.is_some_and(|value| {
        value.trim().is_empty()
            || value.chars().count() > MAX_TITLE_SCALARS
            || value.chars().any(char::is_control)
    }) {
        return Err(AgentServiceError::InvalidRequest("title is invalid".into()));
    }
    Ok(())
}

pub(super) fn require(caller: &Caller) -> Result<(), AgentServiceError> {
    caller
        .authorization
        .allows(Capability::UseOwnSandboxes)
        .then_some(())
        .ok_or(AgentServiceError::Forbidden)
}

pub(super) fn require_launch(
    caller: &Caller,
    authority: Capability,
) -> Result<(), AgentServiceError> {
    (matches!(
        authority,
        Capability::LaunchTriggers | Capability::CreateSandbox
    ) && caller.authorization.allows(authority))
    .then_some(())
    .ok_or(AgentServiceError::Forbidden)
}

pub(super) fn profile_error(error: ProfileStoreError) -> AgentServiceError {
    match error {
        ProfileStoreError::NotFound => AgentServiceError::ProfileNotFound,
        ProfileStoreError::Corrupt(detail) => AgentServiceError::Corrupt(detail),
        other => AgentServiceError::Unavailable(other.to_string()),
    }
}

pub(super) fn store_error(error: SandboxStoreError) -> AgentServiceError {
    match error {
        SandboxStoreError::NotFound => AgentServiceError::NotFound,
        SandboxStoreError::WorkspaceNotReady => AgentServiceError::WorkspaceNotReady,
        SandboxStoreError::SandboxNotReady => AgentServiceError::SandboxNotReady,
        SandboxStoreError::AgentLimitReached { limit, live } => {
            AgentServiceError::LimitReached { limit, live }
        }
        SandboxStoreError::AgentCreationConflict => AgentServiceError::CreationConflict,
        SandboxStoreError::AgentLogCapacityExceeded => AgentServiceError::LogCapacityExceeded,
        SandboxStoreError::Budget(reason) => AgentServiceError::Budget(reason),
        SandboxStoreError::AdmissionPaused => AgentServiceError::AdmissionPaused,
        SandboxStoreError::Corrupt(detail) => AgentServiceError::Corrupt(detail),
        SandboxStoreError::InvalidTransaction(detail) => AgentServiceError::Corrupt(detail.into()),
        other => AgentServiceError::Unavailable(other.to_string()),
    }
}

pub(super) fn control_error(error: AgentControlStoreError) -> AgentServiceError {
    match error {
        AgentControlStoreError::NotFound => AgentServiceError::NotFound,
        AgentControlStoreError::StatusConflict { status } => {
            AgentServiceError::StatusConflict { status }
        }
        AgentControlStoreError::WorkConflict { step } => AgentServiceError::WorkConflict { step },
        AgentControlStoreError::StopReasonConflict => AgentServiceError::StopReasonConflict,
        AgentControlStoreError::Conflict | AgentControlStoreError::RestartEvidenceRequired => {
            AgentServiceError::ControlConflict
        }
        AgentControlStoreError::RestartTurnUnsettled => AgentServiceError::RestartTurnUnsettled,
        AgentControlStoreError::WorktreeRecoveryConflict => {
            AgentServiceError::WorktreeRecoveryConflict
        }
        AgentControlStoreError::LimitReached { limit, live } => {
            AgentServiceError::LimitReached { limit, live }
        }
        AgentControlStoreError::Corrupt(detail) => AgentServiceError::Corrupt(detail),
        AgentControlStoreError::Unavailable(detail) => AgentServiceError::Unavailable(detail),
    }
}

pub(super) fn runtime_error(error: AgentRuntimeError) -> AgentServiceError {
    AgentServiceError::Unavailable(error.to_string())
}
