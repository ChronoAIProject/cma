use super::*;

#[tokio::test]
async fn incomplete_creation_reports_its_precondition_without_a_false_version_conflict() {
    let response = management_error(ManagementError::CreationPending);
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "conflict");
    let detail = body["detail"].as_str().unwrap();
    assert!(detail.starts_with("channel_creation_pending:"));
    assert!(detail.contains("original channel creation request"));
    assert!(detail.contains("refresh"));
    assert!(!detail.contains("expected version"));
}

#[tokio::test]
async fn provider_failures_name_their_class_without_transport_text() {
    use crate::providers::ProviderError;
    for (error, detail) in [
        (
            ProviderError::Unavailable("provider returned HTTP 502".into()),
            "provider request failed: the provider answered HTTP 502",
        ),
        (
            ProviderError::Unavailable("provider request timed out".into()),
            "provider request failed: the provider request timed out",
        ),
        (
            ProviderError::Unavailable("provider transport failed".into()),
            "provider request failed: the provider was unreachable",
        ),
        (
            ProviderError::Unavailable(
                "error sending request for url (http://internal.example.svc/token)".into(),
            ),
            "provider request failed: the provider or its service credential was unavailable",
        ),
        (
            ProviderError::Capacity,
            "provider request failed: the gateway's provider concurrency is exhausted; retry later",
        ),
        (
            ProviderError::Sse("frame too large".into()),
            "provider request failed: the provider stream ended before a terminal answer",
        ),
    ] {
        let response = management_error(ManagementError::Provider(error));
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "provider_unavailable");
        assert_eq!(body["detail"], detail);
        assert!(!body["detail"]
            .as_str()
            .unwrap()
            .contains("internal.example"));
    }
}

#[tokio::test]
async fn known_removed_binding_has_actionable_conflict_instead_of_generic_bad_gateway() {
    let response = management_error(ManagementError::Provider(
        crate::providers::ProviderError::Rejected {
            status: 409,
            code: "binding_resources_unavailable".into(),
        },
    ));
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["detail"]
        .as_str()
        .unwrap()
        .contains("Reconnect the channel"));
}

#[tokio::test]
async fn unsupported_skill_version_update_stays_a_typed_conflict() {
    let response = management_error(ManagementError::Provider(
        crate::providers::ProviderError::Rejected {
            status: 409,
            code: "binding_skill_version_update_unsupported".into(),
        },
    ));
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "conflict");
    assert_eq!(
        body["detail"],
        "The selected skill version cannot be changed through a channel profile update. Keep the channel profile unchanged and use the existing Agent's skills endpoint (POST /api/v1/agents/{id}/skills) when upgrades are enabled and supported."
    );
}

#[tokio::test]
async fn known_reconfiguration_refusal_is_actionable_and_keeps_its_conflict_status() {
    let response = management_error(ManagementError::Provider(
        crate::providers::ProviderError::Rejected {
            status: 409,
            code: "binding_reconfiguration_unsupported".into(),
        },
    ));
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "provider_rejected");
    assert_eq!(body["status"], 409);
    let detail = body["detail"].as_str().unwrap();
    assert!(detail.contains("service scope"));
    assert!(detail.contains("create a new binding with the required scope"));
    assert!(detail.contains("have not been changed"));
    assert!(detail.contains("Keep the current profile"));
}

#[tokio::test]
async fn unrecognized_refusals_do_not_expose_provider_error_text() {
    for (status, code) in [
        (409, "private-provider-credential-must-not-leak"),
        (400, "binding_reconfiguration_unsupported"),
    ] {
        let response = management_error(ManagementError::Provider(
            crate::providers::ProviderError::Rejected {
                status,
                code: code.into(),
            },
        ));
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["detail"], "provider rejected the channel request");
        assert!(!String::from_utf8_lossy(&bytes).contains(code));
    }
}

#[test]
fn unrelated_provider_conflicts_keep_the_existing_error_mapping() {
    let response = management_error(ManagementError::Provider(
        crate::providers::ProviderError::Rejected {
            status: 409,
            code: "another_provider_failure".into(),
        },
    ));
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}
