#[path = "bridge_invokes/cancellation.rs"]
mod cancellation;
#[path = "bridge_invokes/preflight.rs"]
mod preflight;
#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{Error, IdempotencyKey, PageOptions, bridges::*};
use serde_json::{Value, json};
use wire::WireServer;

fn grant_id() -> String {
    format!("brinv_{}", "c".repeat(64))
}
fn actor() -> Value {
    json!({"kind":"api","subject":"owner","delegation_id":null})
}
fn target() -> Value {
    json!({"sandbox_id":"sbx-1","workspace_id":"wks-1","worktree_id":"wkt-1","agent_id":"agt-1"})
}
fn grant() -> Value {
    json!({"id":grant_id(),"revision":1,"owner":{"kind":"person","subject":"owner"},
        "pin":{"bridge_id":"brg-1","config_revision":1},
        "target":{"ids":target(),"sandbox_created_at":"2026-09-05T00:00:00Z","workspace_created_at":"2026-09-05T00:00:00Z",
            "agent_created_at":"2026-09-05T00:00:00Z","agent_creation_fingerprint":"d".repeat(64),"worktree_identity_digest":"e".repeat(64)},
        "consent":"use_retained_agent_context","issued_at":"2026-09-15T00:00:00Z","issued_by":actor(),"state":{"kind":"active"}})
}
fn receipt(revoked: bool) -> Value {
    let mut grant = grant();
    if revoked {
        grant["revision"] = json!(2);
        grant["state"] = json!({"kind":"revoked","at":"2026-09-15T01:00:00Z","actor":actor()});
    }
    json!({"id":"a".repeat(64),"request_digest":"b".repeat(64),"bridge_id":"brg-1","actor":actor(),"result":grant})
}
fn issue() -> IssueBridgeInvokeGrantRequest {
    IssueBridgeInvokeGrantRequest {
        expected_bridge_revision: 1,
        target: serde_json::from_value(target()).unwrap(),
        shared_context: BridgeSharedContextConsent::UseRetainedAgentContext,
    }
}
fn revoke() -> RevokeBridgeInvokeGrantRequest {
    RevokeBridgeInvokeGrantRequest {
        expected_bridge_revision: 1,
        expected_revision: 1,
    }
}
fn key() -> IdempotencyKey {
    IdempotencyKey::new("explicit-consent-command").unwrap()
}
fn expect(
    server: &WireServer,
    method: &str,
    path: &str,
    body: Option<Value>,
    response: Value,
    status: u16,
) {
    let mut expected = wire::json(method, path, body.clone(), status, response, true);
    if body.is_some() {
        expected
            .headers
            .insert("idempotency-key".into(), key().as_str().into());
    }
    server.expect(expected);
}

#[tokio::test]
async fn safe_grant_detail_rejects_bad_identity_lineage_attribution_and_forbidden_fields() {
    let server = WireServer::new().await;
    let _ = (&server.base_url, &server.anonymous);
    let path = format!("/api/v1/bridge-invoke-grants/{}", grant_id());
    for (pointer, value) in [
        ("/id", json!(format!("brinv_{}", "a".repeat(64)))),
        ("/id", json!(format!("brinv_{}", "C".repeat(64)))),
        ("/revision", json!(2)),
        ("/owner/subject", json!("")),
        ("/pin/bridge_id", json!("bad bridge")),
        ("/pin/config_revision", json!(257)),
        ("/issued_at", json!("")),
        (
            "/issued_by",
            json!({"kind":"api","subject":"owner","delegation_id":"delegate"}),
        ),
        ("/issued_by/subject", json!("bad subject")),
        ("/target/ids/agent_id", json!("")),
        ("/target/agent_creation_fingerprint", json!("D".repeat(64))),
        ("/target/worktree_identity_digest", json!("e".repeat(63))),
        ("/target/workspace_created_at", json!("\n")),
        (
            "/state",
            json!({"kind":"revoked","at":"2026-09-15T00:00:00Z","actor":actor()}),
        ),
        ("/state", json!({"kind":"active","proof":"forbidden"})),
    ] {
        let mut response = grant();
        *response.pointer_mut(pointer).unwrap() = value;
        expect(&server, "GET", &path, None, response, 200);
        assert!(
            matches!(
                server.client.bridges().read_invoke_grant(&grant_id()).await,
                Err(Error::Protocol { .. })
            ),
            "{pointer}"
        );
    }
    for field in [
        "config",
        "custody",
        "resource_audit",
        "execution_proof",
        "connector",
    ] {
        let mut response = grant();
        response[field] = json!({"retained":"forbidden"});
        expect(&server, "GET", &path, None, response, 200);
        assert!(matches!(
            server.client.bridges().read_invoke_grant(&grant_id()).await,
            Err(Error::Protocol { .. })
        ));
    }
    expect(&server, "GET", &path, None, grant(), 200);
    assert_eq!(
        server
            .client
            .bridges()
            .read_invoke_grant(&grant_id())
            .await
            .unwrap()
            .revision,
        1
    );
    server.assert_drained();
}

