use std::sync::Arc;

use layer2_core::sandbox::{
    AgentOperationDelivery, AgentOperationIntent, AgentOperationRecord, AgentTurnUsage,
    AgentUpdateRecorder, AgentUsageCheckpoint, DurableAgentUpdateRecorder,
    NativeCompletionEvidence,
};
use layer3_adapter::{
    ContentBlock, ContentChunk, ConversationUpdate, MessageId, PromptResponse, SessionUpdate,
    TurnStopReason,
};

use super::*;

fn chunk(content: &str) -> ConversationUpdate {
    ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
        ContentChunk::new(ContentBlock::from(content)).message_id(MessageId::new("reply")),
    ))
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn replacement_mongo_recorder_recovers_prefix_then_commits_one_complete_message() {
    let (client, name, store) = database().await;
    let current = working(&store, "sbx_checkpoint_recorder").await;
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    recorder
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            chunk("prefix "),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        store.get_agent(&current.id).await.unwrap(),
        Some(current.clone())
    );
    drop(recorder);

    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(reopened.clone()));
    replacement
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            chunk("suffix"),
            NOW.into(),
        )
        .await
        .unwrap();
    drop(replacement);
    let terminal = DurableAgentUpdateRecorder::new(Arc::new(reopened.clone()));
    let outcome = terminal
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::EndTurn)),
            NOW.into(),
        )
        .await
        .unwrap();
    assert!(outcome.turn_closed);
    let logs = reopened.agent_log(&current.id, 0, 100).await.unwrap();
    let messages: Vec<_> = logs
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentLogKind::AgentMessage { content } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(messages, ["prefix suffix"]);
    let checkpoint = reopened
        .agent_message_checkpoint(&current.id)
        .await
        .unwrap()
        .unwrap();
    assert!(checkpoint.assembly.is_empty());
    assert_eq!(checkpoint.active_turn, None);
    assert_eq!(checkpoint.outcome, outcome);
    let finished = reopened.get_agent(&current.id).await.unwrap().unwrap();
    assert_eq!(finished.status, AgentStatus::Ready);
    assert_eq!(finished.conversation.active_turn, None);
    assert!(checkpoint.matches(&finished));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_reconciliation_commits_one_terminal_and_replays_idempotently() {
    let (client, name, store) = database().await;
    let mut current = super::working(&store, "sbx_reconciliation").await;
    let turn = current.conversation.active_turn.unwrap();
    let mut operation = AgentOperationRecord::new(
        1,
        current.conversation.runtime_generation.unwrap(),
        turn,
        AgentOperationIntent::send_message("proof"),
        NOW.into(),
    )
    .unwrap();
    operation.state = AgentOperationDelivery::ExecutionObserved;
    operation.execution_observed_at = Some(NOW.into());
    current.conversation.operation = Some(operation);
    current.conversation.observations.usage = Some(AgentUsageCheckpoint {
        conversation_id: current.conversation.conversation_id.clone().unwrap(),
        source_turn: "native-turn".into(),
        current_turn: Some(AgentTurnUsage {
            turn_sequence: turn,
            input_tokens: 1,
            output_tokens: 1,
        }),
        ..Default::default()
    });
    let revision = current.revision;
    store.save_agent(revision, current.clone()).await.unwrap();
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .record_update(
            &current.id,
            current.conversation.runtime_generation.unwrap(),
            &mut assembly,
            chunk("mongo answer"),
            NOW.into(),
        )
        .await
        .unwrap();
    let evidence = NativeCompletionEvidence {
        native_conversation_id: current.conversation.conversation_id.clone().unwrap(),
        runtime_generation: current.conversation.runtime_generation.unwrap(),
        turn_sequence: turn,
        native_turn_id: "native-turn".into(),
        admitted_at: NOW.into(),
        completed_at: "2026-09-17T00:00:01Z".into(),
        task_started: true,
        task_complete: true,
        no_later_open_turn: true,
        history_complete: true,
        history_prefix_unchanged: true,
        history_sha256: "a".repeat(64),
    };
    let first = recorder
        .reconcile_completion(
            &current.id,
            current.conversation.runtime_generation.unwrap(),
            &mut assembly,
            evidence.clone(),
            PromptResponse::new(TurnStopReason::EndTurn),
            "2026-09-17T00:00:01Z".into(),
        )
        .await
        .unwrap();
    assert!(first.turn_closed);
    let second = recorder
        .reconcile_completion(
            &current.id,
            current.conversation.runtime_generation.unwrap(),
            &mut AgentMessageAssembly::default(),
            evidence,
            PromptResponse::new(TurnStopReason::EndTurn),
            "2026-09-17T00:00:01Z".into(),
        )
        .await
        .unwrap();
    assert_eq!(second, first);
    assert_eq!(
        store
            .agent_log(&current.id, 0, 100)
            .await
            .unwrap()
            .iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TurnCompleted { .. }))
            .count(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_mongo_output_replays_once_after_commit_before_ack_and_reopen() {
    let (client, name, store) = database().await;
    let current = working(&store, "sbx_output_replay").await;
    let stream = uuid::Uuid::new_v4().to_string();
    let retained = |cursor: u64, update: Option<ConversationUpdate>| {
        let (ack, _) = tokio::sync::mpsc::channel(1);
        ConversationUpdate::RuntimeOutput {
            receipt: layer3_adapter::RuntimeOutputReceipt::new(
                layer3_adapter::RuntimeOutputSource {
                    stream_id: stream.clone(),
                    cursor,
                    native_conversation_id: current.conversation.conversation_id.clone().unwrap(),
                    payload_sha256: format!("{cursor:064x}"),
                },
                ack,
            ),
            update: update.map(Box::new),
        }
    };
    let recorder = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
    recorder
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            retained(1, Some(chunk("one answer"))),
            NOW.into(),
        )
        .await
        .unwrap();
    drop(recorder);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(reopened.clone()));
    let mut assembly = AgentMessageAssembly::default();
    replacement
        .record_update(
            &current.id,
            1,
            &mut assembly,
            retained(1, Some(chunk("one answer"))),
            NOW.into(),
        )
        .await
        .unwrap();
    replacement
        .record_update(
            &current.id,
            1,
            &mut assembly,
            retained(
                2,
                Some(ConversationUpdate::TurnEnded(PromptResponse::new(
                    TurnStopReason::EndTurn,
                ))),
            ),
            NOW.into(),
        )
        .await
        .unwrap();
    drop(replacement);
    let reopened_again = MongoSandboxStore::open(&client, &name).await.unwrap();
    let replacement = DurableAgentUpdateRecorder::new(Arc::new(reopened_again.clone()));
    let before = reopened_again.agent_log(&current.id, 0, 100).await.unwrap();
    // The completed prompt no longer has a waiter on the replacement adapter.
    replacement
        .record_update(
            &current.id,
            1,
            &mut AgentMessageAssembly::default(),
            retained(2, None),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(
        reopened_again.agent_log(&current.id, 0, 100).await.unwrap(),
        before
    );
    let messages: Vec<_> = before
        .iter()
        .filter_map(|entry| match &entry.kind {
            AgentLogKind::AgentMessage { content } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(messages, ["one answer"]);
    let checkpoint = reopened_again
        .agent_message_checkpoint(&current.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.output.unwrap().cursor, 2);
    assert!(checkpoint.assembly.is_empty());
    assert_eq!(
        reopened_again
            .get_agent(&current.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Ready
    );
    client.database(&name).drop().await.unwrap();
}
