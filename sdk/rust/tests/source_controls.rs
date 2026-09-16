#[path = "source_controls/cancellation.rs"]
mod cancellation;
#[path = "source_controls/fixture.rs"]
mod fixture;
#[path = "support/wire.rs"]
#[allow(dead_code)]
mod wire;
use cma_sdk::{Error, IdempotencyKey, bridges::*};
use fixture::*;
use serde_json::{Value, json};
use wire::WireServer;

#[tokio::test]
async fn observes_absence_current_consent_and_revocation_without_implicit_commands() {
    let server = WireServer::new().await;
    for policy in [Value::Null, receipt()["result"].clone(), {
        let mut p = receipt()["result"].clone();
        p["enabled"] = json!(false);
        p["revision"] = json!(256);
        p
    }] {
        let state = json!({"policy":policy});
        expect(&server, "GET", None, state.clone(), 200);
        assert_eq!(
            server
                .client
                .bridges()
                .read_source_controls(&grant())
                .await
                .unwrap(),
            serde_json::from_value::<BridgeSourceControlState>(state).unwrap()
        );
    }
    assert_eq!(server.request_count(), 3);
    server.assert_drained();
}
#[tokio::test]
async fn receipt_is_exact_and_reserved_final_revision_only_revokes() {
    let server = WireServer::new().await;
    expect(&server, "PUT", Some(json!(request())), receipt(), 200);
    let accepted = server
        .client
        .bridges()
        .set_source_controls(&grant(), &request(), &key())
        .await
        .unwrap();
    assert_eq!(accepted, serde_json::from_value(receipt()).unwrap());
    let mut revoke = request();
    revoke.expected_policy_revision = 255;
    revoke.expected_grant_revision = 2;
    revoke.action = BridgeSourceControlAction::RevokeSenderControls;
    let mut result = receipt();
    result["request"] = json!(revoke);
    result["result"]["revision"] = json!(256);
    result["result"]["enabled"] = json!(false);
    expect(&server, "PUT", Some(json!(revoke)), result.clone(), 200);
    assert_eq!(
        server
            .client
            .bridges()
            .set_source_controls(&grant(), &revoke, &key())
            .await
            .unwrap(),
        serde_json::from_value(result).unwrap()
    );
    server.assert_drained();
}
#[tokio::test]
async fn rejects_mixed_receipts_private_fields_and_missing_nullable_policy() {
    let server = WireServer::new().await;
    for (pointer, value) in [
        ("/id", json!("bad")),
        ("/request/expected_bridge_revision", json!(99)),
        ("/request/expected_grant_revision", json!(2)),
        ("/request/expected_policy_revision", json!(1)),
        ("/request/action", json!("revoke_sender_controls")),
        (
            "/result/grant_id",
            json!(format!("brinv_{}", "a".repeat(64))),
        ),
        ("/result/id", json!("bad")),
        ("/result/revision", json!(0)),
        ("/result/revision", json!(256)),
        ("/result/enabled", json!(false)),
        ("/result/pin/config_revision", json!(257)),
        ("/result/updated_at", json!("")),
        ("/result/updated_by/subject", json!("")),
        ("/result/updated_by/delegation_id", json!("delegated")),
        ("/result/target/agent_creation_fingerprint", json!("bad")),
        ("/result/target/worktree_identity_digest", json!("bad")),
        ("/result/target/ids/agent_id", json!("")),
        ("/result/target/sandbox_created_at", json!("")),
        ("/result/scope", json!("all_senders")),
        ("/result/approval_mode", json!("channel")),
    ] {
        let mut bad = receipt();
        *bad.pointer_mut(pointer).unwrap() = value;
        expect(&server, "PUT", Some(json!(request())), bad, 200);
        assert!(
            matches!(
                server
                    .client
                    .bridges()
                    .set_source_controls(&grant(), &request(), &key())
                    .await,
                Err(Error::Protocol { .. })
            ),
            "{pointer}"
        );
    }
    for pointer in [
        "",
        "/request",
        "/result",
        "/result/updated_by",
        "/result/owner",
        "/result/pin",
        "/result/target",
        "/result/target/ids",
    ] {
        let mut bad = receipt();
        bad.pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("private_evidence".into(), json!({}));
        expect(&server, "PUT", Some(json!(request())), bad, 200);
        assert!(
            matches!(
                server
                    .client
                    .bridges()
                    .set_source_controls(&grant(), &request(), &key())
                    .await,
                Err(Error::Protocol { .. })
            ),
            "{pointer}"
        );
    }
    for state in [
        json!({}),
        json!({"policy":receipt()["result"],"extra":true}),
    ] {
        expect(&server, "GET", None, state, 200);
        assert!(matches!(
            server.client.bridges().read_source_controls(&grant()).await,
            Err(Error::Protocol { .. })
        ));
    }
    for status in [201, 202] {
        expect(&server, "PUT", Some(json!(request())), receipt(), status);
        assert!(matches!(
            server
                .client
                .bridges()
                .set_source_controls(&grant(), &request(), &key())
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}
#[tokio::test]
async fn local_bounds_and_invalid_keys_never_reach_transport() {
    let server = WireServer::new().await;
    for (field, value) in [
        ("expected_bridge_revision", 0),
        ("expected_bridge_revision", u64::MAX),
        ("expected_grant_revision", 0),
        ("expected_grant_revision", 3),
        ("expected_policy_revision", 255),
        ("expected_policy_revision", 256),
    ] {
        let mut body = json!(request());
        body[field] = json!(value);
        let request = serde_json::from_value(body).unwrap();
        assert!(matches!(
            server
                .client
                .bridges()
                .set_source_controls(&grant(), &request, &key())
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert!(matches!(
        server.client.bridges().read_source_controls("bad").await,
        Err(Error::Validation { .. })
    ));
    assert!(matches!(
        server
            .client
            .bridges()
            .set_source_controls(
                &grant(),
                &request(),
                &IdempotencyKey::new("x".repeat(201)).unwrap()
            )
            .await,
        Err(Error::Validation { .. })
    ));
    assert_eq!(server.request_count(), 0);
    server.assert_drained();
}
#[tokio::test]
async fn conflict_and_unavailable_do_not_retry_and_explicit_recovery_keeps_original_consent() {
    for status in [409, 503] {
        let server = WireServer::new().await;
        expect(
            &server,
            "PUT",
            Some(json!(request())),
            json!({"type":"about:blank","title":"Unavailable","status":status,"detail":"Keep original body","code":"source_control_conflict"}),
            status,
        );
        assert!(matches!(
            server
                .client
                .bridges()
                .set_source_controls(&grant(), &request(), &key())
                .await,
            Err(Error::Api { .. })
        ));
        assert_eq!(server.request_count(), 1);
        expect(&server, "PUT", Some(json!(request())), receipt(), 200);
        assert_eq!(
            server
                .client
                .bridges()
                .set_source_controls(&grant(), &request(), &key())
                .await
                .unwrap(),
            serde_json::from_value(receipt()).unwrap()
        );
        assert_eq!(server.request_count(), 2);
        server.assert_drained();
    }
}
