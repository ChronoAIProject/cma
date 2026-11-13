use super::*;

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn final_answer_seeks_exact_evidence_across_long_turns_and_later_appends() {
    for closed in [1, 10, 100, 1000] {
        let (_, db, store, agent, entries) = fixture(closed, true).await;
        let transcript = expected(&agent, &entries);
        let expected = transcript.turns[0].final_answer().unwrap();
        let measured = crate::store::mongo_read_telemetry::isolated_test(async {
            let answer = store
                .agent_log_final_answer(&agent, expected.sequence)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(answer.turn_sequence, 3);
            assert_eq!(&answer.message, expected);
            crate::store::mongo_read_telemetry::snapshot("agent_log")
        })
        .await;
        assert!(measured[2] <= 20, "bounded originals: {measured:?}");
        assert!(measured[3] < 32 * 1024, "bounded BSON: {measured:?}");
        for sequence in [0, 3, 4, 5, agent.conversation.last_seq + 1] {
            assert!(
                matches!(
                    store.agent_log_final_answer(&agent, sequence).await,
                    Err(SandboxStoreError::NotFound)
                ),
                "{sequence}"
            );
        }
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn final_answer_refuses_missing_or_mismatched_evidence_and_originals() {
    for fault in ["message", "turn", "foreign", "source", "projection"] {
        let (_, db, store, agent, entries) = fixture(2, true).await;
        let sequence = expected(&agent, &entries).turns[0]
            .final_answer()
            .unwrap()
            .sequence;
        match fault {
            "message" | "turn" => {
                store
                    .segment_logs
                    .evidence
                    .delete_many(doc! {"kind":fault})
                    .await
                    .unwrap();
            }
            "foreign" => {
                store
                    .segment_logs
                    .evidence
                    .update_one(
                        doc! {"kind":"message", "entity_key":sequence.to_string()},
                        doc! {"$set":{"incarnation":"foreign"}},
                    )
                    .await
                    .unwrap();
            }
            "source" => {
                store
                    .agent_log
                    .update_one(
                        doc! {"agent_id":agent.id.as_str(),"seq":sequence as i64},
                        doc! {"$set":{"record.kind.content":"changed"}},
                    )
                    .await
                    .unwrap();
            }
            _ => {
                store
                    .segment_logs
                    .projections
                    .delete_many(doc! {})
                    .await
                    .unwrap();
            }
        }
        assert!(
            matches!(
                store.agent_log_final_answer(&agent, sequence).await,
                Err(SandboxStoreError::Corrupt(_) | SandboxStoreError::SequenceConflict)
            ),
            "{fault}"
        );
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn final_answer_rejects_failed_active_gapped_empty_and_steered_turns() {
    use layer2_core::sandbox::{AgentOperationDelivery, AgentOperationKind};
    for outcome in ["failed", "active", "gap", "empty", "steered"] {
        let mut kinds = vec![
            AgentLogKind::ConversationOpened {
                conversation_id: "native".into(),
                runtime_generation: 1,
            },
            AgentLogKind::TurnStarted {
                turn_sequence: 3,
                origin_key: None,
                model: None,
            },
            AgentLogKind::OwnerMessage {
                content: "owner".into(),
            },
            AgentLogKind::AgentMessage {
                content: if outcome == "empty" { " " } else { "answer" }.into(),
            },
        ];
        match outcome {
            "gap" => kinds.push(AgentLogKind::ConversationGap { frames: 1 }),
            "steered" => kinds.extend([
                AgentLogKind::OperationDeliveryChanged {
                    operation_sequence: 6,
                    operation: AgentOperationKind::Steer,
                    state: AgentOperationDelivery::DispatchFenced,
                },
                AgentLogKind::OwnerMessage {
                    content: "steer".into(),
                },
            ]),
            _ => {}
        }
        match outcome {
            "active" => {}
            "failed" => kinds.push(AgentLogKind::TurnInterrupted { turn_sequence: 3 }),
            _ => kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 }),
        }
        let (db, store, agent, _) = custom(kinds, 1, (outcome == "active").then_some(3)).await;
        assert!(
            matches!(
                store.agent_log_final_answer(&agent, 5).await,
                Err(SandboxStoreError::NotFound)
            ),
            "{outcome}"
        );
        db.drop().await.unwrap();
    }
}
