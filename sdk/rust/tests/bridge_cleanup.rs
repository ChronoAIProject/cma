#[path = "bridge_cleanup/cancellation.rs"]
mod cancellation;
#[path = "bridge_cleanup/fixture.rs"]
mod fixture;
#[path = "support/wire.rs"]
#[allow(dead_code)]
mod wire;
use cma_sdk::{Error, PageOptions, bridges::*};
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
        .request_child_cleanup(BRIDGE, &allocation(), &request(), &key())
        .await
        .unwrap();
    let exact: BridgeChildCleanupStatus = serde_json::from_value(status()).unwrap();
    assert_eq!(accepted, exact);
    assert_eq!(accepted.progress, BridgeChildCleanupProgress::Requested);
    assert!(!accepted.agent_capacity_released);
    assert!(accepted.observed_at_ms.unwrap() < accepted.receipt.as_ref().unwrap().accepted_at_ms);
    let mut done = status();
    done["progress"] = json!("complete");
    done["holds"] = json!([]);
    done["agent_capacity_released"] = json!(true);
    expect(&server, "GET", done.clone(), 200);
    assert_eq!(
        server
            .client
            .bridges()
            .read_child_cleanup(BRIDGE, &allocation())
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
            .read_child_cleanup(BRIDGE, &allocation())
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
        ("/receipt/id", json!("wrong")),
        ("/receipt/requested_by/delegation_id", json!("delegated")),
        ("/receipt/accepted_at_ms", json!(-1)),
        ("/observed_at_ms", Value::Null),
        ("/holds", json!(["uncertain_reply", "pending_command"])),
        ("/holds", json!(["uncertain_reply", "uncertain_reply"])),
        ("/progress", json!("complete")),
        ("/progress", json!("invented")),
        ("/workspace_retained", json!(false)),
        ("/history_retained", json!(false)),
        ("/agent_capacity_released", json!(true)),
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
                    .request_child_cleanup(BRIDGE, &allocation(), &request(), &key())
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
                .request_child_cleanup(BRIDGE, &allocation(), &request(), &key())
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    for field in ["receipt", "observed_at_ms"] {
        let mut bad = status();
        bad.as_object_mut().unwrap().remove(field);
        expect(&server, "GET", bad, 200);
        assert!(matches!(
            server
                .client
                .bridges()
                .read_child_cleanup(BRIDGE, &allocation())
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    expect(&server, "POST", status(), 200);
    assert!(matches!(
        server
            .client
            .bridges()
            .request_child_cleanup(BRIDGE, &allocation(), &request(), &key())
            .await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn allocation_cursor_remains_opaque_and_pages_keep_exact_bridge_order_and_limit() {
    let server = WireServer::new().await;
    let opaque = "opaque/+/v1?part=1";
    let path = format!("/api/v1/bridges/{BRIDGE}/allocations?limit=1");
    let page = json!({"items":[row()],"next_cursor":opaque});
    server.expect(wire::json("GET", &path, None, 200, page.clone(), true));
    assert_eq!(
        server
            .client
            .bridges()
            .list_allocations(
                BRIDGE,
                &PageOptions {
                    cursor: None,
                    limit: Some(1)
                }
            )
            .await
            .unwrap(),
        serde_json::from_value(page).unwrap()
    );
    let query =
        reqwest::Url::parse_with_params("http://fixture", [("cursor", opaque), ("limit", "1")])
            .unwrap()
            .query()
            .unwrap()
            .to_owned();
    let path = format!("/api/v1/bridges/{BRIDGE}/allocations?{query}");
    let opts = PageOptions {
        cursor: Some(opaque.into()),
        limit: Some(1),
    };
    server.expect(wire::json(
        "GET",
        &path,
        None,
        200,
        json!({"items":[],"next_cursor":null}),
        true,
    ));
    assert!(
        server
            .client
            .bridges()
            .list_allocations(BRIDGE, &opts)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    for items in [json!([]), json!([row()])] {
        let continued = json!({"items": items, "next_cursor": "another-page"});
        let path = format!("/api/v1/bridges/{BRIDGE}/allocations?limit=2");
        server.expect(wire::json("GET", &path, None, 200, continued.clone(), true));
        assert_eq!(
            server
                .client
                .bridges()
                .list_allocations(
                    BRIDGE,
                    &PageOptions {
                        cursor: None,
                        limit: Some(2)
                    }
                )
                .await
                .unwrap(),
            serde_json::from_value(continued).unwrap()
        );
    }
    let mut other = row();
    other["bridge_id"] = json!("another");
    for bad in [
        json!({"items":[row()],"next_cursor":opaque}),
        json!({"items":[],"next_cursor":""}),
        json!({"items":[],"next_cursor":"x".repeat(1025)}),
        json!({"items":[row(),row()],"next_cursor":null}),
        json!({"items":[other],"next_cursor":null}),
    ] {
        server.expect(wire::json("GET", &path, None, 200, bad, true));
        assert!(matches!(
            server
                .client
                .bridges()
                .list_allocations(BRIDGE, &opts)
                .await,
            Err(Error::Protocol { .. })
        ));
    }
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
                .read_child_cleanup(BRIDGE, bad)
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
                .request_child_cleanup(BRIDGE, &allocation(), &bad, &key())
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert!(matches!(
        server
            .client
            .bridges()
            .request_child_cleanup(
                BRIDGE,
                &allocation(),
                &request(),
                &cma_sdk::IdempotencyKey::new("x".repeat(201)).unwrap()
            )
            .await,
        Err(Error::Validation { .. })
    ));
    for limit in [0, 101] {
        assert!(matches!(
            server
                .client
                .bridges()
                .list_allocations(
                    BRIDGE,
                    &PageOptions {
                        cursor: None,
                        limit: Some(limit)
                    }
                )
                .await,
            Err(Error::Validation { .. })
        ));
    }
    for cursor in [String::new(), "x".repeat(1025)] {
        assert!(matches!(
            server
                .client
                .bridges()
                .list_allocations(
                    BRIDGE,
                    &PageOptions {
                        cursor: Some(cursor),
                        limit: None
                    }
                )
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert_eq!(server.request_count(), 0);
}
