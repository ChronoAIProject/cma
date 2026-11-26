use super::*;

#[tokio::test]
async fn migration_and_observation_capabilities_are_independent_and_default_closed() {
    let f = fixture(false);
    let path = "/api/v1/admin/sandboxes/sbx_update/runtime-migration";
    for caps in [
        vec![],
        vec![Capability::UseOwnSandboxes],
        vec![Capability::ReadAllSandboxes],
    ] {
        let response = send(
            &f.app,
            Method::POST,
            path,
            Some(json!({"clear":true})),
            caps,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let response = send(
        &f.app,
        Method::POST,
        path,
        Some(json!({"quiet_since":"2026-01-01T00:00:00Z"})),
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response_json(response).await["code"],
        "invalid_runtime_migration"
    );
    // A structurally complete request reaches the disabled service boundary.
    // Caller-provided approval fields remain invalid even with every guard set.
    let body = json!({
        "migration": {
            "request_key": "operator-request", "expected_revision": 1,
            "expected_image": format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
            "target_image": format!("registry.example/runtime@sha256:{}", "b".repeat(64)),
            "origin": {
                "sandbox_id": "sbx_update", "owner": "foreign-owner",
                "profile_id": "sbp_test", "profile_revision": 1,
                "profile_digest": "a".repeat(64),
                "profile_image": format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
                "home": {
                    "request": {
                        "volume_id": "home", "sandbox_id": "sbx_update",
                        "policy": {"storage_class": "standard-rwo", "capacity_gib": 8}
                    },
                    "receipt": {
                        "volume_id": "home", "request_sha256": "a".repeat(64),
                        "storage_reference": "home-pvc", "storage_generation": "pvc-uid"
                    }
                }
            }
        },
        "authority_id": "operator-authority", "expected_current_revision": 1
    });
    let response = send(
        &f.app,
        Method::POST,
        path,
        Some(body.clone()),
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    for field in [
        "clear",
        "quiet_since",
        "proof",
        "approved_target",
        "verifier_path",
    ] {
        let mut untrusted = body.clone();
        untrusted[field] = json!(true);
        let response = send(
            &f.app,
            Method::POST,
            path,
            Some(untrusted),
            [Capability::ControlAnySandbox],
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response_json(response).await["code"],
            "invalid_runtime_migration"
        );
    }
    let observation = "/api/v1/admin/sandboxes/sbx_update/runtime-update/activity";
    let response = send(
        &f.app,
        Method::GET,
        observation,
        None,
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = send(
        &f.app,
        Method::GET,
        observation,
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(
        body,
        json!({"status":"unknown","reason":"unqualified_source"})
    );
    assert!(f.activity.records.lock().unwrap().is_empty());
}
