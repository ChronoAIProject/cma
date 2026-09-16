#[path = "support/wire.rs"]
#[allow(dead_code)]
mod wire;
use cma_sdk::{Error, IdempotencyKey, bridges::*};
use serde_json::{Value, json};
use wire::WireServer;
fn receipt(action: &str) -> Value {
    let id = format!("bral_{}", "1".repeat(64));
    json!({"id":format!("bpc_{}","3".repeat(64)),"bridge_id":"bridge-partition","request_sha256":"4".repeat(64),
        "actor":{"kind":"human","subject":"owner"},"action":action,"expected_bridge_revision":7,
        "allocation_id":id,"generation":1,"previous_revision":5,"result":{"id":id,"generation":1,"revision":6,"state":"retired"},
        "successor":if action=="reset"{json!({"id":format!("bral_{}","2".repeat(64)),"generation":2,"revision":1,"state":"reserved"})}else{Value::Null},"created_at_ms":200})
}
fn expect(server: &WireServer, action: &str, value: Value, status: u16) {
    let path = format!(
        "/api/v1/bridges/bridge-partition/partitions/bral_{}/{}",
        "1".repeat(64),
        action
    );
    let mut response = wire::json(
        "POST",
        &path,
        Some(json!({"expected_bridge_revision":7,"expected_revision":5})),
        status,
        value,
        true,
    );
    response.headers.insert(
        "idempotency-key".into(),
        "original-partition-command".into(),
    );
    server.expect(response);
}
#[tokio::test]
async fn partition_commands_preserve_exact_body_key_and_durable_acceptance() {
    let server = WireServer::new().await;
    let id = format!("bral_{}", "1".repeat(64));
    let body = BridgePartitionCommandRequest {
        expected_bridge_revision: 7,
        expected_revision: 5,
    };
    let key = IdempotencyKey::new("original-partition-command").unwrap();
    for action in ["reset", "detach"] {
        expect(&server, action, receipt(action), 202);
        let first = if action == "reset" {
            server
                .client
                .bridges()
                .reset_partition("bridge-partition", &id, &body, &key)
                .await
        } else {
            server
                .client
                .bridges()
                .detach_partition("bridge-partition", &id, &body, &key)
                .await
        }
        .unwrap();
        expect(&server, action, receipt(action), 202);
        let second = if action == "reset" {
            server
                .client
                .bridges()
                .reset_partition("bridge-partition", &id, &body, &key)
                .await
        } else {
            server
                .client
                .bridges()
                .detach_partition("bridge-partition", &id, &body, &key)
                .await
        }
        .unwrap();
        assert_eq!(first, second);
    }
    server.assert_drained();
}
#[tokio::test]
async fn partition_command_sdk_rejects_changed_request_result_or_unacknowledged_success() {
    let server = WireServer::new().await;
    let id = format!("bral_{}", "1".repeat(64));
    let body = BridgePartitionCommandRequest {
        expected_bridge_revision: 7,
        expected_revision: 5,
    };
    let key = IdempotencyKey::new("original-partition-command").unwrap();
    for (pointer, value) in [
        ("/allocation_id", json!(format!("bral_{}", "7".repeat(64)))),
        ("/action", json!("detach")),
        ("/expected_bridge_revision", json!(8)),
        ("/previous_revision", json!(6)),
        ("/result/revision", json!(7)),
        ("/successor/generation", json!(3)),
        ("/successor/state", json!("retired")),
        ("/successor", Value::Null),
        (
            "/actor",
            json!({"kind":"api","subject":"owner","delegation_id":"delegated"}),
        ),
    ] {
        let mut bad = receipt("reset");
        *bad.pointer_mut(pointer).unwrap() = value;
        expect(&server, "reset", bad, 202);
        assert!(
            matches!(
                server
                    .client
                    .bridges()
                    .reset_partition("bridge-partition", &id, &body, &key)
                    .await,
                Err(Error::Protocol { .. })
            ),
            "{pointer}"
        );
    }
    for pointer in ["", "/actor", "/result", "/successor"] {
        let mut bad = receipt("reset");
        bad.pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("private_evidence".into(), json!("secret"));
        expect(&server, "reset", bad, 202);
        assert!(matches!(
            server
                .client
                .bridges()
                .reset_partition("bridge-partition", &id, &body, &key)
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    let mut missing = receipt("detach");
    missing.as_object_mut().unwrap().remove("successor");
    expect(&server, "detach", missing, 202);
    assert!(matches!(
        server
            .client
            .bridges()
            .detach_partition("bridge-partition", &id, &body, &key)
            .await,
        Err(Error::Protocol { .. })
    ));
    expect(&server, "reset", receipt("reset"), 200);
    assert!(matches!(
        server
            .client
            .bridges()
            .reset_partition("bridge-partition", &id, &body, &key)
            .await,
        Err(Error::Protocol { .. })
    ));
    let count = server.request_count();
    assert!(matches!(
        server
            .client
            .bridges()
            .detach_partition("bridge-partition", "invalid", &body, &key)
            .await,
        Err(Error::Validation { .. })
    ));
    assert_eq!(server.request_count(), count);
    server.assert_drained();
}
