use super::*;

fn fixture() -> Agent {
    agent(
        "agt_projection",
        "wtr_projection",
        "wks_projection",
        "sbx_projection",
        "owner",
        AgentStatus::Ready,
    )
}

fn entry(agent: &Agent, seq: u64, kind: AgentLogKind) -> AgentLogEntry {
    AgentLogEntry {
        agent_id: agent.id.clone(),
        seq,
        appended_at: NOW.into(),
        kind,
    }
}

fn prefix(agent: &Agent) -> Vec<AgentLogEntry> {
    [
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "native-fixture".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "fixture input".into(),
        },
    ]
    .into_iter()
    .enumerate()
    .map(|(i, kind)| entry(agent, i as u64 + 1, kind))
    .collect()
}

#[test]
fn incremental_projection_matches_legacy_fold_across_mid_turn_boundaries() {
    let mut agent = fixture();
    let mut entries = prefix(&agent);
    for kind in [
        AgentLogKind::AgentMessage {
            content: "earlier output".into(),
        },
        AgentLogKind::Usage {
            input_tokens: 100,
            output_tokens: 10,
            context_used: 50,
            context_window: Some(1000),
        },
        AgentLogKind::OperationDeliveryChanged {
            operation_sequence: 7,
            operation: AgentOperationKind::Steer,
            state: AgentOperationDelivery::DispatchFenced,
        },
        AgentLogKind::OwnerMessage {
            content: "fixture steering".into(),
        },
        AgentLogKind::AgentMessage {
            content: "fixture answer".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
        AgentLogKind::ConversationOpened {
            conversation_id: "native-fixture".into(),
            runtime_generation: 2,
        },
        AgentLogKind::Usage {
            input_tokens: 15,
            output_tokens: 3,
            context_used: 10,
            context_window: Some(1000),
        },
    ] {
        entries.push(entry(&agent, entries.len() as u64 + 1, kind));
    }
    agent.conversation.conversation_id = Some("native-fixture".into());
    agent.conversation.runtime_generation = Some(2);
    agent.conversation.last_seq = entries.len() as u64;
    let legacy = crate::sandbox::agent_reads_fold::fold_agent_log(&agent, &entries).unwrap();
    let mut state = AgentLogProjectionState::default();
    let mut last_turn = None;
    for entry in entries {
        for change in state.observe(&agent, &entry, None).unwrap() {
            if let AgentLogEvidenceChange::Turn(turn) = change {
                last_turn = Some(turn);
            }
        }
        // Simulate durable reloads at count and byte-triggered boundaries,
        // including immediately after the steering fence.
        state = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    }
    state.validate_aggregate(&agent).unwrap();
    assert_eq!(state.usage(), legacy.usage.usage);
    assert_eq!(
        last_turn.as_ref().unwrap().outcome,
        legacy.transcript.turns[0].outcome
    );
    assert_eq!(last_turn.unwrap().final_answer_sequence(), Some(9));
}

#[test]
fn a_turn_crossing_many_segments_does_not_grow_the_scalar_projection() {
    let agent = fixture();
    let mut state = AgentLogProjectionState::default();
    for entry in prefix(&agent) {
        state.observe(&agent, &entry, None).unwrap();
    }
    let content = "x".repeat(16 * 1024);
    for seq in 5..=30_005 {
        let changes = state
            .observe(
                &agent,
                &entry(
                    &agent,
                    seq,
                    AgentLogKind::AgentMessage {
                        content: content.clone(),
                    },
                ),
                None,
            )
            .unwrap();
        assert_eq!(changes.len(), 2);
    }
    assert!(serde_json::to_vec(&state).unwrap().len() < 2048);
    assert_eq!(state.active.unwrap().last_message.unwrap().0, 30_005);
}

#[test]
fn projection_refuses_gaps_duplicate_creation_and_unpaired_steering() {
    let agent = fixture();
    let mut state = AgentLogProjectionState::default();
    for entry in prefix(&agent) {
        state.observe(&agent, &entry, None).unwrap();
    }
    assert!(state
        .clone()
        .observe(
            &agent,
            &entry(
                &agent,
                6,
                AgentLogKind::ModeChanged {
                    mode: "fixture".into()
                }
            ),
            None
        )
        .is_err());
    let creation = prefix(&agent).remove(0).kind;
    assert!(state
        .clone()
        .observe(&agent, &entry(&agent, 5, creation), None)
        .is_err());
    state
        .observe(
            &agent,
            &entry(
                &agent,
                5,
                AgentLogKind::OperationDeliveryChanged {
                    operation_sequence: 5,
                    operation: AgentOperationKind::Steer,
                    state: AgentOperationDelivery::DispatchFenced,
                },
            ),
            None,
        )
        .unwrap();
    assert!(state
        .observe(
            &agent,
            &entry(
                &agent,
                6,
                AgentLogKind::AgentMessage {
                    content: "unexpected".into()
                }
            ),
            None
        )
        .is_err());
}

#[tokio::test]
async fn legacy_usage_and_event_pages_remain_available_above_complete_transcript_budget() {
    let store = Arc::new(InMemorySandboxStore::default());
    let agent = insert_hierarchy(
        &store,
        "agt_large_read",
        "wtr_large_read",
        "wks_large_read",
        "sbx_large_read",
        "owner",
        AgentStatus::Ready,
    )
    .await;
    let mut kinds = vec![
        AgentLogKind::ConversationOpened {
            conversation_id: "native-fixture".into(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "fixture input".into(),
        },
    ];
    kinds.extend((0..20).map(|_| AgentLogKind::AgentMessage {
        content: "x".repeat(256 * 1024),
    }));
    kinds.extend([
        AgentLogKind::Usage {
            input_tokens: 123,
            output_tokens: 45,
            context_used: 100,
            context_window: Some(1000),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ]);
    let agent = replace_history(&store, agent, kinds, Some("native-fixture"), Some(1), None).await;
    let reads = service(store);
    assert!(matches!(
        reads.transcript(&caller("owner", true), &agent.id).await,
        Err(AgentReadError::PaginationRequired)
    ));
    let usage = reads
        .usage(&caller("owner", true), &agent.id)
        .await
        .unwrap();
    assert_eq!(usage.usage.input_tokens, 123);
    assert_eq!(usage.usage.output_tokens, 45);
    assert_eq!(usage.through_seq, agent.conversation.last_seq);
    let mut cursor = 0;
    while cursor < agent.conversation.last_seq {
        let page = reads
            .log(&caller("owner", true), &agent.id, cursor, 4)
            .await
            .unwrap();
        assert_eq!(page[0].seq, cursor + 1);
        assert!(page.len() <= 4);
        cursor = page.last().unwrap().seq;
    }
}

#[test]
fn bounded_bootstrap_refuses_unproven_queued_input_and_carries_content_identity() {
    let mut agent = fixture();
    let mut fold = BoundedAgentLogFold::default();
    let mut source = prefix(&agent);
    // An origin reference cannot manufacture a queued owner message.
    source[2].kind = AgentLogKind::TurnStarted {
        turn_sequence: 3,
        origin_key: Some("queued-message:2".into()),
        model: None,
    };
    for row in &source[..2] {
        fold.observe(&agent, row).unwrap();
    }
    assert!(fold.observe(&agent, &source[2]).is_err());

    let mut fold = BoundedAgentLogFold::default();
    let mut source = prefix(&agent)[..2].to_vec();
    source.push(entry(
        &agent,
        3,
        AgentLogKind::OwnerMessage {
            content: "original input".into(),
        },
    ));
    for row in &source {
        fold.observe(&agent, row).unwrap();
    }
    agent.conversation.last_seq = 3;
    agent.conversation.conversation_id = Some("native-fixture".into());
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.queued_messages =
        vec![QueuedAgentMessage::new(3, "changed input".into(), NOW.into()).unwrap()];
    assert!(fold.finish(&agent).is_err());
}
