#[path = "support/wire.rs"]
#[allow(dead_code)]
mod wire;

use cma_sdk::{Error, IdempotencyKey};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use wire::{Expected, WireServer};

fn receipt() -> Value {
    json!({
        "id": "acd_0123456789abcdef0123456789abcdef",
        "status": "running", "step": "inventory",
        "requested_at": "2026-11-14T00:00:00Z",
        "deadline_at": "2026-11-21T00:00:00Z",
        "completed_at": null, "steps": []
    })
}

fn expected(method: &str, status: u16, response: Value) -> Expected {
    let mut headers = BTreeMap::from([("accept".into(), "application/json".into())]);
    let body = if method == "POST" {
        headers.insert("idempotency-key".into(), "erase-own-data".into());
        headers.insert("content-type".into(), "application/json".into());
        Some(json!({"acknowledge":"irreversible"}))
    } else {
        None
    };
    Expected {
        method: method.into(),
        path: "/api/v1/account/deletion".into(),
        headers,
        body,
        status,
        response: serde_json::to_vec(&response).unwrap(),
        ..Default::default()
    }
}

#[tokio::test]
async fn request_retains_explicit_acknowledgement_and_replay_key() {
    let server = WireServer::new().await;
    let key = IdempotencyKey::new("erase-own-data").unwrap();
    for _ in 0..2 {
        server.expect(expected("POST", 202, receipt()));
        let accepted = server
            .anonymous
            .account()
            .deletion()
            .request(&key)
            .await
            .unwrap();
        assert_eq!(serde_json::to_value(accepted).unwrap(), receipt());
    }
    server.expect(expected("GET", 200, receipt()));
    assert_eq!(
        serde_json::to_value(
            server
                .anonymous
                .account()
                .deletion()
                .status()
                .await
                .unwrap()
        )
        .unwrap(),
        receipt()
    );
    server.assert_drained();
}

#[tokio::test]
async fn failed_status_reads_and_unknown_states_never_claim_completion() {
    let server = WireServer::new().await;
    server.expect(expected(
        "GET",
        404,
        json!({"code":"account_deletion_not_found", "detail":"No request exists."}),
    ));
    assert!(
        matches!(server.anonymous.account().deletion().status().await, Err(Error::Api {status:404, code:Some(code), ..}) if code == "account_deletion_not_found")
    );
    let mut invalid = receipt();
    invalid["status"] = json!("deleted");
    server.expect(expected("GET", 200, invalid));
    assert!(matches!(
        server.anonymous.account().deletion().status().await,
        Err(Error::Protocol { .. })
    ));
    let mut invalid = receipt();
    invalid["step"] = json!("done");
    server.expect(expected("GET", 200, invalid));
    assert!(matches!(
        server.anonymous.account().deletion().status().await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn completion_timestamp_is_required_but_nullable() {
    let server = WireServer::new().await;
    server.expect(expected("GET", 200, receipt()));
    assert!(
        server
            .anonymous
            .account()
            .deletion()
            .status()
            .await
            .unwrap()
            .completed_at
            .is_none()
    );
    let mut completed = receipt();
    completed["completed_at"] = json!("2026-11-15T00:00:00Z");
    server.expect(expected("GET", 200, completed));
    assert_eq!(
        server
            .anonymous
            .account()
            .deletion()
            .status()
            .await
            .unwrap()
            .completed_at
            .as_deref(),
        Some("2026-11-15T00:00:00Z")
    );
    let mut missing = receipt();
    missing.as_object_mut().unwrap().remove("completed_at");
    server.expect(expected("GET", 200, missing));
    assert!(matches!(
        server.anonymous.account().deletion().status().await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}
