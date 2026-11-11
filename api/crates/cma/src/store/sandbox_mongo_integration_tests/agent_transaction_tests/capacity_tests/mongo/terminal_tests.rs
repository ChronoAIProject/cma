use super::*;
use layer2_core::sandbox::{AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder};
use layer3_adapter::{ConversationUpdate, PromptIdentity, PromptResponse, TurnStopReason};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn count_exhausted_native_terminal_settles_once_without_losing_history() {
    let (client, name) = database("capacity_native_terminal").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let ready = ready_conversation_fixture(&store, "capacity_native_terminal").await;
    let turn = begin_stored_conversation_turn(&store, ready).await;
    store.commit_agent_conversation(turn.clone()).await.unwrap();
    let full = fill_normal_capacity(&store, turn.agent, false).await;
    let history = fingerprint(&store, &full.id, full.conversation.last_seq).await;
    let terminal = ConversationUpdate::TurnEndedForPrompt {
        prompt_id: PromptIdentity::new(
            full.conversation.conversation_id.as_deref().unwrap(),
            1,
            full.conversation.active_turn.unwrap(),
        ),
        response: PromptResponse::new(TurnStopReason::EndTurn),
    };
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let outcome = recorder
        .record_update(
            &full.id,
            1,
            &mut AgentMessageAssembly::default(),
            terminal,
            MINUTE.into(),
        )
        .await
        .expect("an exhausted normal budget must not strand the native terminal");
    assert!(outcome.turn_closed);
    let saved = store.get_agent(&full.id).await.unwrap().unwrap();
    assert_eq!(saved.status, AgentStatus::Failed);
    assert_eq!(
        saved.failure.as_ref().unwrap().class,
        "agent_log_capacity_exceeded"
    );
    assert!(saved.conversation.active_turn.is_none());
    assert!(saved.work.lease.is_none());
    assert_eq!(saved.work.step, AgentWorkStep::None);
    assert_eq!(
        fingerprint(&store, &full.id, full.conversation.last_seq).await,
        history
    );
    assert_eq!(
        outcome
            .appended
            .iter()
            .filter(|entry| matches!(
                entry.kind,
                AgentLogKind::TurnCompleted { .. } | AgentLogKind::TurnFailed { .. }
            ))
            .count(),
        1
    );
    client.database(&name).drop().await.unwrap();
}
