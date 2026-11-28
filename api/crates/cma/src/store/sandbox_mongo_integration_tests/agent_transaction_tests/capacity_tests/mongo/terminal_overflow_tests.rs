use super::*;
use crate::store::agent_log_integrity::STOP_AGENT_LOG_BYTES;
use crate::store::sandbox_mongo_integration_tests::capacity_fixtures::fill_to;
use layer2_core::sandbox::{
    AgentConversationError, AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use layer3_adapter::{
    ContentBlock, ContentChunk, ConversationUpdate, PromptIdentity, PromptResponse,
    RuntimeOutputReceipt, RuntimeOutputSource, SessionUpdate, TurnStopReason,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn terminal_count_overflow_keeps_private_prefix_and_fits_exact_failure_boundary() {
    overflow(false, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn terminal_byte_overflow_keeps_one_mib_prefix_and_publishes_capacity_failure() {
    overflow(true, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn refused_output_keeps_one_mib_prefix_without_consuming_failure_reserve() {
    overflow(true, true).await;
}

async fn overflow(bytes: bool, refused: bool) {
    let (client, name) = database("capacity_overflow").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let ready = ready_conversation_fixture(&store, "capacity_overflow").await;
    let start = begin_stored_conversation_turn(&store, ready).await;
    store
        .commit_agent_conversation(start.clone())
        .await
        .unwrap();
    let full = fill_to(
        &store,
        start.agent,
        (!bytes).then_some(10_062),
        bytes.then_some(STOP_AGENT_LOG_BYTES - 4096),
    )
    .await;
    let before_hash = fingerprint(&store, &full.id, full.conversation.last_seq).await;
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let mut assembly = AgentMessageAssembly::default();
    let (acks, mut acknowledged) = tokio::sync::mpsc::channel(2);
    let source = |cursor| RuntimeOutputSource {
        stream_id: "95111111-1111-4111-8111-111111111111".into(),
        cursor,
        native_conversation_id: full.conversation.conversation_id.clone().unwrap(),
        payload_sha256: "e".repeat(64),
    };
    crate::sandbox::record_update_for_test(
        recorder.as_ref(),
        &full.id,
        1,
        &mut assembly,
        ConversationUpdate::RuntimeOutput {
            receipt: RuntimeOutputReceipt::new(source(1), acks.clone()),
            update: Some(
                Box::new(ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(
                    ContentChunk::new(ContentBlock::from("x".repeat(1024 * 1024))),
                )))
                .into(),
            ),
        },
    )
    .await
    .unwrap();
    assert_eq!(acknowledged.recv().await.unwrap().cursor, 1);
    let prefix = store
        .agent_message_checkpoint(&full.id)
        .await
        .unwrap()
        .unwrap();
    let before_row = store
        .agents
        .find_one(doc! {"_id": full.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let terminal = ConversationUpdate::TurnEndedForPrompt {
        prompt_id: PromptIdentity::new(
            full.conversation.conversation_id.as_deref().unwrap(),
            1,
            full.conversation.active_turn.unwrap(),
        ),
        response: PromptResponse::new(TurnStopReason::EndTurn),
    };
    let inner = if refused {
        ConversationUpdate::Said(SessionUpdate::ToolCallUpdate(
            layer3_adapter::ToolCallUpdate::new(
                "refused",
                layer3_adapter::ToolCallUpdateFields::new().title("refused output"),
            ),
        ))
    } else {
        terminal
    };
    let update = ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(source(2), acks),
        update: Some(Box::new(inner).into()),
    };
    let started = std::time::Instant::now();
    let result = crate::sandbox::record_update_for_test(
        recorder.as_ref(),
        &full.id,
        1,
        &mut assembly,
        update.clone(),
    )
    .await;
    assert_eq!(result, Err(AgentConversationError::CapacityExceeded));
    assert!(started.elapsed() < std::time::Duration::from_secs(120));
    assert!(
        acknowledged.try_recv().is_err(),
        "refused output must not be ACKed"
    );
    let saved = store.get_agent(&full.id).await.unwrap().unwrap();
    assert_eq!(saved.status, AgentStatus::Failed);
    assert!(saved.conversation.active_turn.is_none());
    assert!(saved.work.lease.is_none());
    assert_eq!(saved.conversation.last_seq, full.conversation.last_seq + 2);
    let row = store
        .agents
        .find_one(doc! {"_id": full.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.get("agent_capacity_prefix_v1"),
        before_row.get("agent_message_checkpoint")
    );
    let retained: layer2_core::sandbox::store::AgentMessageCheckpoint =
        mongodb::bson::deserialize_from_bson(row["agent_capacity_prefix_v1"].clone()).unwrap();
    assert_eq!(retained, prefix);
    let checkpoint = store
        .agent_message_checkpoint(&full.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.output, prefix.output);
    assert_eq!(checkpoint.output.as_ref().unwrap().cursor, 1);
    assert!(checkpoint.assembly.is_empty());
    let tail = store
        .agent_log(&full.id, full.conversation.last_seq, 20)
        .await
        .unwrap();
    assert_eq!(
        tail.iter()
            .filter(|e| matches!(e.kind, AgentLogKind::TurnFailed { .. }))
            .count(),
        1
    );
    assert!(
        matches!(&tail[0].kind, AgentLogKind::TurnFailed { failure, .. }
        if failure.class == "agent_log_capacity_exceeded" && failure.detail.contains("retained privately") && failure.detail.contains("support") && failure.detail.contains("operator export"))
    );
    let peer = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    assert!(DurableAgentUpdateRecorder::new(peer)
        .record_update(
            &full.id,
            1,
            &mut AgentMessageAssembly::default(),
            update,
            MINUTE.into()
        )
        .await
        .is_err());
    assert_eq!(
        store.get_agent(&full.id).await.unwrap(),
        Some(saved.clone())
    );
    assert_eq!(
        fingerprint(&store, &full.id, full.conversation.last_seq).await,
        before_hash
    );
    let mut revised = saved.clone();
    revised.revision += 1;
    store
        .save_agent(saved.revision, revised.clone())
        .await
        .unwrap();
    store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&revised),
            requested_at: TWO_MINUTES.into(),
        })
        .await
        .unwrap();
    let row = store
        .agents
        .find_one(doc! {"_id":full.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.get("agent_capacity_prefix_v1"),
        before_row.get("agent_message_checkpoint")
    );
    client.database(&name).drop().await.unwrap();
}
