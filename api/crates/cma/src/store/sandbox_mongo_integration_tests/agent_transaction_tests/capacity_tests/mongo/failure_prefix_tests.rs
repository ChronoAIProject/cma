use super::*;
use crate::store::sandbox_mongo_integration_tests::capacity_fixtures::fill_to;
use futures_util::TryStreamExt;
use layer2_core::sandbox::{
    AgentConversationError, AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use layer3_adapter::{
    ContentBlock, ContentChunk, ConversationUpdate, RuntimeOutputReceipt, RuntimeOutputSource,
    SessionUpdate, ToolCallUpdate, ToolCallUpdateFields,
};

#[derive(Clone, Copy)]
enum Capacity {
    Count(u64),
    Bytes(usize),
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn fitting_live_prefix_remains_visible_after_capacity_failure_and_reload() {
    run(Capacity::Count(10_000), true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failure_prefix_count_boundary_publishes_exactly_and_retains_one_over() {
    run(Capacity::Count(10_061), true).await;
    run(Capacity::Count(10_062), false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failure_prefix_byte_boundary_publishes_exactly_and_retains_one_over() {
    // Measure the real encoded failed batch, then fill a fresh history so it
    // fits exactly. IDs/timestamp widths and padded sequence IDs are identical.
    let reference = run(Capacity::Count(10_000), true).await;
    let bytes: usize = reference
        .iter()
        .map(|entry| log_document(entry).unwrap().to_vec().unwrap().len())
        .sum();
    let remaining = crate::store::agent_log_integrity::STOP_AGENT_LOG_BYTES - bytes;
    let exact = run(Capacity::Bytes(remaining), true).await;
    assert_eq!(
        exact
            .iter()
            .map(|entry| log_document(entry).unwrap().to_vec().unwrap().len())
            .sum::<usize>(),
        bytes
    );
    run(Capacity::Bytes(remaining + 1), false).await;
}

async fn run(capacity: Capacity, visible: bool) -> Vec<AgentLogEntry> {
    let (client, name) = database("failure_prefix_visibility").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let ready = ready_conversation_fixture(&store, "failure_prefix").await;
    let start = begin_stored_conversation_turn(&store, ready).await;
    store
        .commit_agent_conversation(start.clone())
        .await
        .unwrap();
    let (count, bytes) = match capacity {
        Capacity::Count(value) => (Some(value), None),
        Capacity::Bytes(value) => (None, Some(value)),
    };
    let full = fill_to(&store, start.agent, count, bytes).await;
    let history = fingerprint(&store, &full.id, full.conversation.last_seq).await;
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let mut live = recorder.subscribe_live(&full.id).unwrap();
    let mut assembly = AgentMessageAssembly::default();
    let prefix_text = "Already visible partial output — 已显示。".repeat(64);
    let (acks, mut acknowledged) = tokio::sync::mpsc::channel(2);
    let retained = |cursor, update: ConversationUpdate| ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(
            RuntimeOutputSource {
                stream_id: "d9111111-1111-4111-8111-111111111111".into(),
                cursor,
                native_conversation_id: full.conversation.conversation_id.clone().unwrap(),
                payload_sha256: "a".repeat(64),
            },
            acks.clone(),
        ),
        update: Some(Box::new(update).into()),
    };
    crate::sandbox::record_update_for_test(
        recorder.as_ref(),
        &full.id,
        1,
        &mut assembly,
        retained(
            1,
            ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                ContentBlock::from(prefix_text.clone()),
            ))),
        ),
    )
    .await
    .unwrap();
    assert_eq!(acknowledged.recv().await.unwrap().cursor, 1);
    assert_eq!(
        live.try_recv().unwrap().delta,
        prefix_text,
        "the owner already saw this committed fragment"
    );
    let before = store
        .agent_message_checkpoint(&full.id)
        .await
        .unwrap()
        .unwrap();
    let refused = retained(
        2,
        ConversationUpdate::Said(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "refused",
            ToolCallUpdateFields::new().title("Unrecorded tool output"),
        ))),
    );
    assert_eq!(
        crate::sandbox::record_update_for_test(
            recorder.as_ref(),
            &full.id,
            1,
            &mut assembly,
            refused.clone()
        )
        .await,
        Err(AgentConversationError::CapacityExceeded)
    );
    assert!(
        acknowledged.try_recv().is_err(),
        "the refused frame must not be ACKed"
    );
    let reopened = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let saved = reopened.get_agent(&full.id).await.unwrap().unwrap();
    assert_eq!(saved.status, AgentStatus::Failed);
    assert!(saved.conversation.active_turn.is_none());
    let tail = reopened
        .agent_log(&full.id, full.conversation.last_seq, 20)
        .await
        .unwrap();
    assert_eq!(
        tail.iter()
            .filter(|e| matches!(e.kind, AgentLogKind::TurnFailed { .. }))
            .count(),
        1
    );
    assert!(!tail
        .iter()
        .any(|e| matches!(e.kind, AgentLogKind::TurnCompleted { .. })));
    let failed = if visible {
        assert_eq!(tail.len(), 3);
        assert!(
            matches!(&tail[0].kind, AgentLogKind::AgentMessage { content } if content == &prefix_text)
        );
        &tail[1]
    } else {
        assert_eq!(tail.len(), 2);
        assert!(!tail
            .iter()
            .any(|e| matches!(e.kind, AgentLogKind::AgentMessage { .. })));
        &tail[0]
    };
    assert!(
        matches!(&failed.kind, AgentLogKind::TurnFailed { failure, .. } if failure.class == "agent_log_capacity_exceeded")
    );
    let row = reopened
        .agents
        .find_one(doc! {"_id":full.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    if visible {
        assert!(!row.contains_key("agent_capacity_prefix_v1"));
    } else {
        assert_eq!(
            row.get("agent_capacity_prefix_v1"),
            Some(&mongodb::bson::serialize_to_bson(&before).unwrap())
        );
        assert!(saved.failure.as_ref().unwrap().detail.contains("recover"));
    }
    let after = reopened
        .agent_message_checkpoint(&full.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.output, before.output);
    assert!(DurableAgentUpdateRecorder::new(reopened.clone())
        .record_update(
            &full.id,
            1,
            &mut AgentMessageAssembly::default(),
            refused,
            MINUTE.into()
        )
        .await
        .is_err());
    assert_eq!(
        reopened
            .agent_log(&full.id, full.conversation.last_seq, 20)
            .await
            .unwrap(),
        tail
    );
    assert_eq!(
        fingerprint(&reopened, &full.id, full.conversation.last_seq).await,
        history
    );
    if let Capacity::Bytes(before_bytes) = capacity {
        let mut cursor = reopened
            .agent_log
            .find(doc! {"agent_id": full.id.as_str()})
            .await
            .unwrap();
        let mut total = 0;
        while let Some(row) = cursor.try_next().await.unwrap() {
            total += row.to_vec().unwrap().len();
        }
        let appended: usize = tail
            .iter()
            .map(|e| log_document(e).unwrap().to_vec().unwrap().len())
            .sum();
        assert_eq!(total, before_bytes + appended);
        if visible {
            assert_eq!(
                total,
                crate::store::agent_log_integrity::STOP_AGENT_LOG_BYTES
            );
        } else {
            assert!(total <= crate::store::agent_log_integrity::STOP_AGENT_LOG_BYTES);
        }
    }
    client.database(&name).drop().await.unwrap();
    tail
}
