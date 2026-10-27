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
