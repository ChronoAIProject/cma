use layer2_core::sandbox::store::{
    AgentLogStore, AgentMessageCheckpoint, AgentStore, SandboxMutationGuard,
};
use layer2_core::sandbox::{AgentLogKind, AgentMessageAssembly, AgentStatus, AgentUpdateOutcome};

use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture};

#[path = "checkpoint_abort_tests.rs"]
mod aborted;
#[path = "checkpoint_anchor_tests.rs"]
mod anchor;
#[path = "checkpoint_coalesce_tests.rs"]
mod coalesce;
#[path = "checkpoint_failpoint_tests.rs"]
mod failpoints;
#[path = "checkpoint_measurement_tests.rs"]
mod measurement;
#[path = "checkpoint_recorder_tests.rs"]
mod recorder;
#[path = "checkpoint_snapshot_tests.rs"]
mod snapshot;

const NOW: &str = "2026-09-17T00:00:00Z";

async fn working(store: &MongoSandboxStore, id: &str) -> Agent {
    let (parent, current) = fixture(store, id).await;
    start_turn(store, &parent, &current).await
}

async fn start_turn(
    store: &MongoSandboxStore,
    parent: &layer2_core::sandbox::Sandbox,
    current: &Agent,
) -> Agent {
    let mut mutation = update(current, "owner input");
    mutation
        .agent
        .transition(AgentStatus::Working, NOW.into(), None)
        .unwrap();
    mutation.agent.revision = current.revision + 1;
    let turn_sequence = current.conversation.last_seq + 1;
    mutation.agent.conversation.active_turn = Some(turn_sequence);
    mutation.agent.conversation.last_seq = current.conversation.last_seq + 2;
    mutation.agent_logs = vec![
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "owner input".into(),
        },
    ];
    mutation.admission_guard = Some(SandboxMutationGuard::capture(parent));
    store
        .commit_agent_conversation(mutation.clone())
        .await
        .unwrap();
    mutation.agent
}

fn update(current: &Agent, content: &str) -> AgentConversationMutation {
    let mut next = current.clone();
    next.revision += 1;
    next.conversation.last_seq += 1;
    next.updated_at = NOW.into();
    AgentConversationMutation {
        settings_fence: None,
        command_claim: None,
        execution_fence: None,
        trigger_execution_fence: None,
        admission_guard: None,
        expected_agent_revision: current.revision,
        expected_last_seq: current.conversation.last_seq,
        runtime_generation: current.conversation.runtime_generation.unwrap(),
        agent: next,
        appended_at: NOW.into(),
        agent_logs: vec![AgentLogKind::AgentMessage {
            content: content.into(),
        }],
    }
}

fn assembly(content: &str) -> AgentMessageAssembly {
    let mut value = serde_json::to_value(AgentMessageAssembly::default()).unwrap();
    value["message_id"] = serde_json::json!("message-1");
    value["content"] = serde_json::json!(content);
    serde_json::from_value(value).unwrap()
}

fn partial(current: &Agent, content: &str) -> AgentMessageCheckpoint {
    AgentMessageCheckpoint {
        version: 1,
        update_id: uuid::Uuid::new_v4().to_string(),
        update_fingerprint: "a".repeat(64),
        output: None,
        native_conversation_id: current.conversation.conversation_id.clone().unwrap(),
        runtime_generation: current.conversation.runtime_generation.unwrap(),
        active_turn: current.conversation.active_turn,
        after_seq: current.conversation.last_seq,
        assembly: assembly(content),
        outcome: AgentUpdateOutcome {
            appended: vec![],
            turn_closed: false,
        },
    }
}

fn boundary(mutation: &AgentConversationMutation) -> AgentMessageCheckpoint {
    let mut next = partial(&mutation.agent, "");
    next.assembly = AgentMessageAssembly::default();
    next.outcome.appended = conversation_logs(mutation).unwrap();
    next
}

