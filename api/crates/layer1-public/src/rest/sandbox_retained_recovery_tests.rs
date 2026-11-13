use super::*;
#[path = "sandbox_retained_reopen_fixture.rs"]
pub(crate) mod reopen;

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
async fn retained_recovery_get_is_owner_only_and_unconfigured_action_is_closed() {
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
            Some(json!({"expected_revision": 1, "confirm_same_image_recovery": true})),
            Some("no-admission"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
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

struct NeverReopen;
#[async_trait::async_trait]
impl layer2_core::sandbox::RetainedReopenVerifier for NeverReopen {
    async fn verify(
        &self,
        _: &layer2_core::sandbox::RetainedReopenSnapshot,
    ) -> Result<layer2_core::sandbox::RetainedReopenProof, layer2_core::sandbox::SandboxStoreError>
    {
        panic!("refused requests must not inspect native state")
    }
}

#[tokio::test]
async fn retained_reopen_post_requires_owner_consent_revision_key_and_no_caller_proof() {
    let mut f = update_fixture(false).await;
    f.state = f.state.with_retained_reopening(Arc::new(
        layer2_core::sandbox::RetainedReopenService::new(
            f.store.clone(),
            Arc::new(NeverReopen),
            false,
        ),
    ));
    f.router = hierarchy_router(f.state.clone());
    let path = "/api/v1/sandboxes/sbx_update/runtime-recovery";
    let body = json!({"expected_revision": 1, "confirm_same_image_recovery": true});
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
                    path,
                    Some(body.clone()),
                    subject,
                    Some("recovery-key")
                )
            )
            .await
            .0,
            expected
        );
    }
    for (value, key, expected) in [
        (body.clone(), None, StatusCode::BAD_REQUEST),
        (
            json!({"expected_revision":1,"confirm_same_image_recovery":false}),
            Some("key"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"expected_revision":0,"confirm_same_image_recovery":true}),
            Some("key"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"expected_revision":1,"confirm_same_image_recovery":true,"image":"other-image"}),
            Some("key"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (body, Some("key"), StatusCode::SERVICE_UNAVAILABLE),
    ] {
        assert_eq!(
            json_response(
                &f.router,
                authenticated(Method::POST, path, Some(value), key)
            )
            .await
            .0,
            expected
        );
    }
    assert_eq!(
        f.store
            .get_sandbox(&layer2_core::SandboxId("sbx_update".into()))
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Active
    );
}
