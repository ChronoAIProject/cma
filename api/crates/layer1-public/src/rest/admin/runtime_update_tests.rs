use super::*;
use crate::rest::instances_tests::runtime_update_tests::{
    assert_managed_update_queued, native_runtime_update_sandbox,
};
use layer2_core::DetailKey;

const PATH: &str = "/api/v1/admin/sandboxes/sbx_update/runtime-update";

#[tokio::test]
async fn admin_managed_runtime_update_refuses_an_explicit_image_on_a_retained_home() {
    for custom in [true, false] {
        let f = fixture(false);
        let original = native_runtime_update_sandbox(
            sandbox("sbx_update", "foreign-owner", "runtime-v1"),
            custom,
        );
        f.sandboxes.create_sandbox(original.clone()).await.unwrap();
        let response = send(
            &f.app,
            Method::POST,
            PATH,
            Some(json!({"target_version":"runtime-v2"})),
            [Capability::ControlAnySandbox],
        )
        .await;
        let status = response.status();
        let body = response_json(response).await;
        let stored = f
            .sandboxes
            .get_sandbox(&original.id)
            .await
            .unwrap()
            .unwrap();
        let records = f.activity.records.lock().unwrap();
        if custom {
            assert_eq!(status, StatusCode::CONFLICT, "{body}");
            assert_eq!(body["code"], "runtime_update_not_available");
            assert_eq!(stored, original);
            assert!(records.is_empty());
        } else {
            assert_eq!(status, StatusCode::ACCEPTED, "{body}");
            assert_managed_update_queued(&original, &stored, &body);
            assert_eq!(records.len(), 1);
            let record = &records[0];
            assert_eq!(record.actor, Some(SubjectId::new("operator-a")));
            assert_eq!(record.action, ActivityAction::Update);
            assert_eq!(record.outcome, ActivityOutcome::Succeeded);
            assert_eq!(
                record.target,
                Some(ActivityTarget::Resource("sandbox:sbx_update".into()))
            );
            assert_eq!(record.detail.get(DetailKey::Subject), Some("foreign-owner"));
            assert_eq!(record.detail.get(DetailKey::Runtime), Some("runtime-v2"));
        }
    }
}

#[tokio::test]
async fn runtime_update_admin_capability_target_audit_and_projection_are_exact() {
    let f = fixture(false);
    let original = sandbox("sbx_update", "foreign-owner", "runtime-v2");
    f.sandboxes.create_sandbox(original.clone()).await.unwrap();
    for caps in [
        vec![],
        vec![Capability::UseOwnSandboxes],
        vec![Capability::ReadAllSandboxes],
    ] {
        let response = send(
            &f.app,
            Method::POST,
            PATH,
            Some(json!({"target_version":"runtime-v2"})),
            caps,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let response = send(
        &f.app,
        Method::POST,
        PATH,
        Some(json!({"target_version":"unapproved-runtime"})),
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(response).await["code"],
        "runtime_update_target_changed"
    );
    assert!(f.activity.records.lock().unwrap().is_empty());
    let response = send(
        &f.app,
        Method::POST,
        PATH,
        Some(json!({"target_version":"runtime-v2"})),
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let view = response_json(response).await;
    assert_eq!(view["runtime_update"]["state"], "up_to_date");
    assert_eq!(view["runtime_update"]["latest_version"], "runtime-v2");
    assert_eq!(
        f.sandboxes.get_sandbox(&original.id).await.unwrap(),
        Some(original)
    );
    let records = f.activity.records.lock().unwrap().clone();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.actor, Some(SubjectId::new("operator-a")));
    assert_eq!(record.action, ActivityAction::Update);
    assert_eq!(
        record.target,
        Some(ActivityTarget::Resource("sandbox:sbx_update".into()))
    );
    assert_eq!(record.detail.get(DetailKey::Subject), Some("foreign-owner"));
    assert_eq!(record.detail.get(DetailKey::Runtime), Some("runtime-v2"));
    assert_eq!(
        record.detail.get(DetailKey::Setting),
        Some("runtime_update")
    );
    let response = send(
        &f.app,
        Method::GET,
        "/api/v1/admin/sandboxes/sbx_update",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await["runtime_update"],
        view["runtime_update"]
    );
}

#[tokio::test]
async fn runtime_update_admin_rejects_unsupported_homes_and_missing_sandboxes() {
    let f = fixture(false);
    f.sandboxes
        .create_sandbox(sandbox("sbx_update", "foreign-owner", "runtime-v1"))
        .await
        .unwrap();
    let response = send(
        &f.app,
        Method::POST,
        PATH,
        Some(json!({"target_version":"runtime-v2"})),
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(response).await["code"],
        "runtime_update_not_available"
    );
    let response = send(
        &f.app,
        Method::POST,
        "/api/v1/admin/sandboxes/missing/runtime-update",
        Some(json!({"target_version":"runtime-v2"})),
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(f.activity.records.lock().unwrap().is_empty());
}
