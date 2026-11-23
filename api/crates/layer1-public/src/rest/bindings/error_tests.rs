use super::*;

#[tokio::test]
async fn binding_home_capacity_refusal_is_safe_and_retryable() {
    let response = error(BindingError::GlobalCapacityReached);
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "60");
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "sandbox_capacity_unavailable");
    assert_eq!(
        body["detail"],
        layer2_core::admission::CapacityKind::RetainedHomes.guidance()
    );
    assert_eq!(body.as_object().unwrap().len(), 5);
    assert!(!body.to_string().contains("limit"));
    assert!(!body.to_string().contains("count"));
}

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
        .contains("create a new binding with the required scope"));
    assert!(body["detail"]
        .as_str()
        .unwrap()
        .contains("have not been changed"));
}

#[tokio::test]
async fn changed_scope_put_is_actionable_and_unchanged_put_preserves_the_binding() {
    use crate::rest::bindings_tests::{app, authority, body, create_backing_resources, request};
    use layer2_core::profiles::content_digest;
    use layer2_core::sandbox::binding::{BindingStore, BindingWork};
    use serde_json::{json, Value};
    use tower::ServiceExt;

    for retained_service in [
        "retained-private-service",
        "aa480b39-3ba0-42af-9c21-10d02885337c",
    ] {
        let (app, store) = app();
        let path = "/api/v1/cmaeg/bindings/scope-binding";
        let mut input = body();
        input["profile"]["permission"]["service_scope"] = json!([retained_service]);
        input["profile_digest"] =
            format!("sha256:{}", content_digest(&input["profile"]).unwrap()).into();
        let response = app
            .clone()
            .oneshot(request("PUT", path, Some(input.clone()), Some(authority())))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut retained = store.binding("scope-binding").await.unwrap().unwrap();
        let revision = retained.revision;
        retained.state = BindingState::Ready;
        retained.work = BindingWork::Idle;
        retained
            .advance_revision(layer2_core::now_rfc3339())
            .unwrap();
        create_backing_resources(&store, &retained).await;
        store
            .save_binding(Some(revision), retained.clone())
            .await
            .unwrap();

        for service in [
            None,
            Some("personal-custom-service"),
            Some("106f2888-6bb9-4ff1-a8d8-249af96c06ac"),
            Some("github"),
            None,
        ] {
            let mut update = input.clone();
            if let Some(service) = service {
                update["profile"]["permission"]["service_scope"] =
                    json!([retained_service, service]);
                update["profile_digest"] =
                    format!("sha256:{}", content_digest(&update["profile"]).unwrap()).into();
            }
            let response = app
                .clone()
                .oneshot(request("PUT", path, Some(update), Some(authority())))
                .await
                .unwrap();
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            if service.is_some() {
                assert_eq!(response.status(), StatusCode::CONFLICT);
                assert_eq!(
                    response.headers()[header::CONTENT_TYPE],
                    "application/problem+json"
                );
                let bytes = axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap();
                let problem: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(problem["status"], 409);
                assert_eq!(problem["code"], "binding_reconfiguration_unsupported");
                let detail = problem["detail"].as_str().unwrap();
                assert!(detail.contains("create a new binding with the required scope"));
                assert!(detail.contains("have not been changed"));
                assert!(!problem
                    .to_string()
                    .contains(input["agent_key"].as_str().unwrap()));
            } else {
                assert_eq!(response.status(), StatusCode::OK);
            }
            assert_eq!(
                store.binding("scope-binding").await.unwrap(),
                Some(retained.clone())
            );
        }
    }
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
