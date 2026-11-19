use super::*;
use crate::rest::agents_tests::{conversation_fixture, insert_agent, seeded_agent};
use futures_util::StreamExt;
use layer2_core::sandbox::{
    store::{AgentLogStore, AgentStore},
    AgentMessageAssembly, AgentRuntimeUpdate, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use std::sync::Arc;

async fn fixture() -> (
    AgentConversationState,
    Arc<layer2_core::sandbox::store::InMemorySandboxStore>,
    Agent,
    Arc<DurableAgentUpdateRecorder>,
) {
    let (state, store) = conversation_fixture().await;
    let state = state.with_bounded_replay(true);
    let agent = seeded_agent("agt_live_pages", AgentStatus::Working, 300, Some(3));
    let mut kinds = vec![
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 3,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "original input".into(),
        },
    ];
    kinds.resize(
        298,
        AgentLogKind::AgentMessage {
            content: "durable history".into(),
        },
    );
    insert_agent(&store, agent.clone(), kinds).await;
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    (state.with_updates(recorder.clone()), store, agent, recorder)
}

async fn publish(recorder: &DurableAgentUpdateRecorder, agent: &Agent, text: &str) {
    recorder
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            AgentRuntimeUpdate::Said(
                serde_json::from_value(serde_json::json!({
                    "sessionUpdate":"agent_message_chunk", "messageId":"live-pages",
                    "content":{"type":"text","text":text}
                }))
                .unwrap(),
            ),
            "2026-09-16T00:00:00Z".into(),
        )
        .await
        .unwrap();
}

fn caller(agent: &Agent) -> Caller {
    Caller::new(
        agent.owner.clone(),
        layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
    )
}

#[tokio::test(start_paused = true)]
async fn live_wake_waits_for_all_durable_pages_and_does_not_wait_for_poll_timer() {
    let (state, _, agent, recorder) = fixture().await;
    let watching = recorder.subscribe_live(&agent.id);
    let response = open(
        state,
        caller(&agent),
        agent.id.clone(),
        0,
        true,
        None,
        watching,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    publish(&recorder, &agent, "immediate live").await;
    let before = tokio::time::Instant::now();
    let mut body = response.into_body().into_data_stream();
    let mut last = 0;
    loop {
        let frame = body.next().await.unwrap().unwrap();
        let wire = String::from_utf8(frame.to_vec()).unwrap();
        if let Some(id) = wire.lines().find_map(|line| line.strip_prefix("id: ")) {
            last = id.parse().unwrap();
        }
        if wire.contains("immediate live") {
            assert_eq!(last, 300);
            break;
        }
    }
    assert_eq!(
        tokio::time::Instant::now(),
        before,
        "buffered live output must not wait for the fallback"
    );
}

#[tokio::test(start_paused = true)]
async fn backpressured_live_batch_catches_up_before_gap_without_advancing_live_cursor() {
    let (state, store, mut agent, recorder) = fixture().await;
    let watching = recorder.subscribe_live(&agent.id);
    let response = open(
        state,
        caller(&agent),
        agent.id.clone(),
        300,
        true,
        None,
        watching,
    )
    .await;
    let mut body = response.into_body().into_data_stream();
    publish(&recorder, &agent, "first live").await;
    publish(&recorder, &agent, "discarded live").await;
    let frame = body.next().await.unwrap().unwrap();
    let wire = String::from_utf8_lossy(&frame);
    assert!(wire.contains("first live") && !wire.contains("id:"));
    store
        .append_agent_log(
            &agent.id,
            agent.updated_at.clone(),
            AgentLogKind::AgentMessage {
                content: "committed after backpressure".into(),
            },
        )
        .await
        .unwrap();
    let revision = agent.revision;
    agent.revision += 1;
    agent.conversation.last_seq += 1;
    store.save_agent(revision, agent).await.unwrap();
    tokio::time::advance(Duration::from_secs(1)).await;
    let mut delivered = String::new();
    for _ in 0..4 {
        let frame = body.next().await.unwrap().unwrap();
        delivered.push_str(&String::from_utf8_lossy(&frame));
    }
    assert!(delivered.contains("committed after backpressure"));
    assert!(!delivered.contains("discarded live"));
    assert!(delivered.find("id: 301").unwrap() < delivered.find("conversationGap").unwrap());
    assert_eq!(
        delivered
            .lines()
            .filter(|line| line.starts_with("id:"))
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn broadcast_lag_reports_gap_and_retains_durable_cursor() {
    let (state, _, agent, recorder) = fixture().await;
    let watching = recorder.subscribe_live(&agent.id);
    let response = open(
        state,
        caller(&agent),
        agent.id.clone(),
        300,
        true,
        None,
        watching,
    )
    .await;
    for _ in 0..300 {
        publish(&recorder, &agent, "ephemeral").await;
    }
    let mut body = response.into_body().into_data_stream();
    let frame = body.next().await.unwrap().unwrap();
    let wire = String::from_utf8_lossy(&frame);
    assert!(wire.contains("conversationGap"));
    assert!(
        !wire.contains("id:"),
        "lag never acknowledges a durable event"
    );
}
