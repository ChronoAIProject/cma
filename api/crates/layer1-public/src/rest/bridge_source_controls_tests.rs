mod bridge_store;
pub(super) mod fixture;
use super::bridge_invokes_tests::fixture::{request, send};
use axum::{
    body::{to_bytes, Body},
    http::{Method, StatusCode},
};
use fixture::*;
use serde_json::{json, Value};
use tower::ServiceExt;

#[tokio::test]
async fn revoked_invoke_still_allows_exact_receipt_recovery_and_explicit_policy_revocation() {
    let f = fixture().await;
    let enabled = send(
        &f.router,
        Method::PUT,
        &f.path,
        "owner-a",
        Some("enable"),
        Some(body(0, "allow_sender_cancel_and_steer")),
    )
    .await;
    assert_eq!(enabled.0, StatusCode::OK);
    f.bridge
        .service
        .revoke(
            &super::resource_access_tests::context("owner-a"),
            None,
            layer2_core::bridge::invoke::RevokeBridgeInvokeGrant {
                grant_id: layer2_core::bridge::invoke::BridgeInvokeGrantId(f.grant.clone()),
                expected_bridge_revision: 2,
                expected_revision: 1,
                idempotency_key: "revoke-invoke".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        send(
            &f.router,
            Method::PUT,
            &f.path,
            "owner-a",
            Some("enable"),
            Some(body(0, "allow_sender_cancel_and_steer"))
        )
        .await,
        enabled
    );
    let mut new = body(1, "allow_sender_cancel_and_steer");
    new["expected_grant_revision"] = json!(2);
    let rejected = send(
        &f.router,
        Method::PUT,
        &f.path,
        "owner-a",
        Some("new-consent"),
        Some(new),
    )
    .await;
    assert_eq!(rejected.0, StatusCode::CONFLICT);
    assert_eq!(rejected.1["code"], "bridge_invoke_revoked");
    let mut revoke = body(1, "revoke_sender_controls");
    revoke["expected_grant_revision"] = json!(2);
    let revoked = send(
        &f.router,
        Method::PUT,
        &f.path,
        "owner-a",
        Some("revoke-policy"),
        Some(revoke),
    )
    .await;
    assert_eq!(revoked.0, StatusCode::OK);
    assert_eq!(revoked.1["result"]["enabled"], false);
}

#[tokio::test]
async fn explicit_consent_is_separate_and_exact_replay_never_reenables_revoked_policy() {
    let f = fixture().await;
    let absent = send(&f.router, Method::GET, &f.path, "owner-a", None, None).await;
    assert_eq!(absent, (StatusCode::OK, json!({"policy":null})));
    let grant_path = format!("/api/v1/bridge-invoke-grants/{}", f.grant);
    let original = send(&f.router, Method::GET, &grant_path, "owner-a", None, None).await;
    let enabled = send(
        &f.router,
        Method::PUT,
        &f.path,
        "owner-a",
        Some("enable"),
        Some(body(0, "allow_sender_cancel_and_steer")),
    )
    .await;
    assert_eq!(enabled.0, StatusCode::OK, "{}", enabled.1);
    assert_eq!(enabled.1["result"]["updated_by"]["subject"], "owner-a");
    assert_eq!(enabled.1["result"]["scope"], "source_sender");
    assert_eq!(enabled.1["result"]["approval_mode"], "dashboard_only");
    assert_eq!(enabled.1["result"]["revision"], 1);
    let revoked = send(
        &f.router,
        Method::PUT,
        &f.path,
        "owner-a",
        Some("revoke"),
        Some(body(1, "revoke_sender_controls")),
    )
    .await;
    assert_eq!(revoked.0, StatusCode::OK);
    assert_eq!(revoked.1["result"]["enabled"], false);
    assert_eq!(
        send(
            &f.router,
            Method::PUT,
            &f.path,
            "owner-a",
            Some("enable"),
            Some(body(0, "allow_sender_cancel_and_steer"))
        )
        .await,
        enabled
    );
    assert_eq!(
        send(&f.router, Method::GET, &f.path, "owner-a", None, None)
            .await
            .1["policy"],
        revoked.1["result"]
    );
    let conflict = send(
        &f.router,
        Method::PUT,
        &f.path,
        "owner-a",
        Some("enable"),
        Some(body(2, "allow_sender_cancel_and_steer")),
    )
    .await;
    assert_eq!(conflict.0, StatusCode::CONFLICT);
    assert_eq!(conflict.1["code"], "idempotency_key_reused");
    let renewed = send(
        &f.router,
        Method::PUT,
        &f.path,
        "owner-a",
        Some("new-enable"),
        Some(body(2, "allow_sender_cancel_and_steer")),
    )
    .await;
    assert_eq!(renewed.0, StatusCode::OK);
    assert_eq!(renewed.1["result"]["revision"], 3);
    assert_eq!(
        send(&f.router, Method::GET, &grant_path, "owner-a", None, None).await,
        original
    );
    for private in [
        "resource_audit",
        "custody",
        "connector",
        "request_sha256",
        "runtime_id",
    ] {
        assert!(!enabled.1.to_string().contains(private));
    }
}

#[tokio::test]
async fn consent_rejects_implicit_actions_injected_authority_stale_revisions_and_foreign_callers() {
    let f = fixture().await;
    let valid = body(0, "allow_sender_cancel_and_steer");
    assert_eq!(
        send(
            &f.router,
            Method::PUT,
            &f.path,
            "owner-a",
            None,
            Some(valid.clone())
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    for field in [
        "expected_bridge_revision",
        "expected_grant_revision",
        "expected_policy_revision",
        "action",
    ] {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert_eq!(
            send(
                &f.router,
                Method::PUT,
                &f.path,
                "owner-a",
                Some("missing"),
                Some(missing)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    for field in [
        "actor",
        "owner",
        "scope",
        "approval_mode",
        "target",
        "enabled",
    ] {
        let mut injected = valid.clone();
        injected[field] = json!("injected");
        assert_eq!(
            send(
                &f.router,
                Method::PUT,
                &f.path,
                "owner-a",
                Some("injected"),
                Some(injected)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    for field in [
        "expected_bridge_revision",
        "expected_grant_revision",
        "expected_policy_revision",
    ] {
        let mut stale = valid.clone();
        stale[field] = json!(stale[field].as_u64().unwrap() + 1);
        assert_eq!(
            send(
                &f.router,
                Method::PUT,
                &f.path,
                "owner-a",
                Some("stale"),
                Some(stale)
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
    }
    for method in [Method::GET, Method::PUT] {
        assert_eq!(
            send(
                &f.router,
                method,
                &f.path,
                "outsider",
                Some("foreign"),
                Some(valid.clone())
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    for action in ["approve", "cancel", "steer"] {
        assert_eq!(
            send(
                &f.router,
                Method::PUT,
                &f.path,
                "owner-a",
                Some("wrong-action"),
                Some(body(0, action))
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        send(&f.router, Method::GET, &f.path, "owner-a", None, None)
            .await
            .1,
        json!({"policy":null})
    );
}

#[tokio::test]
async fn disabled_service_and_body_failures_are_private_typed_responses() {
    let f = fixture().await;
    let disabled = super::bridge_source_control_router(None);
    for method in [Method::GET, Method::PUT] {
        let response = disabled
            .clone()
            .oneshot(request(
                method,
                &f.path,
                "owner-a",
                Some("test"),
                Some(body(0, "allow_sender_cancel_and_steer")),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()["cache-control"], "private, no-store");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(response.headers()["retry-after"], "1");
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["code"], "bridge_source_control_unavailable");
    }
    let mut oversized = request(
        Method::PUT,
        &f.path,
        "owner-a",
        Some("large"),
        Some(json!({})),
    );
    *oversized.body_mut() = Body::from(" ".repeat(8193));
    assert_eq!(
        f.router.clone().oneshot(oversized).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}
