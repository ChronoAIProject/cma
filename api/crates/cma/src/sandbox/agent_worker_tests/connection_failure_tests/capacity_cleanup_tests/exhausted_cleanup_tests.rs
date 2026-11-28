use super::*;
use layer2_core::sandbox::{AgentConversationError, AgentMessageAssembly};
use layer3_adapter::{
    ConversationUpdate, PromptIdentity, PromptResponse, RuntimeOutputReceipt, RuntimeOutputSource,
    TurnStopReason,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn full_stop_count_reserve_preserves_active_turn_until_owner_cleanup() {
    cleanup(false, false).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn full_stop_byte_reserve_preserves_active_turn_until_owner_cleanup() {
    cleanup(true, false).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn native_settlement_near_stop_count_ceiling_still_deletes_original_runtime() {
    cleanup(false, true).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn native_settlement_near_stop_byte_ceiling_still_deletes_original_runtime() {
    cleanup(true, true).await;
}

async fn cleanup(bytes: bool, settled: bool) {
    let (client, database, h) = fixture().await;
    let agent = h
        .admit_with_first_message("capacity-final-reserve", vec![], Some("execute once"))
        .await;
    for _ in 0..6 {
        h.run(&agent.id, SystemTime::now()).await;
    }
    let working = h.stored(&agent.id).await;
    assert!(working.conversation.active_turn.is_some());
    let full = crate::store::fill_agent_capacity_to_fixture(
        h.store.as_ref(),
        working,
        (!bytes).then_some(if settled { 10_060 } else { 10_064 }),
        bytes.then_some(crate::store::AGENT_STOP_CAPACITY_BYTES - if settled { 4096 } else { 0 }),
    )
    .await;
    let (acks, mut acknowledged) = tokio::sync::mpsc::channel(1);
    let update = ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(
            RuntimeOutputSource {
                stream_id: "a6111111-1111-4111-8111-111111111111".into(),
                cursor: 1,
                native_conversation_id: full.conversation.conversation_id.clone().unwrap(),
                payload_sha256: "f".repeat(64),
            },
            acks,
        ),
        update: Some(
            Box::new(ConversationUpdate::TurnEndedForPrompt {
                prompt_id: PromptIdentity::new(
                    full.conversation.conversation_id.as_deref().unwrap(),
                    full.conversation.runtime_generation.unwrap(),
                    full.conversation.active_turn.unwrap(),
                ),
                response: PromptResponse::new(TurnStopReason::EndTurn),
            })
            .into(),
        ),
    };
    let started = std::time::Instant::now();
    let result = crate::sandbox::record_update_for_test(
        h.updates.as_ref(),
        &agent.id,
        full.conversation.runtime_generation.unwrap(),
        &mut AgentMessageAssembly::default(),
        update,
    )
    .await;
    let current = h.stored(&agent.id).await;
    let effects_before = h.backend.requests().await.len();
    if settled {
        result.unwrap();
        assert_eq!(acknowledged.recv().await.unwrap().cursor, 1);
        assert_eq!(current.status, AgentStatus::Failed);
        assert!(current.conversation.active_turn.is_none());
    } else {
        assert_eq!(result, Err(AgentConversationError::CapacityExceeded));
        assert!(acknowledged.try_recv().is_err());
        assert_eq!(current, full);
        assert_eq!(current.status, AgentStatus::Working);
        h.store
            .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
                target: AgentControlTarget::from(&current),
                reason: AgentStopReason::Owner,
                requested_at: rfc3339_at(SystemTime::now()),
            })
            .await
            .unwrap();
        for _ in 0..3 {
            h.run(&agent.id, SystemTime::now()).await;
        }
        assert_eq!(h.stored(&agent.id).await.status, AgentStatus::Stopped);
    }
    let current = h.stored(&agent.id).await;
    h.store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&current),
            requested_at: rfc3339_at(SystemTime::now()),
        })
        .await
        .unwrap();
    for _ in 0..4 {
        h.run(&agent.id, SystemTime::now()).await;
    }
    let deleted = h.stored(&agent.id).await;
    assert_eq!(deleted.status, AgentStatus::Deleted);
    assert_eq!(deleted.work.step, AgentWorkStep::None);
    assert!(deleted.work.lease.is_none());
    assert_eq!(
        deleted.conversation.conversation_id,
        full.conversation.conversation_id
    );
    let effects: Vec<_> = h.backend.requests().await[effects_before..]
        .iter()
        .map(|(_, r)| r.operation())
        .collect();
    assert_eq!(
        effects,
        [
            WireOperation::CloseAgent,
            WireOperation::RemoveAgentHome,
            WireOperation::RemoveWorktree
        ]
    );
    let snapshot = h.backend.snapshot(&h.runtime).await.unwrap();
    assert!(!snapshot
        .homes
        .contains_key(&AgentSlot::new(agent.id.to_string())));
    assert!(!snapshot
        .worktrees
        .contains_key(&("project".into(), "wt-1".into())));
    assert!(started.elapsed() < Duration::from_secs(120));
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    assert_eq!(reopened.get_agent(&agent.id).await.unwrap(), Some(deleted));
    let old_tail = reopened
        .agent_log(&agent.id, full.conversation.last_seq - 1, 1)
        .await
        .unwrap();
    assert!(
        matches!(&old_tail[0].kind,AgentLogKind::ToolOutputReported {output,..} if !output.text.is_empty())
    );
    client.database(&database).drop().await.unwrap();
}
