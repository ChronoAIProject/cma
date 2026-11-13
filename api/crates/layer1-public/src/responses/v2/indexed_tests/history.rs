use super::*;

#[tokio::test]
async fn historical_models_and_usage_stop_at_the_next_original_turn() {
    for (changed, explicit) in [(false, false), (true, false), (true, true)] {
        let mut kinds = vec![AgentLogKind::ModeChanged {
            mode: "code".into(),
        }];
        if changed {
            kinds[0] = AgentLogKind::OperationDeliveryChanged {
                operation_sequence: 1,
                operation: layer2_core::sandbox::AgentOperationKind::UpdateSettings,
                state: layer2_core::sandbox::AgentOperationDelivery::WriteConfirmed,
            };
        }
        kinds.extend([
            AgentLogKind::TurnStarted {
                turn_sequence: 4,
                origin_key: None,
                model: explicit.then(|| "historical-model".into()),
            },
            AgentLogKind::OwnerMessage {
                content: "question".into(),
            },
            message("old result"),
            AgentLogKind::TurnCompleted { turn_sequence: 4 },
            AgentLogKind::Usage {
                input_tokens: 7,
                output_tokens: 11,
                context_used: 18,
                context_window: None,
            },
            AgentLogKind::TurnStarted {
                turn_sequence: 9,
                origin_key: None,
                model: Some("new-model".into()),
            },
            AgentLogKind::OwnerMessage {
                content: "new question".into(),
            },
            message("new result"),
            AgentLogKind::Usage {
                input_tokens: 19,
                output_tokens: 23,
                context_used: 42,
                context_window: None,
            },
            AgentLogKind::TurnCompleted { turn_sequence: 9 },
        ]);
        let f = make_turn(kinds, 4).await;
        let expected = f.golden();
        assert_eq!(
            expected.model,
            if explicit {
                "historical-model"
            } else if changed {
                "unknown"
            } else {
                "creation-model"
            }
        );
        assert_eq!(expected.usage.as_ref().unwrap().output_tokens, 11);
        let reading = f.reading.clone();
        let actual = f
            .observe(move |source| {
                Box::pin(async move {
                    projection::indexed::complete(
                        source,
                        &crate::agent_page::AgentPageOrigin::test(),
                        reading,
                    )
                    .await
                })
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }
}
