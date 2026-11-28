use super::*;

#[tokio::test]
async fn live_fragments_before_broadcast_lag_precede_the_gap_and_later_fragments() {
    let (mut open, f) = following().await;
    let mut assembly = AgentMessageAssembly::default();
    for _ in 0..300 {
        f.state
            .updates
            .as_ref()
            .unwrap()
            .record_update(
                &open.header.id,
                open.header.runtime_generation.unwrap(),
                &mut assembly,
                AgentRuntimeUpdate::Said(
                    serde_json::from_value(serde_json::json!({
                        "sessionUpdate": "agent_message_chunk", "messageId": "later",
                        "content": {"type": "text", "text": "after lag"}
                    }))
                    .unwrap(),
                ),
                "2026-11-06T00:00:00Z".into(),
            )
            .await
            .unwrap();
    }
    open.header = open
        .state
        .reads
        .event_stream_header(&open.authorization)
        .await
        .unwrap();
    let mut first = fragment(&open);
    first.delta = "before lag".into();
    open.held = Some(first.into());
    open.queue_live(false);
    open.ended = true;
    let bytes = axum::body::to_bytes(response(open).into_body(), usize::MAX)
        .await
        .unwrap();
    let wire = String::from_utf8(bytes.to_vec()).unwrap();
    let before = wire.find("before lag").unwrap();
    let gap = wire.find("event: cma.agent.live_gap\n").unwrap();
    let after = wire.find("after lag").unwrap();
    assert!(before < gap && gap < after, "{wire}");
}

#[tokio::test]
async fn serialized_pages_stop_before_byte_limit_without_acknowledging_omitted_rows() {
    let f = fixture(false, false).await;
    for _ in 0..5 {
        append(
            &f.store,
            AgentLogKind::AgentMessage {
                content: "a".repeat(1024 * 1024),
            },
        )
        .await;
    }
    let (status, page) = call(&f.router, Method::GET, SNAPSHOT, "owner-a", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(serde_json::to_vec(&page).unwrap().len() <= projection::MAX_BYTES);
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.last().unwrap()["sequence"], 5);
    let (status, next) = call(
        &f.router,
        Method::GET,
        &format!(
            "{SNAPSHOT}?page_cursor={}",
            page["next_page_cursor"].as_str().unwrap()
        ),
        "owner-a",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(next["items"][0]["sequence"], 6);
    assert!(next["next_page_cursor"].is_null());
}
