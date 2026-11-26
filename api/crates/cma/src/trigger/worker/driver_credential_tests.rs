use super::*;

#[test]
fn checkpoint_wait_is_distinct_from_provider_and_workspace_failures() {
    assert_eq!(
        agent_service_error(AgentServiceError::SandboxNotReady).code,
        "launch_sandbox_not_ready"
    );
    assert_eq!(
        workspace_service_error(WorkspaceServiceError::SandboxNotReady).code,
        "launch_sandbox_not_ready"
    );
    for failure in [
        agent_service_error(AgentServiceError::WorkspaceNotReady),
        agent_service_error(AgentServiceError::Unavailable("unavailable".into())),
        workspace_service_error(WorkspaceServiceError::Unavailable("unavailable".into())),
    ] {
        assert_eq!(failure.code, "launch_dependency_unavailable");
        assert!(failure.retryable);
    }
}

#[test]
fn credential_prerequisite_lost_after_launch_acceptance_keeps_the_actionable_code() {
    let failure = sandbox_service_error(SandboxServiceError::CredentialSetupRequired);
    assert_eq!(failure.code, "sandbox_credential_setup_required");
    assert!(!failure.retryable);
    assert!(failure.safe_detail.contains("same account"));
    let outage =
        sandbox_service_error(SandboxServiceError::Unavailable("private-authority".into()));
    assert!(outage.retryable);
    assert_eq!(outage.code, "launch_dependency_unavailable");
    assert!(!outage.safe_detail.contains("private-authority"));
}

#[test]
fn global_capacity_keeps_the_original_launch_retryable() {
    let failure = sandbox_service_error(SandboxServiceError::GlobalCapacityReached);
    assert_eq!(failure.code, "sandbox_capacity_unavailable");
    assert!(failure.retryable);
    let owner_limit = sandbox_service_error(SandboxServiceError::SandboxLimitReached {
        limit: 1,
        live: 1,
        usage: Default::default(),
    });
    assert_eq!(owner_limit.code, "sandbox_limit_reached");
    assert!(!owner_limit.retryable);
}
