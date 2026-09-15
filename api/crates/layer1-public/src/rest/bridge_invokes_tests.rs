//! Grant management HTTP boundary and shared wire contract fixtures.
pub(super) mod fixture;
use axum::http::{Method, StatusCode};
use fixture::*;
use serde_json::json;

#[tokio::test]
async fn disabled_explicit_consent_replays_then_terminal_revoke_preserves_safe_target_and_actor() {
    let f = fixture().await;
    let grants = format!("/api/v1/bridges/{}/invoke-grants", f.id);
    let readiness = format!("/api/v1/bridges/{}/invoke-readiness", f.id);
    let missing = send(&f.router, Method::GET, &readiness, "owner-a", None, None).await;
    assert_eq!(missing.0, StatusCode::OK);
    assert_eq!(missing.1["status"], "consent_required");
    let issue = send(
        &f.router,
        Method::POST,
        &grants,
        "owner-a",
        Some("consent"),
        Some(issue_body()),
    )
    .await;
    assert_eq!(issue.0, StatusCode::CREATED);
    assert_eq!(issue.1["result"]["issued_by"]["subject"], "owner-a");
    assert_eq!(
        send(
            &f.router,
            Method::POST,
            &grants,
            "owner-a",
            Some("consent"),
            Some(issue_body())
        )
        .await,
        issue
    );
    let grant = issue.1["result"]["id"].as_str().unwrap();
    let detail = format!("/api/v1/bridge-invoke-grants/{grant}");
    assert_eq!(
        send(&f.router, Method::GET, &detail, "owner-a", None, None)
            .await
            .1,
        issue.1["result"]
    );
    let ready = send(&f.router, Method::GET, &readiness, "owner-a", None, None).await;
    assert_eq!(ready.1["status"], "consent_present");
    assert_eq!(ready.1["candidate"], issue.1["result"]);
    for secret in [
        "connector",
        "custody",
        "issuer",
        "resource_audit",
        "callback_url",
        "execution_proof",
    ] {
        assert!(!issue.1.to_string().contains(secret));
    }
    let body = json!({"expected_bridge_revision":1,"expected_revision":1});
    let revoked = send(
        &f.router,
        Method::POST,
        &format!("{detail}/revoke"),
        "owner-a",
        Some("revoke"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(revoked.0, StatusCode::OK);
    assert_eq!(revoked.1["result"]["state"]["kind"], "revoked");
    assert_eq!(
        send(
            &f.router,
            Method::POST,
            &format!("{detail}/revoke"),
            "owner-a",
            Some("revoke"),
            Some(body)
        )
        .await,
        revoked
    );
    assert_eq!(
        send(&f.router, Method::GET, &readiness, "owner-a", None, None)
            .await
            .1["status"],
        "consent_required"
    );
    assert_eq!(
        send(
            &f.router,
            Method::POST,
            &grants,
            "owner-a",
            Some("consent"),
            Some(issue_body())
        )
        .await,
        issue
    );
}

#[tokio::test]
async fn explicit_consent_and_idempotency_are_required_and_injected_authority_is_rejected() {
    let f = fixture().await;
    let path = format!("/api/v1/bridges/{}/invoke-grants", f.id);
    assert_eq!(
        send(
            &f.router,
            Method::POST,
            &path,
            "owner-a",
            None,
            Some(issue_body())
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let mut missing = issue_body();
    missing.as_object_mut().unwrap().remove("shared_context");
    assert_eq!(
        send(
            &f.router,
            Method::POST,
            &path,
            "owner-a",
            Some("missing"),
            Some(missing)
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    for field in [
        "owner-a",
        "issued_by",
        "resource_audit",
        "config",
        "custody",
        "grant_id",
    ] {
        let mut forged = issue_body();
        forged[field] = json!("injected");
        assert_eq!(
            send(
                &f.router,
                Method::POST,
                &path,
                "owner-a",
                Some(field),
                Some(forged)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        send(
            &f.router,
            Method::POST,
            &path,
            "outsider",
            Some("foreign"),
            Some(issue_body())
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &f.router,
            Method::GET,
            &format!("{path}?limit=0"),
            "owner-a",
            None,
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(&f.router, Method::GET, &path, "owner-a", None, None)
            .await
            .1["items"],
        json!([])
    );
}
