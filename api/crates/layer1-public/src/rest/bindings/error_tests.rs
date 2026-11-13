use super::*;

#[tokio::test]
async fn operator_pause_is_safe_and_paces_binding_event_retries() {
    let response = error(BindingError::AdmissionPaused);
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "60");
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "admission_paused");
    assert_eq!(
        body["detail"],
        layer2_core::admission::ADMISSION_PAUSED_GUIDANCE
    );
}

#[tokio::test]
async fn unsupported_update_explains_that_retained_work_is_unchanged() {
    let response = error(BindingError::ReconfigurationUnsupported);
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "binding_reconfiguration_unsupported");
    assert!(body["detail"]
        .as_str()
        .unwrap()
        .contains("have not been changed"));
}

#[tokio::test]
async fn unsupported_skill_version_update_has_a_named_conflict_code() {
    let response = error(BindingError::SkillVersionUpdateUnsupported {
        id: "research".into(),
        installed: "1.0".into(),
        requested: "1.1".into(),
    });
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "binding_skill_version_update_unsupported");
    assert!(body["detail"].as_str().unwrap().contains("version \"1.0\""));
}

#[tokio::test]
async fn storage_outage_remains_service_unavailable_with_sanitized_detail() {
    let response = error(BindingError::Unavailable("private storage details".into()));
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "binding_unavailable");
    assert_eq!(body["detail"], "Binding service is unavailable.");
}
