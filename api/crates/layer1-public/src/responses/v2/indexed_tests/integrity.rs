use super::*;

#[tokio::test]
async fn malformed_coordinates_and_capture_identities_are_refused() {
    for damage in 0..9 {
        let mut kinds = beginning();
        kinds.extend([
            message("first"),
            message("second"),
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ]);
        let mut f = make(kinds).await;
        f.mutate(|data| match damage {
            0 => data.changes[0].ordinal = 99,
            1 => data.changes[0].source_sequence = 1,
            2 => data.changes[0].after.nonempty_messages = 0,
            3 => data.changes[1].before.messages = 0,
            4 => data.changes.pop().map(|_| ()).unwrap(),
            5 => data.metadata.created_at = "2026-01-01T00:00:00Z".into(),
            6 => data.metadata.index.as_mut().unwrap().agent_id = AgentId("agt_foreign".into()),
            7 => data.metadata.index.as_mut().unwrap().through_sequence += 1,
            _ => {
                data.rows.get_mut(&5).unwrap().agent_id = AgentId("agt_foreign".into());
            }
        });
        let reading = f.reading.clone();
        let result = f
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
            .await;
        assert!(
            matches!(result, Err(AgentReadError::Corrupt(_))),
            "damage {damage}"
        );
    }
}

#[tokio::test]
async fn invalid_tool_references_cannot_change_current_output_or_replay() {
    for damage in 0..3 {
        let mut kinds = beginning();
        kinds.extend([
            tool("call", "running"),
            output("call", "private"),
            update("call", "completed"),
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ]);
        let mut f = make(kinds).await;
        f.mutate(|data| {
            let tool = data.tools.get_mut("call").unwrap();
            match damage {
                0 => tool.turn_sequence += 1,
                1 => tool.output_sequence = Some(1),
                _ => tool.terminal_sequence = Some(99),
            }
        });
        assert!(matches!(
            f.observe(|source| Box::pin(async move {
                projection::indexed::output_page(source, 0)
                    .await
                    .map(|_| ())
            }))
            .await,
            Err(AgentReadError::Corrupt(_))
        ));
    }
}

#[tokio::test]
async fn source_metadata_must_match_its_certified_reference_and_payload() {
    for damage in 0..4 {
        let mut kinds = beginning();
        kinds.extend([
            message("answer"),
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
            AgentLogKind::Usage {
                input_tokens: 7,
                output_tokens: 11,
                context_used: 18,
                context_window: None,
            },
        ]);
        let mut f = make(kinds).await;
        f.mutate(|data| match damage {
            0 => data.metadata.turn.as_mut().unwrap().facts.model = Some("wrong-model".into()),
            1 => {
                data.metadata
                    .turn
                    .as_mut()
                    .unwrap()
                    .facts
                    .response_usage
                    .as_mut()
                    .unwrap()
                    .output_tokens += 1
            }
            2 => data.rows.get_mut(&6).unwrap().appended_at = "2026-09-06T00:00:00Z".into(),
            _ => {
                data.rows.get_mut(&7).unwrap().kind = AgentLogKind::ModeChanged {
                    mode: "wrong-kind".into(),
                }
            }
        });
        let reading = f.reading.clone();
        assert!(matches!(
            f.observe(move |source| Box::pin(async move {
                projection::indexed::complete(
                    source,
                    &crate::agent_page::AgentPageOrigin::test(),
                    reading,
                )
                .await
            }))
            .await,
            Err(AgentReadError::Corrupt(_))
        ));
    }
}
