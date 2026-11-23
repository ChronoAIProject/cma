//! A refused binding release is a typed conflict with a closed detail.
use super::*;

#[tokio::test]
async fn refused_release_is_a_typed_conflict_that_names_only_safe_provider_codes() {
    for (code, named) in [
        ("forbidden", "(HTTP 403 forbidden)"),
        ("See https://internal.example/x", "(HTTP 403)"),
        ("", "(HTTP 403)"),
    ] {
        let response = management_error(ManagementError::ReleaseRefused {
            status: 403,
            code: code.into(),
        });
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "provider_rejected");
        let detail = body["detail"].as_str().unwrap();
        assert!(detail.contains(named), "{detail}");
        assert!(detail.contains("was not deleted"));
        assert!(!detail.contains("internal.example"));
    }
}
