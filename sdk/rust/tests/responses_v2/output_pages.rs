use super::wire::{Expected, WireServer, headers};
use cma_sdk::Error;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn page() -> Value {
    let output = super::fixture::events().last().unwrap()["response"]["output"].clone();
    json!({"object":"cma.response.output_page", "response_id":"resp_contract",
        "data":output,"has_more":true,"next_after":"indexed:8","through":12000})
}

fn expect(server: &WireServer, body: Value) {
    server.expect(Expected {
        method: "GET".into(),
        path: "/api/v2/responses/resp_contract/output?after=indexed%3A7".into(),
        headers: headers(true, "application/json"),
        status: 200,
        response_headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
        response: serde_json::to_vec(&body).unwrap(),
        ..Expected::default()
    });
}

#[tokio::test]
async fn output_page_preserves_opaque_cursor_without_submitting_input() {
    let server = WireServer::new().await;
    expect(&server, page());
    let result = server
        .client
        .responses_v2()
        .read_output_page("resp_contract", Some("indexed:7"))
        .await
        .unwrap();
    assert_eq!(result.data.len(), 1);
    assert_eq!(result.next_after.as_deref(), Some("indexed:8"));
    assert_eq!(result.through, Some(12000));
    assert_eq!(server.request_count(), 1);
    server.assert_drained();
}

#[tokio::test]
async fn output_page_refuses_foreign_identity_and_nonprogressing_continuation() {
    let server = WireServer::new().await;
    for (key, value) in [
        ("response_id", json!("resp_foreign")),
        ("next_after", Value::Null),
        ("has_more", json!(false)),
        ("next_after", json!("indexed:7")),
        ("next_after", json!("")),
        ("data", json!([])),
    ] {
        let mut changed = page();
        changed[key] = value;
        expect(&server, changed);
        assert!(matches!(
            server
                .client
                .responses_v2()
                .read_output_page("resp_contract", Some("indexed:7"))
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    assert_eq!(server.request_count(), 6);
    server.assert_drained();
}