#[tokio::test]
async fn readiness_requires_an_active_candidate_for_the_exact_current_configuration() {
    let server = WireServer::new().await;
    let ready = json!({"bridge_id":"brg-1","revision":2,"config_revision":1,"status":"consent_present","candidate":grant()});
    for (pointer, value) in [
        ("/bridge_id", json!("brg-other")),
        ("/revision", json!(0)),
        ("/revision", json!(u64::MAX)),
        ("/config_revision", json!(3)),
        ("/candidate", Value::Null),
        ("/status", json!("consent_required")),
        ("/candidate/pin/bridge_id", json!("brg-other")),
        ("/candidate/pin/config_revision", json!(2)),
        ("/candidate", receipt(true)["result"].clone()),
    ] {
        let mut response = ready.clone();
        *response.pointer_mut(pointer).unwrap() = value;
        expect(
            &server,
            "GET",
            "/api/v1/bridges/brg-1/invoke-readiness",
            None,
            response,
            200,
        );
        assert!(
            matches!(
                server.client.bridges().invoke_readiness("brg-1").await,
                Err(Error::Protocol { .. })
            ),
            "{pointer}"
        );
    }
    for response in [
        ready,
        json!({"bridge_id":"brg-1","revision":2,"config_revision":1,"status":"consent_required","candidate":null}),
        json!({"bridge_id":"brg-1","revision":2,"config_revision":1,"status":"not_applicable","candidate":null}),
    ] {
        expect(
            &server,
            "GET",
            "/api/v1/bridges/brg-1/invoke-readiness",
            None,
            response,
            200,
        );
        server
            .client
            .bridges()
            .invoke_readiness("brg-1")
            .await
            .unwrap();
    }
    server.assert_drained();
}

