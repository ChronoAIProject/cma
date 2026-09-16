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
