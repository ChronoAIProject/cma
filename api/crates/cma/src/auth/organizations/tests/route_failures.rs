//! Exercise the real HTTP route after successful provider authentication.
//!
//! These cases distinguish empty membership, refusal and unavailable authority;
//! they do not claim that an injected provider failure caused the live incident.

use std::sync::atomic::Ordering;

use axum::http::StatusCode;
use serde_json::json;

mod diagnostics;
mod fixture;
use fixture::Fixture;

#[tokio::test]
async fn both_managers_observe_current_choices_and_an_empty_authorized_inventory() {
    let f = Fixture::new().await;
    for manager in [0, 1, 0, 1] {
        let role = if manager == 0 { "admin" } else { "viewer" };
        f.reply(
            StatusCode::OK,
            json!({"orgs":[
                {"id":"org-z","your_role":"member"},
                {"id":"org-a","your_role":role,"display_name":"A"}
            ]}),
        )
        .await;
        let before = f.calls();
        let (status, body) = f.request(manager).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["items"].as_array().unwrap().len(), 2);
        assert_eq!(body["items"][0]["id"], "org-a");
        assert_eq!(body["items"][0]["role"], role);
        assert_eq!(body["items"][1]["id"], "org-z");
        assert_eq!(f.calls() - before, 1);
    }
    f.reply(StatusCode::OK, json!({"orgs":[]})).await;
    for manager in 0..2 {
        let (status, body) = f.request(manager).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({"items":[]}));
    }
    assert_eq!(f.calls(), 6);
    f.close().await;
}

#[tokio::test]
async fn org_dependency_failure_after_identity_success_is_typed_and_not_empty_success() {
    let f = Fixture::new().await;
    for manager in 0..2 {
        for upstream in [
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            f.reply(upstream, json!({"detail":"must-not-leak-provider-error"}))
                .await;
            let before = f.calls();
            let (status, body) = f.request(manager).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body["code"], "organization_authority_unavailable");
            assert!(body.get("items").is_none());
            assert_eq!(f.calls() - before, 1, "no hidden inventory retry");

            f.reply(StatusCode::OK, json!({"orgs":[]})).await;
            assert_eq!(
                f.request(manager).await,
                (StatusCode::OK, json!({"items":[]}))
            );
        }
    }
    assert_eq!(f.calls(), 8);
    f.close().await;
}

#[tokio::test]
async fn provider_nonmember_refusal_is_forbidden_on_both_managers() {
    let f = Fixture::new().await;
    for manager in 0..2 {
        for upstream in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
        ] {
            f.reply(upstream, json!({"detail":"must-not-leak-denial"}))
                .await;
            let before = f.calls();
            let (status, body) = f.request(manager).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(body["code"], "resource_forbidden");
            assert_eq!(f.calls() - before, 1);
        }
    }
    f.close().await;
}

#[tokio::test]
async fn invalid_inventory_stays_unavailable_after_successful_authentication() {
    let f = Fixture::new().await;
    for manager in 0..2 {
        for body in [
            "must-not-leak-invalid-json".to_owned(),
            json!({"orgs":[{"id":"org-a","your_role":"unknown"}]}).to_string(),
            json!({"orgs":[{"id":"../org","your_role":"admin"}]}).to_string(),
            json!({"orgs":[{"id":"org-a","your_role":"admin"},{"id":"org-a","your_role":"viewer"}]}).to_string(),
        ] {
            f.control.inventory.write().await.body = body;
            let before = f.calls();
            let (status, body) = f.request(manager).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body["code"], "organization_authority_unavailable");
            assert_eq!(f.calls() - before, 1);
        }
    }
    f.close().await;
}

#[tokio::test]
async fn next_request_on_either_manager_rechecks_revocation_before_org_inventory() {
    let f = Fixture::new().await;
    for manager in 0..2 {
        assert_eq!(f.request(manager).await.0, StatusCode::OK);
    }
    let before = f.calls();
    let introspections = f.control.introspection_calls.load(Ordering::SeqCst);
    *f.control.introspection.write().await = json!({"active":false});
    for manager in 0..2 {
        assert_eq!(f.request(manager).await.0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(f.calls(), before);
    assert_eq!(
        f.control.introspection_calls.load(Ordering::SeqCst) - introspections,
        2
    );
    f.close().await;
}