#[test]
fn private_checkpoint_decoder_accepts_legacy_rows_and_rejects_corruption() {
    assert_eq!(checkpoint::read(&doc! {}).unwrap(), None);
    for value in [
        mongodb::bson::Bson::Null,
        "broken".into(),
        doc! { "version": 99 }.into(),
    ] {
        assert!(matches!(
            checkpoint::read(&doc! { checkpoint::FIELD: value }),
            Err(SandboxStoreError::Corrupt(_))
        ));
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn checkpoint_survives_reopen_without_public_revision_or_log_changes() {
    let (client, name, store) = database().await;
    let current = working(&store, "sbx_checkpoint_reopen").await;
    let logs = store.agent_log(&current.id, 0, 100).await.unwrap();
    assert!(store
        .agent_message_checkpoint(&current.id)
        .await
        .unwrap()
        .is_none());
    let mut checkpoint = partial(&current, "prefix before restart");
    checkpoint.output = Some(layer3_adapter::RuntimeOutputSource {
        stream_id: uuid::Uuid::new_v4().to_string(),
        cursor: 17,
        native_conversation_id: checkpoint.native_conversation_id.clone(),
        payload_sha256: "b".repeat(64),
    });
    store
        .commit_agent_message_update(current.clone(), None, checkpoint.clone(), None)
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .agent_message_checkpoint(&current.id)
            .await
            .unwrap(),
        Some(checkpoint)
    );
    assert_eq!(
        reopened.get_agent(&current.id).await.unwrap(),
        Some(current.clone())
    );
    assert_eq!(reopened.agent_log(&current.id, 0, 100).await.unwrap(), logs);
    let row = reopened
        .agents
        .find_one(doc! { "_id": current.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(row.contains_key(checkpoint::FIELD));
    assert!(!row
        .get_document("record")
        .unwrap()
        .contains_key(checkpoint::FIELD));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn checkpoint_boundary_is_atomic_and_exact_retry_never_appends_twice() {
    let (client, name, store) = database().await;
    let current = working(&store, "sbx_checkpoint_boundary").await;
    let previous = partial(&current, "prefix");
    store
        .commit_agent_message_update(current.clone(), None, previous.clone(), None)
        .await
        .unwrap();
    let mutation = update(&current, "prefix and final suffix");
    let next = boundary(&mutation);
    let mut inconsistent = next.clone();
    inconsistent.outcome.appended.clear();
    assert_eq!(
        store
            .commit_agent_message_update(
                current.clone(),
                Some(previous.clone()),
                inconsistent,
                Some(mutation.clone())
            )
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_agent(&current.id).await.unwrap(),
        Some(current.clone())
    );
    assert_eq!(
        store.agent_message_checkpoint(&current.id).await.unwrap(),
        Some(previous.clone())
    );
    assert_eq!(
        store.agent_log(&current.id, 0, 100).await.unwrap().len(),
        current.conversation.last_seq as usize
    );
    for _ in 0..2 {
        assert_eq!(
            store
                .commit_agent_message_update(
                    current.clone(),
                    Some(previous.clone()),
                    next.clone(),
                    Some(mutation.clone())
                )
                .await
                .unwrap(),
            next.outcome
        );
    }
    assert_eq!(
        store.get_agent(&current.id).await.unwrap(),
        Some(mutation.agent.clone())
    );
    assert_eq!(
        store.agent_log(&current.id, 0, 100).await.unwrap().last(),
        next.outcome.appended.last()
    );
    assert_eq!(
        store.agent_message_checkpoint(&current.id).await.unwrap(),
        Some(next.clone())
    );
    let mut reused_id = next;
    reused_id.update_fingerprint = "b".repeat(64);
    assert_eq!(
        store
            .commit_agent_message_update(current, Some(previous), reused_id, Some(mutation))
            .await,
        Err(SandboxStoreError::Conflict)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn checkpoint_fences_revision_sequence_generation_turn_and_previous_receipt() {
    let (client, name, store) = database().await;
    let current = working(&store, "sbx_checkpoint_fences").await;
    let previous = partial(&current, "prefix");
    store
        .commit_agent_message_update(current.clone(), None, previous.clone(), None)
        .await
        .unwrap();
    for variation in 0..7 {
        let mut stale = current.clone();
        let mut expected = Some(previous.clone());
        let mut next = partial(&current, "prefix suffix");
        match variation {
            0 => stale.revision += 1,
            1 => stale.conversation.last_seq += 1,
            2 => stale.conversation.runtime_generation = Some(2),
            3 => stale.conversation.active_turn = None,
            4 => expected = None,
            5 => next.runtime_generation = 2,
            6 => next.native_conversation_id = "another-conversation".into(),
            _ => unreachable!(),
        }
        assert_eq!(
            store
                .commit_agent_message_update(stale, expected, next, None)
                .await,
            Err(SandboxStoreError::Conflict),
            "variation {variation}"
        );
    }
    assert_eq!(
        store.agent_message_checkpoint(&current.id).await.unwrap(),
        Some(previous)
    );
    assert_eq!(store.get_agent(&current.id).await.unwrap(), Some(current));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn checkpoint_concurrent_quiet_updates_compare_the_private_previous_value() {
    let (client, name, store) = database().await;
    let current = working(&store, "sbx_checkpoint_concurrent").await;
    let left = partial(&current, "left");
    let right = partial(&current, "right");
    let (left_result, right_result) = tokio::join!(
        store.commit_agent_message_update(current.clone(), None, left.clone(), None),
        store.commit_agent_message_update(current.clone(), None, right.clone(), None),
    );
    let winner = match (left_result, right_result) {
        (Ok(_), Err(SandboxStoreError::Conflict)) => left,
        (Err(SandboxStoreError::Conflict), Ok(_)) => right,
        results => panic!("expected one private checkpoint winner: {results:?}"),
    };
    assert_eq!(
        store.agent_message_checkpoint(&current.id).await.unwrap(),
        Some(winner)
    );
    assert_eq!(store.get_agent(&current.id).await.unwrap(), Some(current));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn generic_conversation_writes_carry_partial_content_and_retain_invalidated_receipt() {
    let (client, name, store) = database().await;
    let current = working(&store, "sbx_checkpoint_carry").await;
    let previous = partial(&current, "unfinished prefix");
    store
        .commit_agent_message_update(current.clone(), None, previous.clone(), None)
        .await
        .unwrap();
    let mutation = update(&current, "separate control evidence");
    store
        .commit_agent_conversation(mutation.clone())
        .await
        .unwrap();
    let carried = store
        .agent_message_checkpoint(&current.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(carried.assembly, previous.assembly);
    assert!(carried.matches(&mutation.agent));
    let mut close = update(&mutation.agent, "turn closed elsewhere");
    close.agent.conversation.active_turn = None;
    close.agent.status = AgentStatus::Ready;
    store
        .commit_agent_conversation(close.clone())
        .await
        .unwrap();
    let invalidated = store
        .agent_message_checkpoint(&current.id)
        .await
        .unwrap()
        .unwrap();
    assert!(invalidated.assembly.is_empty());
    assert!(!invalidated.matches(&close.agent));
    assert_eq!(invalidated.update_id, previous.update_id);
    assert_eq!(
        store
            .commit_agent_message_update(current, None, previous.clone(), None)
            .await
            .unwrap(),
        previous.outcome
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn checkpoint_bounds_corruption_and_history_seal_fail_closed() {
    let (client, name, store) = database().await;
    let current = working(&store, "sbx_checkpoint_invalid").await;
    let oversized = partial(
        &current,
        &"x".repeat(layer2_core::sandbox::MAX_DURABLE_AGENT_MESSAGE_BYTES + 1),
    );
    assert!(matches!(
        store
            .commit_agent_message_update(current.clone(), None, oversized, None)
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    store
        .agents
        .update_one(
            doc! { "_id": current.id.as_str() },
            doc! { "$set": { "channel_history_seal": true } },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .commit_agent_message_update(current.clone(), None, partial(&current, "sealed"), None)
            .await,
        Err(SandboxStoreError::Conflict)
    );
    store.agents.update_one(doc! { "_id": current.id.as_str() }, doc! { "$unset": { "channel_history_seal": "" }, "$set": { checkpoint::FIELD: { "version": 99 } } }).await.unwrap();
    assert!(matches!(
        store.agent_message_checkpoint(&current.id).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert!(matches!(
        store
            .commit_agent_conversation(update(&current, "refused"))
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert_eq!(store.get_agent(&current.id).await.unwrap(), Some(current));
    client.database(&name).drop().await.unwrap();
}

#[path = "typed_fence_tests.rs"]
mod typed_fence;
