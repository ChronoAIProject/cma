use super::*;

#[tokio::test]
async fn conversation_history_requires_explicit_cursor_and_never_auto_drains() {
    let server = WireServer::new().await;
    let client = client(&server);
    for (cursor, path) in [
        (None, "/v1/events/original/history"),
        (
            Some("opaque/+?cursor"),
            "/v1/events/original/history?cursor=opaque%2F%2B%3Fcursor",
        ),
    ] {
        server.expect(wire::json(
            "GET",
            path,
            None,
            200,
            sample("ConversationHistoryPage"),
            true,
        ));
        same(
            client.read_event_history("original", cursor).await.unwrap(),
            sample("ConversationHistoryPage"),
        );
        server.assert_drained();
    }
}

#[tokio::test]
async fn native_thread_empty_scan_page_remains_a_single_explicit_read() {
    let server = WireServer::new().await;
    let client = client(&server);
    let fixtures: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json"
    ))
    .unwrap();
    let page = fixtures["ConversationHistoryPage"][2].clone();
    assert_eq!(page["scanned_messages"], 25);
    server.expect(wire::json(
        "GET",
        "/v1/events/original/history",
        None,
        200,
        page.clone(),
        true,
    ));
    same(
        client.read_event_history("original", None).await.unwrap(),
        page,
    );
    server.assert_drained();
}
