#[path = "workspace_cleanup/cancellation.rs"]
mod cancellation;
#[path = "workspace_cleanup/fixture.rs"]
mod fixture;
#[path = "support/wire.rs"]
#[allow(dead_code)]
mod wire;
use cma_sdk::{Error, bridges::*};
use fixture::*;
use serde_json::{Value, json};
use wire::WireServer;

#[tokio::test]
async fn cleanup_retains_exact_scope_and_adopted_observation_without_claiming_removal() {
    let server = WireServer::new().await;
    expect(&server, "POST", status(), 202);
    let accepted = server
        .client
        .bridges()
        .request_workspace_cleanup(BRIDGE, &allocation(), &request(), &key())
        .await
        .unwrap();
    let exact: BridgeWorkspaceCleanupStatus = serde_json::from_value(status()).unwrap();
    assert_eq!(accepted, exact);
    assert_eq!(accepted.progress, BridgeWorkspaceCleanupProgress::Requested);
    assert!(!accepted.workspace_capacity_released);
    assert!(accepted.observed_at_ms.unwrap() < accepted.receipt.as_ref().unwrap().accepted_at_ms);
    let mut done = status();
    done["progress"] = json!("complete");
    done["holds"] = json!([]);
    done["workspace_capacity_released"] = json!(true);
    expect(&server, "GET", done.clone(), 200);
    assert_eq!(
        server
            .client
            .bridges()
            .read_workspace_cleanup(BRIDGE, &allocation())
            .await
            .unwrap(),
        serde_json::from_value(done).unwrap()
    );
    let mut before = status();
    before["progress"] = json!("not_requested");
    before["receipt"] = Value::Null;
    before["holds"] = json!([]);
    before["observed_at_ms"] = Value::Null;
    expect(&server, "GET", before, 200);
    assert!(
        server
            .client
            .bridges()
            .read_workspace_cleanup(BRIDGE, &allocation())
            .await
            .unwrap()
            .receipt
            .is_none()
    );
    assert_eq!(server.request_count(), 3);
    server.assert_drained();
}

