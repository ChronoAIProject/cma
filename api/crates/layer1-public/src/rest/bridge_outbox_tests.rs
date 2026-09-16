pub(super) mod fixture;
use super::bridge_invokes_tests::fixture::send;
use axum::http::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn recovery_boundary_requires_exact_cas_acknowledgement_and_closed_nested_actions() {
    let f = fixture::fixture().await;
    let path = format!("/api/v1/bridges/{}/delivery-recovery", f.id);
    let body = json!({"item_id":f.item.id,"expected_revision":4,"expected_bridge_revision":2,"action":{"kind":"retry","accept_duplicate_risk":false}});
    let missing = send(
        &f.router,
        Method::POST,
        &path,
        "owner-a",
        Some("recover"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(missing.0, StatusCode::BAD_REQUEST);
    assert_eq!(missing.1["code"], "bridge_delivery_duplicate_risk_required");
    for action in [
        json!({"kind":"reconcile","sent":true}),
        json!({"kind":"abandon","chat_id":"elsewhere"}),
        json!({"kind":"retry","accept_duplicate_risk":true,"credential":"secret"}),
    ] {
        let mut extra = body.clone();
        extra["action"] = action;
        let refusal = send(
            &f.router,
            Method::POST,
            &path,
            "owner-a",
            Some("extra"),
            Some(extra),
        )
        .await;
        assert_eq!(refusal.0, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(refusal.1["code"], "invalid_json");
    }
    let mut accepted = body.clone();
    accepted["action"]["accept_duplicate_risk"] = json!(true);
    let reply = send(
        &f.router,
        Method::POST,
        &path,
        "owner-a",
        Some("recover"),
        Some(accepted.clone()),
    )
    .await;
    assert_eq!(reply.0, StatusCode::OK);
    assert_eq!(reply.1["result"]["status"], "pending");
    assert_eq!(reply.1["result"]["attempts"][0]["status"], "unknown");
    assert_eq!(
        send(
            &f.router,
            Method::POST,
            &path,
            "owner-a",
            Some("recover"),
            Some(accepted)
        )
        .await,
        reply
    );
    assert_eq!(
        send(
            &f.router,
            Method::POST,
            &path,
            "outsider",
            Some("recover"),
            Some(body)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let serialized = reply.1.to_string();
    for secret in [
        "Private reply text",
        "protected-original-chat",
        "original-inbound",
        "brsec_",
    ] {
        assert!(!serialized.contains(secret));
    }
}
