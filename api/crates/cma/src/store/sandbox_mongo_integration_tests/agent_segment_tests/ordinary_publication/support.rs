use super::*;
use layer2_core::sandbox::{store::AgentMessageCheckpoint, AgentUpdateOutcome};
use layer3_adapter::{ContentBlock, ContentChunk, ConversationUpdate, MessageId, SessionUpdate};
use mongodb::bson::Document;
use std::collections::BTreeMap;

pub(super) async fn working(
    suffix: &str,
) -> (
    Client,
    String,
    MongoSandboxStore,
    Agent,
    AdmittedAgentCommand,
) {
    let (client, name, store, agent) = super::super::retained_assessment::fixture(suffix).await;
    assert!(store
        .due_agent_work(NOW, 20)
        .await
        .unwrap()
        .contains(&agent.id));
    let (working, command) = begin_working(&store, agent).await;
    (client, name, store, working, command)
}

pub(super) async fn begin_working(
    store: &MongoSandboxStore,
    agent: Agent,
) -> (Agent, AdmittedAgentCommand) {
    let ready = super::super::worker_discovery::finish_legacy_connection(store, agent).await;
    let parent = store.get_sandbox(&ready.sandbox_id).await.unwrap().unwrap();
    let command = record(
        store
            .reserve_command(live_request(&ready, &parent, "ordinary-output"))
            .await
            .unwrap(),
    );
    let claimed = store
        .claim_next_command(claim_request(
            &ready,
            "output-worker",
            &layer2_core::now_rfc3339(),
        ))
        .await
        .unwrap()
        .unwrap();
    let claim = claimed.work.lease.as_ref().unwrap().claim.clone();
    store
        .commit_agent_conversation(message_mutation(&ready, &parent, &claim))
        .await
        .unwrap();
    // Keep custody through runtime output. The actual settlement transaction,
    // including abort/unknown retries, must clear this exact original claim.
    let working = store.get_agent(&ready.id).await.unwrap().unwrap();
    (working, command)
}

pub(super) fn chunk(content: &str) -> ConversationUpdate {
    ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
        ContentChunk::new(ContentBlock::from(content)).message_id(MessageId::new("reply")),
    ))
}

pub(super) fn output(current: &Agent, content: &str) -> AgentConversationMutation {
    let mut next = current.clone();
    next.revision += 1;
    next.conversation.last_seq += 1;
    next.updated_at = layer2_core::now_rfc3339();
    AgentConversationMutation {
        settings_fence: None,
        command_claim: None,
        execution_fence: None,
        trigger_execution_fence: None,
        admission_guard: None,
        expected_agent_revision: current.revision,
        expected_last_seq: current.conversation.last_seq,
        runtime_generation: current.conversation.runtime_generation.unwrap(),
        appended_at: next.updated_at.clone(),
        agent: next,
        agent_logs: vec![AgentLogKind::AgentMessage {
            content: content.into(),
        }],
    }
}

pub(super) fn partial(current: &Agent, content: &str) -> AgentMessageCheckpoint {
    let mut assembly = serde_json::to_value(AgentMessageAssembly::default()).unwrap();
    assembly["message_id"] = serde_json::json!("reply");
    assembly["content"] = serde_json::json!(content);
    AgentMessageCheckpoint {
        version: 1,
        update_id: uuid::Uuid::new_v4().to_string(),
        update_fingerprint: "a".repeat(64),
        native_conversation_id: current.conversation.conversation_id.clone().unwrap(),
        runtime_generation: current.conversation.runtime_generation.unwrap(),
        active_turn: current.conversation.active_turn,
        after_seq: current.conversation.last_seq,
        assembly: serde_json::from_value(assembly).unwrap(),
        outcome: AgentUpdateOutcome {
            appended: vec![],
            turn_closed: false,
        },
        output: None,
    }
}

pub(super) fn boundary(mutation: &AgentConversationMutation) -> AgentMessageCheckpoint {
    let mut next = partial(&mutation.agent, "");
    next.assembly = AgentMessageAssembly::default();
    next.outcome.appended = conversation::conversation_logs(mutation).unwrap();
    next
}

pub(super) async fn snapshot(client: &Client, name: &str) -> BTreeMap<String, Vec<Document>> {
    use futures_util::TryStreamExt;
    let mut result = BTreeMap::new();
    let database = client.database(name);
    for collection in segments::COLLECTIONS.iter().copied().chain([
        "agents",
        "agent_log",
        "sandboxes",
        "agent_commands",
        "agent_command_orders",
        "agent_suggestions",
    ]) {
        let rows = database
            .collection::<Document>(collection)
            .find(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        result.insert(collection.into(), rows);
    }
    result
}
