use super::*;
use layer2_core::sandbox::{AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder};
use layer3_adapter::{ConversationUpdate, PromptIdentity, PromptResponse, TurnStopReason};
use std::time::{Duration, Instant};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn count_exhausted_native_terminal_settles_once_without_losing_history() {
    terminal_case(false, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn byte_exhausted_native_terminal_settles_once_without_losing_history() {
    terminal_case(true, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn count_refused_output_cannot_be_followed_by_successful_terminal() {
    terminal_case(false, true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn byte_refused_output_cannot_be_followed_by_successful_terminal() {
    terminal_case(true, true).await;
}

async fn terminal_case(bytes: bool, refuse_output: bool) {
    let (client, name) = database("capacity_native_terminal").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let ready = ready_conversation_fixture(&store, "capacity_native_terminal").await;
    let turn = begin_stored_conversation_turn(&store, ready).await;
    store.commit_agent_conversation(turn.clone()).await.unwrap();
    let full = fill_normal_capacity(&store, turn.agent, bytes).await;
    let history = fingerprint(&store, &full.id, full.conversation.last_seq).await;
    let terminal = ConversationUpdate::TurnEndedForPrompt {
        prompt_id: PromptIdentity::new(
            full.conversation.conversation_id.as_deref().unwrap(),
            1,
            full.conversation.active_turn.unwrap(),
        ),
        response: PromptResponse::new(TurnStopReason::EndTurn),
    };
    let update = if refuse_output {
        ConversationUpdate::Said(layer3_adapter::SessionUpdate::ToolCallUpdate(
            layer3_adapter::ToolCallUpdate::new(
                "refused-tool",
                layer3_adapter::ToolCallUpdateFields::new().title("Completed calculation"),
            ),
        ))
    } else {
        terminal.clone()
    };
    let (acks, _acknowledged) = tokio::sync::mpsc::channel(8);
    let retained = |cursor, update| ConversationUpdate::RuntimeOutput {
        receipt: layer3_adapter::RuntimeOutputReceipt::new(
            layer3_adapter::RuntimeOutputSource {
                stream_id: "72111111-1111-4111-8111-111111111111".into(),
                cursor,
                native_conversation_id: full.conversation.conversation_id.clone().unwrap(),
                payload_sha256: "a".repeat(64),
            },
            acks.clone(),
        ),
        update: Some(Box::new(update)),
    };
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let started = Instant::now();
    let outcome = recorder
        .record_update(
            &full.id,
            1,
            &mut AgentMessageAssembly::default(),
            retained(1, update.clone()),
            MINUTE.into(),
        )
        .await
        .expect("an exhausted normal budget must not strand the native terminal");
    assert!(started.elapsed() < Duration::from_secs(120));
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
    assert_eq!(saved.work.receipts, full.work.receipts);
    assert_eq!(saved.work.cleanup_debt, full.work.cleanup_debt);
    assert_eq!(
        saved.conversation.observations.usage,
        full.conversation.observations.usage
    );
    assert!(outcome.appended.iter().any(|entry| if refuse_output {
        matches!(&entry.kind, AgentLogKind::TurnFailed { failure, .. } if failure.class == "agent_log_capacity_exceeded")
    } else { matches!(entry.kind, AgentLogKind::TurnCompleted { .. }) }));
    let peer = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let replacement = DurableAgentUpdateRecorder::new(peer);
    replacement
        .record_update(
            &full.id,
            1,
            &mut AgentMessageAssembly::default(),
            retained(1, update),
            MINUTE.into(),
        )
        .await
        .unwrap();
    if refuse_output {
        assert!(replacement
            .record_update(
                &full.id,
                1,
                &mut AgentMessageAssembly::default(),
                retained(2, terminal),
                MINUTE.into()
            )
            .await
            .is_err());
    }
    assert_eq!(store.get_agent(&full.id).await.unwrap(), Some(saved));
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