#[tokio::test]
async fn cleanup_refuses_mismatched_receipts_secret_fields_and_contradictory_progress() {
    let server = WireServer::new().await;
    for (pointer, value) in [
        ("/bridge_id", json!("another")),
        ("/allocation_id", json!(format!("bral_{}", "3".repeat(64)))),
        ("/target/agent_id", json!("replacement")),
        ("/generation", json!(0)),
        ("/receipt/generation", json!(2)),
        ("/receipt/request/expected_bridge_revision", json!(99)),
        ("/receipt/request/expected_allocation_revision", json!(5)),
        ("/receipt/request/expected_workspace_revision", json!(5)),
        (
            "/receipt/request/child_cleanup_receipt_id",
            json!(format!("bclj_{}", "9".repeat(64))),
        ),
        ("/workspace_revision", Value::Null),
        ("/child_cleanup_receipt_id", Value::Null),
        ("/eligible", json!(true)),
        ("/receipt/id", json!("wrong")),
        ("/receipt/requested_by/delegation_id", json!("delegated")),
        ("/receipt/accepted_at_ms", json!(-1)),
        ("/observed_at_ms", Value::Null),
        (
            "/holds",
            json!(["workspace_busy", "child_cleanup_incomplete"]),
        ),
        ("/holds", json!(["workspace_busy", "workspace_busy"])),
        ("/progress", json!("complete")),
        ("/progress", json!("invented")),
        ("/parent_retained", json!(false)),
        ("/history_retained", json!(false)),
        ("/workspace_capacity_released", json!(true)),
        ("/receipt", Value::Null),
    ] {
        let mut bad = status();
        *bad.pointer_mut(pointer).unwrap() = value;
        expect(&server, "POST", bad, 202);
        assert!(
            matches!(
                server
                    .client
                    .bridges()
                    .request_workspace_cleanup(BRIDGE, &allocation(), &request(), &key())
                    .await,
                Err(Error::Protocol { .. })
            ),
            "{pointer}"
        );
    }
    for pointer in [
        "",
        "/receipt",
        "/receipt/request",
        "/receipt/requested_by",
        "/target",
    ] {
        let mut bad = status();
        bad.pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("private_evidence".into(), json!({}));
        expect(&server, "POST", bad, 202);
        assert!(matches!(
            server
                .client
                .bridges()
                .request_workspace_cleanup(BRIDGE, &allocation(), &request(), &key())
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    for field in [
        "receipt",
        "observed_at_ms",
        "workspace_revision",
        "child_cleanup_receipt_id",
    ] {
        let mut bad = status();
        bad.as_object_mut().unwrap().remove(field);
        expect(&server, "GET", bad, 200);
        assert!(matches!(
            server
                .client
                .bridges()
                .read_workspace_cleanup(BRIDGE, &allocation())
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    expect(&server, "POST", status(), 200);
    assert!(matches!(
        server
            .client
            .bridges()
            .request_workspace_cleanup(BRIDGE, &allocation(), &request(), &key())
            .await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn invalid_key_revision_allocation_and_limits_never_issue_http() {
    let server = WireServer::new().await;
    for bad in ["", "bral_short", "bral_INVALID"] {
        assert!(matches!(
            server
                .client
                .bridges()
                .read_workspace_cleanup(BRIDGE, bad)
                .await,
            Err(Error::Validation { .. })
        ));
    }
    for revision in [0, u64::MAX] {
        let mut bad = request();
        bad.expected_bridge_revision = revision;
        assert!(matches!(
            server
                .client
                .bridges()
                .request_workspace_cleanup(BRIDGE, &allocation(), &bad, &key())
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert!(matches!(
        server
            .client
            .bridges()
            .request_workspace_cleanup(
                BRIDGE,
                &allocation(),
                &request(),
                &cma_sdk::IdempotencyKey::new("x".repeat(201)).unwrap()
            )
            .await,
        Err(Error::Validation { .. })
    ));
    for (workspace, child) in [
        (0, format!("bclj_{}", "3".repeat(64))),
        (3, "invalid".into()),
    ] {
        let mut bad = request();
        bad.expected_workspace_revision = workspace;
        bad.child_cleanup_receipt_id = child;
        assert!(matches!(
            server
                .client
                .bridges()
                .request_workspace_cleanup(BRIDGE, &allocation(), &bad, &key())
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert_eq!(server.request_count(), 0);
}

#[tokio::test]
async fn workspace_eligibility_requires_reviewed_inputs_but_historical_replay_survives_runtime_change()
 {
    let server = WireServer::new().await;
    let mut eligible = status();
    eligible["receipt"] = Value::Null;
    eligible["progress"] = json!("not_requested");
    eligible["holds"] = json!([]);
    eligible["eligible"] = json!(true);
    expect(&server, "GET", eligible.clone(), 200);
    assert!(
        server
            .client
            .bridges()
            .read_workspace_cleanup(BRIDGE, &allocation())
            .await
            .unwrap()
            .eligible
    );
    for (field, value) in [
        ("workspace_revision", Value::Null),
        ("child_cleanup_receipt_id", Value::Null),
        ("runtime_supported", json!(false)),
        ("holds", json!(["runtime_unsupported"])),
    ] {
        let mut bad = eligible.clone();
        bad[field] = value;
        expect(&server, "GET", bad, 200);
        assert!(matches!(
            server
                .client
                .bridges()
                .read_workspace_cleanup(BRIDGE, &allocation())
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    for progress in [
        "requested",
        "admission_closed",
        "removal_started",
        "runtime_removed",
        "complete",
    ] {
        let mut retained = status();
        retained["workspace_revision"] = json!(8);
        retained["runtime_supported"] = json!(false);
        retained["progress"] = json!(progress);
        retained["holds"] = json!([]);
        retained["workspace_capacity_released"] = json!(progress == "complete");
        expect(&server, "POST", retained.clone(), 202);
        assert_eq!(
            server
                .client
                .bridges()
                .request_workspace_cleanup(BRIDGE, &allocation(), &request(), &key())
                .await
                .unwrap(),
            serde_json::from_value(retained).unwrap()
        );
    }
    server.assert_drained();
}
