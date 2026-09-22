#[path = "support/wire.rs"]
#[allow(dead_code)]
mod wire;
use cma_sdk::{Error, PageOptions, bridges::BridgePartitionDetail};
use serde_json::{Value, json};
use wire::WireServer;

fn detail() -> Value {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../api/crates/layer1-public/tests/fixtures/public-contract/bridge-partitions.json"
    ))
    .unwrap();
    corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "partition-owner-detail")
        .unwrap()["response"]["body"]
        .clone()
}

#[tokio::test]
async fn pending_previews_reject_leaked_content_duplicate_commands_and_scope_changes() {
    let server = WireServer::new().await;
    let bridge = "brg_pending";
    let allocation = format!("bral_{}", "a".repeat(64));
    let path = format!("/api/v1/bridges/{bridge}/partitions/{allocation}/pending-messages");
    let value = json!({
        "status":"available", "bridge_id":bridge, "partition_id":allocation,
        "generation":1, "agent_id":"agt_pending", "through_sequence":42,
        "messages":[{"command_id":"cmd_one", "response_id":"resp_one",
            "agent_order":1, "admitted_at":"2026-09-18T03:04:20Z",
            "platform":"telegram", "text":"Incoming message", "text_truncated":false}],
        "pending_truncated":false, "source_unavailable":false, "observed_at_ms":1000
    });
    server.expect(wire::json("GET", &path, None, 200, value.clone(), true));
    server
        .client
        .bridges()
        .pending_messages(bridge, &allocation)
        .await
        .unwrap();
    for (pointer, invalid) in [
        ("/bridge_id", json!("brg_other")),
        ("/through_sequence", json!(-1)),
        ("/messages/0/text", json!("😀".repeat(1025))),
        (
            "/messages",
            json!([value["messages"][0], value["messages"][0]]),
        ),
    ] {
        let mut bad = value.clone();
        *bad.pointer_mut(pointer).unwrap() = invalid;
        server.expect(wire::json("GET", &path, None, 200, bad, true));
        assert!(matches!(
            server
                .client
                .bridges()
                .pending_messages(bridge, &allocation)
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    server.expect(wire::json(
        "GET",
        &path,
        None,
        200,
        json!({"status":"redacted", "messages":value["messages"]}),
        true,
    ));
    assert!(matches!(
        server
            .client
            .bridges()
            .pending_messages(bridge, &allocation)
            .await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}
#[tokio::test]
async fn partition_gets_are_exact_read_only_and_reject_inconsistent_redaction() {
    let server = WireServer::new().await;
    let value = detail();
    let bridge = value["partition"]["bridge_id"].as_str().unwrap();
    let id = value["partition"]["id"].as_str().unwrap();
    let path = format!("/api/v1/bridges/{bridge}/partitions/{id}");
    server.expect(wire::json("GET", &path, None, 200, value.clone(), true));
    let read = server
        .client
        .bridges()
        .read_partition(bridge, id)
        .await
        .unwrap();
    assert_eq!(
        read,
        serde_json::from_value::<BridgePartitionDetail>(value.clone()).unwrap()
    );
    for (pointer, invalid) in [
        ("/activity", json!({"status":"redacted"})),
        ("/partition/generation", json!(0)),
        (
            "/partition/source",
            json!({"status":"unavailable","text":"private"}),
        ),
    ] {
        let mut bad = value.clone();
        *bad.pointer_mut(pointer).unwrap() = invalid;
        server.expect(wire::json("GET", &path, None, 200, bad, true));
        assert!(
            matches!(
                server.client.bridges().read_partition(bridge, id).await,
                Err(Error::Protocol { .. })
            ),
            "{pointer}"
        );
    }
    let calls = server.request_count();
    assert!(matches!(
        server
            .client
            .bridges()
            .list_partitions(
                bridge,
                &PageOptions {
                    limit: Some(26),
                    ..Default::default()
                }
            )
            .await,
        Err(Error::Validation { .. })
    ));
    assert!(matches!(
        server
            .client
            .bridges()
            .list_partitions(
                bridge,
                &PageOptions {
                    cursor: Some("invalid".into()),
                    ..Default::default()
                }
            )
            .await,
        Err(Error::Validation { .. })
    ));
    assert_eq!(server.request_count(), calls);
    server.assert_drained();
}
