use super::*;
use layer2_core::sandbox::{
    RetainedRecoveryDisposition, RetainedRecoveryInspector, RetainedRecoveryService,
    RetainedRecoverySnapshot,
};

pub(crate) struct NeverInspect;
#[async_trait::async_trait]
impl RetainedRecoveryInspector for NeverInspect {
    async fn inspect(&self, _: &RetainedRecoverySnapshot) -> RetainedRecoveryDisposition {
        panic!("active lifecycle does not authorize native recovery inspection")
    }
}

#[tokio::test]
async fn retained_recovery_get_is_owner_only_closed_and_has_no_action_route() {
    let mut f = update_fixture(false).await;
    f.state = f
        .state
        .with_retained_recovery(Arc::new(RetainedRecoveryService::new(
            f.store.clone(),
            Arc::new(NeverInspect),
        )));
    f.router = hierarchy_router(f.state.clone());
    let path = "/api/v1/sandboxes/sbx_update/runtime-recovery";
    let before = f
        .store
        .get_sandbox(&layer2_core::SandboxId("sbx_update".into()))
        .await
        .unwrap();
    for (subject, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (
            Some(("owner", Authorization::none())),
            StatusCode::FORBIDDEN,
        ),
        (Some(("foreign", all_capabilities())), StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            json_response(&f.router, request(Method::GET, path, None, subject, None))
                .await
                .0,
            expected
        );
    }
    let (status, body) =
        json_response(&f.router, authenticated(Method::GET, path, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"disposition": "active_lifecycle", "admission_enabled": false})
    );
    assert_eq!(
        f.store
            .get_sandbox(&layer2_core::SandboxId("sbx_update".into()))
            .await
            .unwrap(),
        before
    );
    let response = f
        .router
        .clone()
        .oneshot(authenticated(
            Method::POST,
            path,
            Some(json!({"consent": true})),
            Some("no-admission"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn retained_recovery_get_without_service_returns_unavailable() {
    let f = update_fixture(false).await;
    assert_eq!(
        json_response(
            &f.router,
            authenticated(
                Method::GET,
                "/api/v1/sandboxes/sbx_update/runtime-recovery",
                None,
                None,
            )
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}
