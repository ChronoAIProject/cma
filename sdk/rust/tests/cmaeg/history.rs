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
async fn native_thread_empty_and_mixed_scan_pages_remain_single_explicit_reads() {
    let server = WireServer::new().await;
    let client = client(&server);
    let fixtures: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../api/crates/cmaeg/tests/fixtures/sdk-contract.json"
    ))
    .unwrap();
    for index in [2, 3] {
        let page = fixtures["ConversationHistoryPage"][index].clone();
        server.expect(wire::json(
            "GET",
            if index == 3 {
                "/v1/events/original/history?include_media=true"
            } else {
                "/v1/events/original/history"
            },
            None,
            200,
            page.clone(),
            true,
        ));
        let decoded = if index == 3 {
            client
                .read_event_history_with_media("original", None)
                .await
                .unwrap()
        } else {
            client.read_event_history("original", None).await.unwrap()
        };
        if index == 2 {
            assert!(decoded.messages.is_empty());
            assert_eq!(decoded.scanned_messages, 25);
        } else {
            assert_eq!(
                decoded.messages[0].text,
                "Earlier postText before and after omitted media"
            );
            assert!(
                decoded.messages[0].omitted_content
                    == [cma_sdk::cmaeg::types::HistoryOmittedContent::Media]
            );
            assert!(decoded.messages[0].attachments.is_empty());
        }
        same(decoded, page);
        server.assert_drained();
    }
}

#[tokio::test]
async fn mixed_history_opt_in_is_repeated_with_the_exact_opaque_cursor() {
    let server = WireServer::new().await;
    let client = client(&server);
    server.expect(wire::json(
        "GET",
        "/v1/events/original/history?cursor=opaque%2F%2B%3Fcursor&include_media=true",
        None,
        200,
        sample("ConversationHistoryPage"),
        true,
    ));
    client
        .read_event_history_with_media("original", Some("opaque/+?cursor"))
        .await
        .unwrap();
    server.assert_drained();
}
