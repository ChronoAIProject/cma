use super::*;
use crate::rest::agents_tests::{conversation_fixture, insert_agent, seeded_agent};
use futures_util::StreamExt;
use layer2_core::sandbox::{AgentHistoryCursor, AgentStatus};

fn caller() -> Caller {
    Caller::new(
        layer2_core::SubjectId::new("owner-a"),
        layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
    )
}

fn history(head: u64) -> Vec<AgentLogKind> {
    let mut kinds = vec![
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "original input".into(),
        },
    ];
    kinds.resize(
        head as usize - 3,
        AgentLogKind::AgentMessage {
            content: "retained output".into(),
        },
    );
    kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
    kinds
}

async fn wire(response: Response) -> String {
    assert_eq!(response.status(), StatusCode::OK);
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn ids(wire: &str) -> Vec<u64> {
    wire.lines()
        .filter_map(|line| line.strip_prefix("id: "))
        .map(|value| value.parse().unwrap())
        .collect()
}

#[tokio::test]
async fn replay_crosses_pages_and_every_retained_cursor_keeps_exact_numbering() {
    use layer2_core::sandbox::store::{AgentLogStore, AgentStore};
    let (state, store) = conversation_fixture().await;
    let state = state.with_bounded_replay(true);
    let head = 1004;
    let agent = seeded_agent("agt_bounded", AgentStatus::Ready, head, None);
    insert_agent(&store, agent.clone(), history(head)).await;
    let complete = wire(
        open(
            state.clone(),
            caller(),
            agent.id.clone(),
            0,
            false,
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(ids(&complete), (2..=head).collect::<Vec<_>>());
    // Interleave an append with each of 1,000 retained replay positions across
    // page and terminal boundaries. Finite replay must keep its captured head.
    // A paused consumer reconnects at the last complete durable frame, never a
    // partially projected text frame or an ephemeral fragment.
    let mut current = agent.clone();
    for after in 4..1004 {
        let through = current.conversation.last_seq;
        let response = open(
            state.clone(),
            caller(),
            agent.id.clone(),
            after,
            false,
            None,
            None,
        )
        .await;
        store
            .append_agent_log(
                &agent.id,
                agent.updated_at.clone(),
                AgentLogKind::ModeChanged {
                    mode: "code".into(),
                },
            )
            .await
            .unwrap();
        let revision = current.revision;
        current.revision += 1;
        current.conversation.last_seq += 1;
        store.save_agent(revision, current.clone()).await.unwrap();
        let resumed = wire(response).await;
        assert_eq!(
            ids(&resumed),
            (after + 1..=through).collect::<Vec<_>>(),
            "after={after}"
        );
        assert!(!resumed.contains("history_unavailable"));
        assert!(resumed.contains("\"runId\":\"3\""));
    }
}

#[tokio::test]
async fn failed_page_keeps_verified_cursor_and_stream_reports_failure_without_success() {
    let (state, store) = conversation_fixture().await;
    let state = state.with_bounded_replay(true);
    let agent = seeded_agent("agt_corrupt_page", AgentStatus::Working, 401, Some(3));
    let mut kinds = history(401);
    kinds[198] = AgentLogKind::TurnStarted {
        turn_sequence: 201,
        origin_key: None,
        model: None,
    };
    insert_agent(&store, agent.clone(), kinds).await;
    let mut cursor = AgentHistoryCursor::default();
    let page = state
        .reads
        .owner_event_stream_page(&caller(), &agent.id, &mut cursor, None)
        .await
        .unwrap();
    assert_eq!(page.history.len(), 200);
    assert_eq!(cursor.sequence(), 200);
    assert!(state
        .reads
        .owner_event_stream_page(&caller(), &agent.id, &mut cursor, None)
        .await
        .is_err());
    assert_eq!(
        cursor.sequence(),
        200,
        "failure must not consume the durable cursor"
    );
    let mut body = open(state, caller(), agent.id, 0, false, None, None)
        .await
        .into_body()
        .into_data_stream();
    let mut actual = String::new();
    while let Ok(frame) = body
        .next()
        .await
        .expect("corruption must abort the body, not end successfully")
    {
        actual.push_str(&String::from_utf8_lossy(&frame));
    }
    assert_eq!(ids(&actual), (2..=200).collect::<Vec<_>>());
    assert!(!actual.contains("RUN_ERROR"));
    assert!(!actual.contains("\"type\":\"complete\""));
}

#[tokio::test]
async fn corrupt_initial_page_and_foreign_cursor_refuse_before_any_history() {
    let (state, store) = conversation_fixture().await;
    let state = state.with_bounded_replay(true);
    let agent = seeded_agent("agt_bad_initial", AgentStatus::Ready, 6, None);
    let mut kinds = history(6);
    kinds[0] = AgentLogKind::TurnCompleted { turn_sequence: 3 };
    insert_agent(&store, agent.clone(), kinds).await;
    assert_eq!(
        open(
            state.clone(),
            caller(),
            agent.id.clone(),
            0,
            false,
            None,
            None
        )
        .await
        .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let valid = seeded_agent("agt_other_cursor", AgentStatus::Ready, 6, None);
    insert_agent(&store, valid.clone(), history(6)).await;
    let mut cursor = AgentHistoryCursor::default();
    state
        .reads
        .owner_event_stream_page(&caller(), &valid.id, &mut cursor, None)
        .await
        .unwrap();
    assert!(state
        .reads
        .owner_event_stream_page(&caller(), &agent.id, &mut cursor, None)
        .await
        .is_err());
    assert_eq!(cursor.sequence(), 6);
}

#[tokio::test]
async fn empty_source_is_unavailable_instead_of_a_successful_blank_history() {
    use layer2_core::sandbox::store::AgentStore;
    let (state, store) = conversation_fixture().await;
    let state = state.with_bounded_replay(true);
    let mut agent = seeded_agent("agt_missing_creation", AgentStatus::Ready, 2, None);
    insert_agent(&store, agent.clone(), vec![]).await;
    let revision = agent.revision;
    agent.revision += 1;
    agent.conversation.last_seq = 0;
    store.save_agent(revision, agent.clone()).await.unwrap();
    let response = open(state, caller(), agent.id, 0, false, None, None).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test(start_paused = true)]
async fn idle_stream_uses_fixed_fallback_and_finds_writes_without_hints() {
    use layer2_core::sandbox::store::{AgentLogStore, AgentStore};
    let (state, store) = conversation_fixture().await;
    let state = state.with_bounded_replay(true);
    let mut agent = seeded_agent("agt_dropped_hint", AgentStatus::Working, 4, Some(3));
    insert_agent(&store, agent.clone(), history(6)[..2].to_vec()).await;
    let mut body = open(state, caller(), agent.id.clone(), 4, true, None, None)
        .await
        .into_body()
        .into_data_stream();
    assert!(
        tokio::time::timeout(Duration::from_millis(999), body.next())
            .await
            .is_err()
    );
    store
        .append_agent_log(
            &agent.id,
            agent.updated_at.clone(),
            AgentLogKind::AgentMessage {
                content: "after dropped hint".into(),
            },
        )
        .await
        .unwrap();
    let revision = agent.revision;
    agent.revision += 1;
    agent.conversation.last_seq = 5;
    store.save_agent(revision, agent).await.unwrap();
    let mut observed = String::new();
    for _ in 0..3 {
        let frame = tokio::time::timeout(Duration::from_millis(2), body.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        observed.push_str(&String::from_utf8_lossy(&frame));
    }
    assert!(observed.contains("after dropped hint"));
    assert_eq!(ids(&observed), [5]);
}

#[tokio::test]
async fn byte_trimmed_pages_and_a_large_original_message_preserve_all_text() {
    let (state, store) = conversation_fixture().await;
    let state = state.with_bounded_replay(true);
    let agent = seeded_agent("agt_large_original", AgentStatus::Ready, 8, None);
    let text = "original λ output\n".repeat(800_000);
    let mut kinds = history(8);
    kinds[2] = AgentLogKind::AgentMessage {
        content: text.clone(),
    };
    insert_agent(&store, agent.clone(), kinds).await;
    let actual = wire(open(state, caller(), agent.id, 0, false, None, None).await).await;
    assert_eq!(ids(&actual), (2..=8).collect::<Vec<_>>());
    let frames: Vec<serde_json::Value> = actual
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let reconstructed: String = frames
        .iter()
        .filter(|frame| frame["messageId"] == "msg_5")
        .filter_map(|frame| frame["delta"].as_str())
        .collect();
    assert_eq!(reconstructed, text);
    assert!(frames
        .iter()
        .all(|frame| serde_json::to_vec(frame).unwrap().len() < 64 * 1024));
}

#[tokio::test]
async fn retained_deleted_history_finishes_message_chunks_without_losing_the_cursor() {
    use layer2_core::sandbox::store::AgentStore;
    let (state, store) = conversation_fixture().await;
    let state = state.with_bounded_replay(true);
    let mut agent = seeded_agent("agt_chunk_fence", AgentStatus::Ready, 6, None);
    let mut kinds = history(6);
    kinds[2] = AgentLogKind::AgentMessage {
        content: "λ\u{0001}".repeat(20_000),
    };
    insert_agent(&store, agent.clone(), kinds).await;
    let mut body = open(state, caller(), agent.id.clone(), 4, false, None, None)
        .await
        .into_body()
        .into_data_stream();
    for _ in 0..2 {
        let frame = body.next().await.unwrap().unwrap();
        let text = String::from_utf8_lossy(&frame);
        assert!(!text.contains("id:"));
        assert!(!text.contains("TEXT_MESSAGE_END"));
    }
    let revision = agent.revision;
    agent.revision += 1;
    agent.status = AgentStatus::Deleted;
    agent.deleted_at = Some(agent.updated_at.clone());
    store.save_agent(revision, agent.clone()).await.unwrap();
    // Soft deletion retains authorized history until purge, as on the other
    // public history interface. It must not blank previously visible output.
    let frame = body.next().await.unwrap().unwrap();
    assert!(!String::from_utf8_lossy(&frame).contains("id:"));
    let mut remaining = String::new();
    while let Some(frame) = body.next().await {
        remaining.push_str(&String::from_utf8_lossy(&frame.unwrap()));
    }
    assert_eq!(ids(&remaining), [5, 6]);
    assert!(remaining.contains("TEXT_MESSAGE_END"));
}
