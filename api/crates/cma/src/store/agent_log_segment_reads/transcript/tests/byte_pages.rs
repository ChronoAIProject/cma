use super::*;

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn large_event_pages_shorten_without_losing_or_repeating_source_rows() {
    let mut kinds = vec![
        AgentLogKind::ConversationOpened {
            conversation_id: "native".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "owner\n\"多".repeat(2048),
        },
    ];
    for index in 0..24 {
        kinds.push(AgentLogKind::AgentMessage {
            content: format!("{index}:{}", "message\n\"多".repeat(24 * 1024)),
        });
    }
    kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
    let (db, store, agent, entries) = custom(kinds, 1, None).await;
    assert!(serde_json::to_vec(&entries).unwrap().len() > MAX_BYTES);
    let mut after = 0;
    let mut events = Vec::new();
    loop {
        let page = store
            .agent_log_page_projection(&agent, after, 200)
            .await
            .unwrap()
            .unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_BYTES);
        let Some(last) = page.last() else { break };
        assert!(last.seq > after);
        after = last.seq;
        events.extend(page);
    }
    assert_eq!(events, entries);
    db.drop().await.unwrap();
}
