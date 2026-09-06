use super::*;

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