#[tokio::test]
async fn issue_receipt_correlates_bridge_target_consent_and_actual_manager() {
    let server = WireServer::new().await;
    for (pointer, value) in [
        ("/id", json!("a".repeat(63))),
        ("/request_digest", json!("B".repeat(64))),
        ("/actor/subject", json!("other-manager")),
        (
            "/actor",
            json!({"kind":"api","subject":"owner","delegation_id":"delegate"}),
        ),
        ("/bridge_id", json!("brg-other")),
        ("/result/target/ids/agent_id", json!("agt-other")),
        ("/result/consent", json!("implicit")),
        ("/result/pin/config_revision", json!(2)),
        ("/result", receipt(true)["result"].clone()),
    ] {
        let mut response = receipt(false);
        *response.pointer_mut(pointer).unwrap() = value;
        expect(
            &server,
            "POST",
            "/api/v1/bridges/brg-1/invoke-grants",
            Some(serde_json::to_value(issue()).unwrap()),
            response,
            201,
        );
        assert!(
            matches!(
                server
                    .client
                    .bridges()
                    .issue_invoke_grant("brg-1", &issue(), &key())
                    .await,
                Err(Error::Protocol { .. })
            ),
            "{pointer}"
        );
    }
    let mut other_bridge = receipt(false);
    other_bridge["bridge_id"] = json!("brg-other");
    other_bridge["result"]["pin"]["bridge_id"] = json!("brg-other");
    expect(
        &server,
        "POST",
        "/api/v1/bridges/brg-1/invoke-grants",
        Some(serde_json::to_value(issue()).unwrap()),
        other_bridge,
        201,
    );
    assert!(matches!(
        server
            .client
            .bridges()
            .issue_invoke_grant("brg-1", &issue(), &key())
            .await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn revoke_requires_exact_identity_successor_revision_and_revocation_actor() {
    let server = WireServer::new().await;
    let path = format!("/api/v1/bridge-invoke-grants/{}/revoke", grant_id());
    for (pointer, value) in [
        ("/result/id", json!(format!("brinv_{}", "d".repeat(64)))),
        ("/result/revision", json!(3)),
        ("/result/pin/config_revision", json!(2)),
        ("/result", grant()),
        ("/result/state/at", json!("")),
        (
            "/result/state/actor",
            json!({"kind":"api","subject":"owner","delegation_id":"delegate"}),
        ),
        ("/result/state/actor/subject", json!("different-manager")),
    ] {
        let mut response = receipt(true);
        *response.pointer_mut(pointer).unwrap() = value;
        expect(
            &server,
            "POST",
            &path,
            Some(serde_json::to_value(revoke()).unwrap()),
            response,
            200,
        );
        assert!(
            matches!(
                server
                    .client
                    .bridges()
                    .revoke_invoke_grant(&grant_id(), &revoke(), &key())
                    .await,
                Err(Error::Protocol { .. })
            ),
            "{pointer}"
        );
    }
    expect(
        &server,
        "POST",
        &path,
        Some(json!({"expected_bridge_revision":1,"expected_revision":2})),
        receipt(true),
        200,
    );
    assert!(matches!(
        server
            .client
            .bridges()
            .revoke_invoke_grant(
                &grant_id(),
                &RevokeBridgeInvokeGrantRequest {
                    expected_revision: 2,
                    ..revoke()
                },
                &key()
            )
            .await,
        Err(Error::Protocol { .. })
    ));
    let mut response = receipt(true);
    let revoker = json!({"kind":"human","subject":"new-manager"});
    response["actor"] = revoker.clone();
    response["result"]["state"]["actor"] = revoker;
    expect(
        &server,
        "POST",
        &path,
        Some(serde_json::to_value(revoke()).unwrap()),
        response,
        200,
    );
    let revoked = server
        .client
        .bridges()
        .revoke_invoke_grant(&grant_id(), &revoke(), &key())
        .await
        .unwrap();
    let original: BridgeInvokeGrant = serde_json::from_value(grant()).unwrap();
    assert_eq!(revoked.result.target, original.target);
    assert_eq!(revoked.result.pin, original.pin);
    assert_eq!(revoked.result.issued_by, original.issued_by);
    assert_eq!(revoked.result.consent, original.consent);
    assert_eq!(revoked.result.issued_at, original.issued_at);
    server.assert_drained();
}

#[tokio::test]
async fn pages_require_strict_progress_consistent_bridge_owner_and_exact_continuation() {
    let server = WireServer::new().await;
    let mut other = grant();
    other["id"] = json!(format!("brinv_{}", "d".repeat(64)));
    let mut wrong_owner = other.clone();
    wrong_owner["owner"]["subject"] = json!("other-owner");
    let mut wrong_bridge = grant();
    wrong_bridge["pin"]["bridge_id"] = json!("other-bridge");
    for response in [
        json!({"items":[grant(),grant()]}),
        json!({"items":[other,grant()]}),
        json!({"items":[grant(),wrong_owner]}),
        json!({"items":[wrong_bridge]}),
        json!({"items":[],"next_cursor":grant_id()}),
        json!({"items":[grant()],"next_cursor":format!("brinv_{}","d".repeat(64))}),
        json!({"items":vec![grant();51]}),
    ] {
        expect(
            &server,
            "GET",
            "/api/v1/bridges/brg-1/invoke-grants",
            None,
            response,
            200,
        );
        assert!(matches!(
            server
                .client
                .bridges()
                .list_invoke_grants("brg-1", &PageOptions::default())
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    let options = PageOptions {
        cursor: Some(grant_id()),
        limit: Some(1),
    };
    expect(
        &server,
        "GET",
        &format!(
            "/api/v1/bridges/brg-1/invoke-grants?cursor={}&limit=1",
            grant_id()
        ),
        None,
        json!({"items":[grant()]}),
        200,
    );
    assert!(matches!(
        server
            .client
            .bridges()
            .list_invoke_grants("brg-1", &options)
            .await,
        Err(Error::Protocol { .. })
    ));
    expect(
        &server,
        "GET",
        "/api/v1/bridges/brg-1/invoke-grants",
        None,
        json!({"items":[receipt(true)["result"]],"next_cursor":grant_id()}),
        200,
    );
    assert_eq!(
        server
            .client
            .bridges()
            .list_invoke_grants("brg-1", &PageOptions::default())
            .await
            .unwrap()
            .next_cursor,
        Some(grant_id())
    );
    server.assert_drained();
}
