use super::*;
use crate::dependency_test_support::{finish, Peer};
use axum::http::{header, Method};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn retirement_receipt_requires_exact_terminal_provider_evidence() {
    let mut peer = Peer::start().await;
    let issuer = Arc::new(KeyIssuer::new(
        crate::auth::test_support::contract_auth(&peer.origin)
            .http
            .clone(),
        &peer.origin,
        "test-deployment",
    ));
    let digest = "a".repeat(64);
    let valid = json!({
        "operation_id": "mint_fixture", "owner": "fixture-owner", "request_sha256": digest,
        "receipt_id": "fixture-receipt", "revision": 1, "state": "revoked", "key_id": "fixture-key"
    });
    let mut no_issuance = valid.clone();
    no_issuance["state"] = json!("no_issuance");
    no_issuance.as_object_mut().unwrap().remove("key_id");
    let mut no_issuance_null_key = no_issuance.clone();
    no_issuance_null_key["key_id"] = json!(null);
    let mut no_issuance_with_key = no_issuance.clone();
    no_issuance_with_key["key_id"] = json!("fixture-key");
    let mut cases = vec![
        (200, valid.clone(), Some(true)),
        (200, no_issuance, Some(true)),
        (200, no_issuance_null_key, Some(true)),
        (200, no_issuance_with_key, None),
        (202, valid.clone(), Some(false)),
        (404, valid.clone(), Some(false)),
        (503, valid.clone(), None),
    ];
    for (field, invalid) in [
        ("operation_id", json!("mint_other")),
        ("owner", json!("other-owner")),
        ("request_sha256", json!("b".repeat(64))),
        ("state", json!("pending")),
        ("key_id", json!(null)),
        ("revision", json!(0)),
        ("receipt_id", json!("")),
        ("receipt_id", json!("a".repeat(MAX_RECEIPT_BYTES))),
    ] {
        let mut body = valid.clone();
        body[field] = invalid;
        cases.push((200, body, None));
    }
    for (status, body, expected) in cases {
        let issuer = issuer.clone();
        let digest = digest.clone();
        let task = tokio::spawn(async move {
            issuer
                .retirement_receipt(
                    "mint_fixture",
                    &SubjectId::new("fixture-owner"),
                    &digest,
                    "synthetic-owner-bearer",
                )
                .await
        });
        let request = peer.next().await;
        assert_eq!(request.method, Method::GET);
        assert_eq!(
            request.uri.path(),
            "/api/v1/api-key-issuances/mint_fixture/retirement"
        );
        assert_eq!(
            request.uri.query(),
            Some(format!("request_sha256={}", "a".repeat(64)).as_str())
        );
        assert_eq!(
            request.headers[header::AUTHORIZATION],
            "Bearer synthetic-owner-bearer"
        );
        request.respond(status, body.to_string());
        assert_eq!(
            finish(task).await.ok().map(|receipt| receipt.is_some()),
            expected
        );
    }
}
