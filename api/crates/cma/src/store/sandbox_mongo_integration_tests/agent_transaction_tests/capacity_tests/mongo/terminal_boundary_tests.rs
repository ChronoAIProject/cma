use super::*;
use crate::store::sandbox_mongo_integration_tests::capacity_fixtures::fill_to;
use layer2_core::sandbox::{
    AgentConversationError, AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use layer3_adapter::{
    ConversationUpdate, PromptIdentity, PromptResponse, RuntimeOutputReceipt, RuntimeOutputSource,
    TurnStopReason,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn terminal_count_ceiling_accepts_exactly_and_refuses_one_over() {
    boundary(false).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn terminal_byte_ceiling_accepts_exactly_and_refuses_one_over() {
    boundary(true).await;
}

async fn boundary(bytes: bool) {
    for over in [false, true] {
        let (client, name) = database("terminal_exact_bound").await;
        let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        let ready = ready_conversation_fixture(&store, "terminal_exact_bound").await;
        let start = begin_stored_conversation_turn(&store, ready).await;
        store
            .commit_agent_conversation(start.clone())
            .await
            .unwrap();
        let turn = start.agent.conversation.active_turn.unwrap();
        let kinds = [
            AgentLogKind::TurnCompleted {
                turn_sequence: turn,
            },
            AgentLogKind::StatusChanged {
                from: AgentStatus::Working,
                to: AgentStatus::Failed,
            },
        ];
        let pending_bytes: usize = kinds
            .into_iter()
            .enumerate()
            .map(|(i, kind)| {
                log_document(&AgentLogEntry {
                    agent_id: start.agent.id.clone(),
                    seq: 10_063 + i as u64,
                    appended_at: MINUTE.into(),
                    kind,
                })
                .unwrap()
                .to_vec()
                .unwrap()
                .len()
            })
            .sum();
        let full = fill_to(
            &store,
            start.agent,
            (!bytes).then_some(10_062 + u64::from(over)),
            bytes.then_some(
                crate::store::agent_log_integrity::STOP_AGENT_LOG_BYTES - pending_bytes
                    + usize::from(over),
            ),
        )
        .await;
        let before = fingerprint(&store, &full.id, full.conversation.last_seq).await;
        let (acks, mut received) = tokio::sync::mpsc::channel(1);
        let update = ConversationUpdate::RuntimeOutput {
            receipt: RuntimeOutputReceipt::new(
                RuntimeOutputSource {
                    stream_id: "c8111111-1111-4111-8111-111111111111".into(),
                    cursor: 1,
                    native_conversation_id: full.conversation.conversation_id.clone().unwrap(),
                    payload_sha256: "a".repeat(64),
                },
                acks,
            ),
            update: Some(
                Box::new(ConversationUpdate::TurnEndedForPrompt {
                    prompt_id: PromptIdentity::new(
                        full.conversation.conversation_id.as_deref().unwrap(),
                        1,
                        turn,
                    ),
                    response: PromptResponse::new(TurnStopReason::EndTurn),
                })
                .into(),
            ),
        };
        // Core recorder does not ACK on behalf of its caller. Check transaction
        // result here and keep the production ACK gate in transaction tests.
        let result = DurableAgentUpdateRecorder::new(store.clone())
            .record_update(
                &full.id,
                1,
                &mut AgentMessageAssembly::default(),
                update,
                MINUTE.into(),
            )
            .await;
        assert!(received.try_recv().is_err());
        if over {
            assert_eq!(result, Err(AgentConversationError::CapacityExceeded));
            assert_eq!(store.get_agent(&full.id).await.unwrap(), Some(full.clone()));
        } else {
            let outcome = result.unwrap();
            assert!(outcome.turn_closed);
            let saved = store.get_agent(&full.id).await.unwrap().unwrap();
            assert_eq!(saved.status, AgentStatus::Failed);
            if bytes {
                use futures_util::TryStreamExt;
                let mut rows = store
                    .agent_log
                    .find(doc! {"agent_id":full.id.as_str()})
                    .await
                    .unwrap();
                let mut total = 0;
                while let Some(row) = rows.try_next().await.unwrap() {
                    total += row.to_vec().unwrap().len();
                }
                assert_eq!(
                    total,
                    crate::store::agent_log_integrity::STOP_AGENT_LOG_BYTES
                );
            } else {
                assert_eq!(saved.conversation.last_seq, 10_064);
            }
        }
        assert_eq!(
            fingerprint(&store, &full.id, full.conversation.last_seq).await,
            before
        );
        client.database(&name).drop().await.unwrap();
    }
}
