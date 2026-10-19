use super::*;
use layer2_core::sandbox::{RuntimeBinding, SandboxRuntimeUpdateService};
const PATH: &str = "/api/v1/sandboxes/sbx_update/runtime-update";

pub(crate) async fn update_fixture(configured: bool) -> Fixture {
    let mut f = fixture(3);
    let mut sandbox = stored_sandbox("sbx_update", "owner", SandboxStatus::Active, 4);
    sandbox.runtime = Some(RuntimeBinding {
        runtime_id: "retained-runtime".into(),
        runtime_version: "runtime-v2".into(),
        provision_generation: None,
        boot_id: None,
        bound_at: NOW.into(),
    });
    f.store.create_sandbox(sandbox).await.unwrap();
    if configured {
        f.state = f
            .state
            .with_runtime_updates(Arc::new(SandboxRuntimeUpdateService::new(
                f.store.clone(),
                "runtime-v2".into(),
            )));
    }
    f.router = hierarchy_router(f.state.clone());
    f
}

#[tokio::test]
async fn runtime_update_endpoint_auth_target_and_idempotent_noop_are_exact() {
    let f = update_fixture(true).await;
    let body = json!({"target_version":"runtime-v2"});
    for (subject, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (
            Some(("owner", Authorization::none())),
            StatusCode::FORBIDDEN,
        ),
        (Some(("foreign", all_capabilities())), StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            json_response(
                &f.router,
                request(
                    Method::POST,
                    PATH,
                    Some(body.clone()),
                    subject,
                    Some("auth")
                )
            )
            .await
            .0,
            expected
        );
    }
    assert_eq!(
        json_response(
            &f.router,
            authenticated(Method::POST, PATH, Some(body.clone()), None)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, result) = json_response(
        &f.router,
        authenticated(Method::POST, PATH, Some(body.clone()), Some("same-target")),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(result["runtime_update"]["state"], "up_to_date");
    assert_eq!(result["runtime_update"]["current_version"], "runtime-v2");
    assert_eq!(result["revision"], 1);
    assert_eq!(
        json_response(
            &f.router,
            authenticated(Method::POST, PATH, Some(body), Some("same-target"))
        )
        .await,
        (status, result)
    );
    let stale = json!({"target_version":"runtime-v1"});
    let (status, result) = json_response(
        &f.router,
        authenticated(Method::POST, PATH, Some(stale.clone()), Some("stale")),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(result["code"], "runtime_update_target_changed");
    assert_eq!(
        json_response(
            &f.router,
            authenticated(Method::POST, PATH, Some(stale), Some("same-target"))
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn runtime_update_detail_projection_is_optional_without_feature_and_hides_other_owner() {
    let configured = update_fixture(true).await;
    let path = "/api/v1/sandboxes/sbx_update";
    let (status, result) = json_response(
        &configured.router,
        authenticated(Method::GET, path, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        result["runtime_update"]["schedule"],
        json!({"timezone":"Asia/Singapore","local_time":"02:00"})
    );
    assert!(result["runtime_update"].get("requested_at").is_none());
    let unconfigured = update_fixture(false).await;
    let (_, result) = json_response(
        &unconfigured.router,
        authenticated(Method::GET, path, None, None),
    )
    .await;
    assert!(result.get("runtime_update").is_none());
    let result = json_response(
        &configured.router,
        request(
            Method::GET,
            path,
            None,
            Some(("foreign", all_capabilities())),
            None,
        ),
    )
    .await;
    assert_eq!(result.0, StatusCode::NOT_FOUND);
}
