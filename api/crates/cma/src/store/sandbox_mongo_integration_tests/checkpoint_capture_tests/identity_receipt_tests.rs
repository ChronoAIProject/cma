use super::*;
use layer2_core::sandbox::store::{AgentConversationStore, AgentStore};
use layer2_core::sandbox::{
    AgentMessageAssembly, AgentStatus, AgentUpdateOutcome, AgentUpdateRecorder,
    DurableAgentUpdateRecorder,
};
use layer3_adapter::{
    ConversationUpdate, PromptResponse, RuntimeOutputReceipt, RuntimeOutputSource, TurnStopReason,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyRuntimeOutputSource {
    stream_id: String,
    cursor: u64,
    native_conversation_id: String,
    payload_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyCheckpoint {
    version: u32,
    update_id: String,
    update_fingerprint: String,
    native_conversation_id: String,
    runtime_generation: u64,
    active_turn: Option<u64>,
    after_seq: u64,
    assembly: AgentMessageAssembly,
    outcome: AgentUpdateOutcome,
    output: Option<LegacyRuntimeOutputSource>,
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_identity_terminal_preserves_receipt_and_predecessor_shape() {
    let (client, name) = database("identity_terminal_receipt").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let ready = ready_conversation_fixture(&store, "identity_terminal_receipt").await;
    let turn = begin_stored_conversation_turn(&store, ready).await;
    let working = turn.agent.clone();
    store.commit_agent_conversation(turn).await.unwrap();
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let source = |cursor: u64, hash: &str| {
        RuntimeOutputReceipt::new(
            RuntimeOutputSource {
                stream_id: "d99003d1-0c3c-46cd-aa47-9bbdbed50b12".into(),
                cursor,
                native_conversation_id: working.conversation.conversation_id.clone().unwrap(),
                payload_sha256: hash.into(),
            },
            tokio::sync::mpsc::channel(1).0,
        )
    };
    recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::RuntimeOutput {
                receipt: source(1, &"a".repeat(64)),
                update: None,
            },
            NOW.into(),
        )
        .await
        .unwrap();
    let prompt_id = layer3_adapter::PromptIdentity::new(
        working.conversation.conversation_id.as_deref().unwrap(),
        working.conversation.runtime_generation.unwrap(),
        working.conversation.active_turn.unwrap(),
    );
    let outcome = recorder
        .record_update(
            &working.id,
            1,
            &mut AgentMessageAssembly::default(),
            ConversationUpdate::RuntimeOutput {
                receipt: source(2, &"b".repeat(64)),
                update: Some(
                    Box::new(ConversationUpdate::TurnEndedForPrompt {
                        prompt_id,
                        response: PromptResponse::new(TurnStopReason::EndTurn),
                    })
                    .into(),
                ),
            },
            MINUTE.into(),
        )
        .await
        .unwrap();
    assert!(outcome.turn_closed);
    let checkpoint = store
        .agent_message_checkpoint(&working.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.output.as_ref().unwrap().cursor, 2);
    assert_eq!(
        store.get_agent(&working.id).await.unwrap().unwrap().status,
        AgentStatus::Ready
    );

    let row = store
        .agents
        .find_one(doc! { "_id": working.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    let stored = row
        .get_document("agent_message_checkpoint")
        .unwrap()
        .clone();
    let legacy: LegacyCheckpoint = mongodb::bson::deserialize_from_document(stored).unwrap();
    assert_eq!(legacy.version, 1);
    assert!(!legacy.update_id.is_empty());
    assert_eq!(legacy.update_fingerprint.len(), 64);
    assert_eq!(
        legacy.native_conversation_id,
        "conversation-identity_terminal_receipt"
    );
    assert_eq!(legacy.runtime_generation, 1);
    assert!(legacy.active_turn.is_none());
    assert!(legacy.assembly.is_empty());
    assert!(legacy.outcome.turn_closed);
    let legacy_native_conversation_id = legacy.native_conversation_id.clone();
    let legacy_output = legacy.output.unwrap();
    assert_eq!(
        legacy_output.stream_id,
        "d99003d1-0c3c-46cd-aa47-9bbdbed50b12"
    );
    assert_eq!(legacy_output.cursor, 2);
    assert_eq!(
        legacy_output.native_conversation_id,
        legacy_native_conversation_id
    );
    assert_eq!(legacy_output.payload_sha256, "b".repeat(64));
    assert_eq!(legacy.after_seq, checkpoint.after_seq);
    client.database(&name).drop().await.unwrap();
}
