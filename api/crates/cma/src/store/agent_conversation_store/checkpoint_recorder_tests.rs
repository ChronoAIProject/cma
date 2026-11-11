use std::sync::Arc;

use layer2_core::sandbox::{AgentUpdateRecorder, DurableAgentUpdateRecorder};
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
            update: update.map(|update| Box::new(update).into()),
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
