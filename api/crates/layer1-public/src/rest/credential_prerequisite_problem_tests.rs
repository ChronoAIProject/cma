use super::*;
use crate::rest::instances_problem::HierarchyProblem;
use crate::rest::trigger_problem::TriggerProblem;
use layer2_core::{sandbox::SandboxServiceError, trigger_domain::LaunchServiceError};

#[tokio::test]
async fn manual_and_launch_credential_setup_refusals_have_the_same_safe_document() {
    let expected = serde_json::json!({
        "type": "https://spec.chrono-ai.fun/cma/1.0/problems/sandbox-credential-setup-required",
        "title": "sandbox credential setup required",
        "status": 409,
        "detail": SANDBOX_CREDENTIAL_SETUP_DETAIL,
        "code": "sandbox_credential_setup_required",
    });
    for response in [
        HierarchyProblem::sandbox(SandboxServiceError::CredentialSetupRequired).into_response(),
        TriggerProblem::from(LaunchServiceError::CredentialSetupRequired).into_response(),
    ] {
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            expected
        );
    }
}

#[tokio::test]
async fn prerequisite_outages_do_not_expose_private_dependency_details() {
    for response in [
        HierarchyProblem::sandbox(SandboxServiceError::Unavailable("private-authority".into()))
            .into_response(),
        TriggerProblem::from(LaunchServiceError::Unavailable("private-authority".into()))
            .into_response(),
    ] {
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let document: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(!document.to_string().contains("private-authority"));
        assert_ne!(document["code"], "sandbox_credential_setup_required");
    }
}
