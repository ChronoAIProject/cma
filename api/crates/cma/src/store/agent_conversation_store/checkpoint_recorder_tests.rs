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
