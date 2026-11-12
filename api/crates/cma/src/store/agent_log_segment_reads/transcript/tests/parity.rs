use super::*;
use layer2_core::sandbox::{
    AgentOperationDelivery, AgentOperationKind, AgentUsage, AgentUsageReading,
};

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn serialized_transcript_title_usage_parity_for_terminals_metadata_steering_and_generations()
{
    for outcome in [
        "completed",
        "failed",
        "interrupted",
        "gap",
        "steering",
        "active",
    ] {
        let mut kinds = vec![
            AgentLogKind::ConversationOpened {
                conversation_id: "native".into(),
                runtime_generation: 1,
            },
            AgentLogKind::TurnStarted {
                turn_sequence: 3,
                origin_key: None,
                model: Some("model".into()),
            },
            AgentLogKind::OwnerMessage {
                content: "owner 多".repeat(1000),
            },
            AgentLogKind::TurnMetadataSet {
                turn_sequence: 3,
                metadata: [("key".into(), "value".into())].into(),
            },
            AgentLogKind::AgentMessage {
                content: "first answer".into(),
            },
        ];
        match outcome {
            "gap" => kinds.push(AgentLogKind::ConversationGap { frames: 7 }),
            "steering" => {
                kinds.push(AgentLogKind::OperationDeliveryChanged {
                    operation_sequence: 7,
                    operation: AgentOperationKind::Steer,
                    state: AgentOperationDelivery::DispatchFenced,
                });
                kinds.push(AgentLogKind::OwnerMessage {
                    content: "steer".into(),
                });
            }
            _ => {}
        }
        kinds.push(AgentLogKind::AgentMessage {
            content: "final 多".repeat(1000),
        });
        match outcome {
            "failed" => kinds.push(AgentLogKind::TurnFailed {
                turn_sequence: 3,
                failure: layer2_core::sandbox::TypedFailure {
                    class: "failed".into(),
                    detail: "detail".into(),
                },
            }),
            "interrupted" => kinds.push(AgentLogKind::TurnInterrupted { turn_sequence: 3 }),
            "active" => {}
            _ => kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 }),
        }
        kinds.push(AgentLogKind::Usage {
            input_tokens: 8,
            output_tokens: 13,
            context_used: 21,
            context_window: Some(200),
        });
        kinds.push(AgentLogKind::Usage {
            input_tokens: 5,
            output_tokens: 9,
            context_used: 0,
            context_window: None,
        });
        let generation = if outcome == "active" {
            1
        } else {
            kinds.push(AgentLogKind::RuntimeGenerationRetired {
                previous_generation: 1,
                runtime_generation: 2,
            });
            kinds.push(AgentLogKind::ConversationOpened {
                conversation_id: "native".into(),
                runtime_generation: 3,
            });
            kinds.push(AgentLogKind::Usage {
                input_tokens: 2,
                output_tokens: 3,
                context_used: 5,
                context_window: Some(100),
            });
            3
        };
        let (db, store, agent, entries) =
            custom(kinds, generation, (outcome == "active").then_some(3)).await;
        let full = store
            .agent_log_transcript_projection(&agent)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_vec(&full).unwrap(),
            serde_json::to_vec(&expected(&agent, &entries)).unwrap(),
            "{outcome}"
        );
        for limit in [1, 4, 32] {
            assert_eq!(
                serde_json::to_vec(&collect(&store, &agent, limit).await).unwrap(),
                serde_json::to_vec(&full).unwrap(),
                "{outcome}, {limit}"
            );
        }
        let mut session = store.client.start_session().snapshot(true).await.unwrap();
        let title =
            crate::store::agent_titles::transcript_at_snapshot(&store, &agent, &mut session)
                .await
                .unwrap();
        assert_eq!(
            serialize_title(title),
            serialize_title(AgentTitleTranscript::from_agent_log(&agent, &entries)),
            "{outcome}"
        );
        let usage = store
            .agent_log_usage_projection(&agent)
            .await
            .unwrap()
            .unwrap();
        let expected = AgentUsageReading {
            agent_id: agent.id.clone(),
            through_seq: agent.conversation.last_seq,
            usage: if generation == 1 {
                AgentUsage {
                    input_tokens: 8,
                    output_tokens: 13,
                    context_used: 21,
                    context_window: Some(200),
                    current_turn: None,
                }
            } else {
                AgentUsage {
                    input_tokens: 10,
                    output_tokens: 16,
                    context_used: 5,
                    context_window: Some(100),
                    current_turn: None,
                }
            },
        };
        assert_eq!(
            serde_json::to_vec(&usage).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn queued_turns_seek_in_numeric_order_and_do_not_repeat_the_original_owner() {
    let mut kinds = vec![AgentLogKind::ConversationOpened {
        conversation_id: "native".into(),
        runtime_generation: 1,
    }];
    for _ in 0..8 {
        let owner = kinds.len() as u64 + 2;
        kinds.push(AgentLogKind::OwnerMessage {
            content: format!("queued {owner}"),
        });
        let turn = owner + 1;
        kinds.push(AgentLogKind::TurnStarted {
            turn_sequence: turn,
            origin_key: Some(format!("queued-message:{owner}")),
            model: None,
        });
        kinds.push(AgentLogKind::AgentMessage {
            content: format!("answer {turn}"),
        });
        kinds.push(AgentLogKind::TurnCompleted {
            turn_sequence: turn,
        });
    }
    let (db, store, agent, entries) = custom(kinds, 1, None).await;
    let expected = expected(&agent, &entries);
    for limit in [1, 2, 32] {
        assert_eq!(collect(&store, &agent, limit).await, expected);
    }
    let mut session = store.client.start_session().snapshot(true).await.unwrap();
    assert_eq!(
        crate::store::agent_titles::transcript_at_snapshot(&store, &agent, &mut session)
            .await
            .unwrap(),
        None
    );
    db.drop().await.unwrap();
}

fn serialize_title(title: Option<AgentTitleTranscript>) -> Vec<u8> {
    serde_json::to_vec(&title.map(|value| {
        (
            value.turn_sequence(),
            value.owner_message().to_owned(),
            value.assistant_message().to_owned(),
        )
    }))
    .unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn title_skips_setup_but_never_replaces_an_ineligible_first_task_turn() {
    for failed in [false, true] {
        let mut kinds = vec![AgentLogKind::ConversationOpened {
            conversation_id: "native".into(),
            runtime_generation: 1,
        }];
        for n in 0..3 {
            let turn = kinds.len() as u64 + 2;
            kinds.extend([
                AgentLogKind::TurnStarted {
                    turn_sequence: turn,
                    origin_key: None,
                    model: None,
                },
                AgentLogKind::OwnerMessage {
                    content: format!("owner {n}"),
                },
                AgentLogKind::AgentMessage {
                    content: format!("answer {n}"),
                },
                if failed && n == 1 {
                    AgentLogKind::TurnInterrupted {
                        turn_sequence: turn,
                    }
                } else {
                    AgentLogKind::TurnCompleted {
                        turn_sequence: turn,
                    }
                },
            ]);
        }
        let (db, store, agent, entries) = custom_with_setup(kinds, 1, None, true).await;
        let mut session = store.client.start_session().snapshot(true).await.unwrap();
        let title =
            crate::store::agent_titles::transcript_at_snapshot(&store, &agent, &mut session)
                .await
                .unwrap();
        assert_eq!(
            serialize_title(title.clone()),
            serialize_title(AgentTitleTranscript::from_agent_log(&agent, &entries))
        );
        assert_eq!(title.is_none(), failed);
        if let Some(title) = title {
            assert_eq!(title.owner_message(), "owner 1");
        }
        db.drop().await.unwrap();
    }
}
