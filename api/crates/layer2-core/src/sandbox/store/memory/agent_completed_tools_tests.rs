//! Incident-shaped terminal propagation: completed tools are not turn completion.
use super::*;

#[tokio::test]
async fn completed_tools_then_reconnect_preserve_one_identified_terminal_in_1000_trials() {
    let mut lags = Vec::new();
    for seed in 0..1000 {
        let terminal_first = seed & 1 != 0;
        let final_text = seed & 2 != 0;
        let failed = seed & 4 != 0;
        let (store, working, recorder) = working_agent().await;
        let mut old = AgentMessageAssembly::default();
        let mut cursor = 0;
        let mut updates = vec![message_chunk("ack", "I will read the unchanged fixture.")];
        for call in ["read", "calculate"] {
            updates.push(ConversationUpdate::Said(SessionUpdate::ToolCall(
                layer3_adapter::ToolCall::new(call, call),
            )));
            updates.push(ConversationUpdate::Said(SessionUpdate::ToolCallUpdate(
                ToolCallUpdate::new(
                    call,
                    ToolCallUpdateFields::new().status(layer3_adapter::ToolCallStatus::Completed),
                ),
            )));
        }
        updates.push(ConversationUpdate::Usage(
            layer3_adapter::ConversationUsage {
                turn_id: "11111111-1111-4111-8111-111111111111".into(),
                input_tokens: 40,
                output_tokens: 12,
                last_input_tokens: 40,
                last_output_tokens: 12,
                context_used: 40,
                context_window: Some(1000),
                counters_reset: false,
            },
        ));
        for update in updates {
            cursor += 1;
            recorder
                .record_update(
                    &working.id,
                    1,
                    &mut old,
                    retained(cursor, Some(update)),
                    NOW.into(),
                )
                .await
                .unwrap();
        }
        let before = store.get_agent(&working.id).await.unwrap().unwrap();
        assert_eq!(before.status, AgentStatus::Working);
        assert_eq!(
            before.conversation.active_turn,
            working.conversation.active_turn
        );
        assert!(matches!(
            store
                .agent_log(&working.id, 0, 200)
                .await
                .unwrap()
                .last()
                .unwrap()
                .kind,
            AgentLogKind::UsageUpdated { .. }
        ));

        // PublishConnection finishes lifecycle reconnection, not the task turn.
        let claimed = store
            .claim_agent_work(
                &working.id,
                "reconnector",
                NOW.into(),
                "2026-09-05T00:20:00Z".into(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.work.step, AgentWorkStep::PublishConnection);
        let fence = claimed.work.lease.as_ref().unwrap().fence;
        let mut connected = claimed.clone();
        connected
            .work
            .advance("reconnector", fence, AgentWorkStep::None, NOW.into())
            .unwrap();
        connected.revision += 1;
        connected.conversation.last_seq += 1;
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "reconnector".into(),
                fence,
                agent: connected,
                worktree: None,
                appended_at: NOW.into(),
                sandbox_event: None,
                agent_logs: vec![AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::PublishConnection,
                }],
            })
            .await
            .unwrap();
        assert_eq!(
            store
                .get_agent(&working.id)
                .await
                .unwrap()
                .unwrap()
                .conversation
                .active_turn,
            working.conversation.active_turn
        );

        let replacement = DurableAgentUpdateRecorder::new(Arc::new(store.clone()));
        let mut fresh = AgentMessageAssembly::default();
        if final_text {
            cursor += 1;
            replacement
                .record_update(
                    &working.id,
                    1,
                    &mut fresh,
                    retained(cursor, Some(message_chunk("final", "Native final answer."))),
                    NOW.into(),
                )
                .await
                .unwrap();
        }
        cursor += 1;
        let terminal = if failed {
            ConversationUpdate::FailedForPrompt {
                prompt_id: layer3_adapter::PromptIdentity::new(
                    "conversation-1",
                    1,
                    working.conversation.active_turn.unwrap(),
                ),
                message: "Native failure.".into(),
                capability: Some("native_execution_failed".into()),
            }
        } else {
            identity_terminal(&working)
        };
        let started = std::time::Instant::now();
        if !terminal_first {
            recorder
                .record_update(&working.id, 1, &mut old, retained(cursor, None), NOW.into())
                .await
                .unwrap();
            assert_eq!(
                store
                    .get_agent(&working.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .conversation
                    .active_turn,
                working.conversation.active_turn
            );
        }
        replacement
            .record_update(
                &working.id,
                1,
                &mut fresh,
                retained(cursor, Some(terminal.clone())),
                NOW.into(),
            )
            .await
            .unwrap();
        if terminal_first {
            recorder
                .record_update(&working.id, 1, &mut old, retained(cursor, None), NOW.into())
                .await
                .unwrap();
        }
        replacement
            .record_update(
                &working.id,
                1,
                &mut fresh,
                retained(cursor, Some(terminal)),
                NOW.into(),
            )
            .await
            .unwrap();
        lags.push(started.elapsed().as_micros());
        let saved = store.get_agent(&working.id).await.unwrap().unwrap();
        assert_eq!(saved.status, AgentStatus::Ready);
        assert!(saved.conversation.active_turn.is_none());
        assert_eq!(
            saved.conversation.observations.usage,
            before.conversation.observations.usage
        );
        let logs = store.agent_log(&working.id, 0, 200).await.unwrap();
        assert_eq!(
            logs.iter()
                .filter(|e| matches!(
                    e.kind,
                    AgentLogKind::TurnCompleted { .. }
                        | AgentLogKind::TurnFailed { .. }
                        | AgentLogKind::TurnInterrupted { .. }
                ))
                .count(),
            1
        );
        assert_eq!(
            logs.iter()
                .filter(|e| matches!(e.kind, AgentLogKind::ToolCallStarted { .. }))
                .count(),
            2
        );
        assert_eq!(logs.iter().filter(|e| matches!(&e.kind, AgentLogKind::ToolCallUpdated {status,..} if status=="completed")).count(), 2);
        let messages: Vec<_> = logs
            .iter()
            .filter_map(|e| {
                if let AgentLogKind::AgentMessage { content } = &e.kind {
                    Some(content.as_str())
                } else {
                    None
                }
            })
            .collect();
        let expected = if final_text {
            vec!["I will read the unchanged fixture.", "Native final answer."]
        } else {
            vec!["I will read the unchanged fixture."]
        };
        assert_eq!(
            messages, expected,
            "seed {seed}: no answer may be invented from tool completion"
        );
        assert!(logs.iter().any(|e| if failed {matches!(&e.kind,AgentLogKind::TurnFailed {failure,..} if failure.class=="native_execution_failed")} else {matches!(e.kind,AgentLogKind::TurnCompleted {..})}));
    }
    lags.sort_unstable();
    assert!(lags[989] <= 5_000_000 && *lags.last().unwrap() <= 30_000_000);
    println!("completed-tools propagation N=1000 p99_us={} max_us={} (in-memory terminal-observation-to-publication; not native production latency)", lags[989], lags.last().unwrap());
}
