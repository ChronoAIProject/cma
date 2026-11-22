use super::*;
use layer2_core::sandbox::{
    AgentMessageAssembly, AgentRuntimeUpdate, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};

#[tokio::test(start_paused = true)]
async fn shared_release_fence_catches_up_before_releasing_a_stale_live_batch() {
    let mut f = fixture().await;
    let id = AgentId("agt_shared_stream".into());
    let mut agent = f.store.get_agent(&id).await.unwrap().unwrap();
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 10,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Another request".into(),
        },
    ] {
        f.store
            .append_agent_log(&id, agent.updated_at.clone(), kind)
            .await
            .unwrap();
    }
    let revision = agent.revision;
    agent.revision += 1;
    agent.status = AgentStatus::Working;
    agent.conversation.active_turn = Some(10);
    agent.conversation.last_seq = 11;
    f.store.save_agent(revision, agent.clone()).await.unwrap();
    let recorder = DurableAgentUpdateRecorder::new(f.store.clone());
    f.state = f
        .state
        .with_bounded_replay(true)
        .with_updates(recorder.clone());
    let response = crate::agui::router(f.state)
        .oneshot(request("recipient", "starting_after=11&follow=true", true))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    for text in ["first live", "discarded live"] {
        recorder
            .record_update(
                &id,
                1,
                &mut AgentMessageAssembly::default(),
                AgentRuntimeUpdate::Said(
                    serde_json::from_value(serde_json::json!({
                        "sessionUpdate": "agent_message_chunk", "messageId": "shared-live",
                        "content": {"type": "text", "text": text}
                    }))
                    .unwrap(),
                ),
                agent.updated_at.clone(),
            )
            .await
            .unwrap();
    }
    let mut body = response.into_body().into_data_stream();
    let frame = body.next().await.unwrap().unwrap();
    let wire = String::from_utf8_lossy(&frame);
    assert!(wire.contains("first live") && !wire.contains("id:"));
    f.store
        .append_agent_log(
            &id,
            agent.updated_at.clone(),
            AgentLogKind::AgentMessage {
                content: "committed after backpressure".into(),
            },
        )
        .await
        .unwrap();
    let revision = agent.revision;
    agent.revision += 1;
    agent.conversation.last_seq = 12;
    f.store.save_agent(revision, agent).await.unwrap();
    let before = tokio::time::Instant::now();
    let mut delivered = String::new();
    for _ in 0..4 {
        let frame = body.next().await.unwrap().unwrap();
        delivered.push_str(&String::from_utf8_lossy(&frame));
    }
    assert!(!delivered.contains("discarded live"));
    assert!(delivered.contains("committed after backpressure"));
    assert!(delivered.find("id: 12").unwrap() < delivered.find("conversationGap").unwrap());
    assert_eq!(
        delivered
            .lines()
            .filter(|line| line.starts_with("id:"))
            .count(),
        1
    );
    assert_eq!(tokio::time::Instant::now(), before);
}
