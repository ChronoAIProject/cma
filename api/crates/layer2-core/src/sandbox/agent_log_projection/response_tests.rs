use super::*;

fn complete(agent: &mut Agent, state: &mut AgentLogProjectionState, turn: u64) {
    apply(
        state,
        agent,
        AgentLogKind::TurnCompleted {
            turn_sequence: turn,
        },
    );
    agent.conversation.active_turn = None;
}

fn legacy_usage(input: u64) -> AgentLogKind {
    AgentLogKind::Usage {
        input_tokens: input,
        output_tokens: 2,
        context_used: 3,
        context_window: None,
    }
}

#[test]
fn historical_model_provenance_freezes_at_each_turn() {
    let (mut agent, mut state) = opened();
    let first = start(&mut agent, &mut state, None);
    assert!(state.active.as_ref().unwrap().legacy_model_known);
    apply(
        &mut state,
        &mut agent,
        AgentLogKind::OperationDeliveryChanged {
            operation_sequence: 5,
            operation: AgentOperationKind::UpdateSettings,
            state: AgentOperationDelivery::PreWriteFailed,
        },
    );
    assert!(state.settings_changed);
    assert!(state.active.as_ref().unwrap().legacy_model_known);
    complete(&mut agent, &mut state, first);
    start(&mut agent, &mut state, None);
    assert!(!state.active.as_ref().unwrap().legacy_model_known);
    let second = state.latest_turn.unwrap();
    complete(&mut agent, &mut state, second);
    start(&mut agent, &mut state, Some("original-explicit"));
    let facts = state.active.as_ref().unwrap();
    assert!(!facts.legacy_model_known);
    assert_eq!(facts.model.as_deref(), Some("original-explicit"));
    state.validate_aggregate(&agent).unwrap();
}

#[test]
fn response_usage_extends_past_terminal_and_stops_at_next_turn() {
    let (mut agent, mut state) = opened();
    let first = start(&mut agent, &mut state, None);
    apply(&mut state, &mut agent, legacy_usage(7));
    complete(&mut agent, &mut state, first);
    let terminal = state.latest_completed.as_ref().unwrap().end_sequence;
    apply(&mut state, &mut agent, usage(first, 9));
    let changes = apply(&mut state, &mut agent, legacy_usage(11));
    let original = state.latest_completed.as_ref().unwrap().clone();
    assert_eq!(original.response_usage.unwrap().input_tokens, 11);
    assert_eq!(original.response_usage_sequence, Some(state.through_seq));
    assert_eq!(original.usage.unwrap().input_tokens, 9);
    assert_eq!(original.end_sequence, terminal);
    assert!(changes.iter().any(|change| matches!(change,
        AgentLogEvidenceChange::Turn(facts) if facts.response_usage == original.response_usage)));
    let next = start(&mut agent, &mut state, None);
    apply(&mut state, &mut agent, usage(first, 13));
    assert_eq!(
        state.latest_completed.as_ref().unwrap().response_usage,
        original.response_usage
    );
    assert!(state.active.as_ref().unwrap().response_usage.is_none());
    apply(&mut state, &mut agent, legacy_usage(17));
    let response = state.active.as_ref().unwrap().response_usage.unwrap();
    assert_eq!((response.turn_sequence, response.input_tokens), (next, 17));
    state.validate_aggregate(&agent).unwrap();
}

#[test]
fn deferred_failure_reference_requires_exact_adjacent_operation_and_time() {
    for (same_operation, adjacent, same_time) in [
        (true, true, true),
        (false, true, true),
        (true, false, true),
        (true, true, false),
    ] {
        let (mut agent, mut state) = opened();
        let turn = start(&mut agent, &mut state, None);
        apply(
            &mut state,
            &mut agent,
            AgentLogKind::OperationDeliveryChanged {
                operation_sequence: if same_operation { turn - 1 } else { turn },
                operation: AgentOperationKind::SendMessage,
                state: AgentOperationDelivery::PreWriteFailed,
            },
        );
        let prewrite = state.through_seq;
        if !adjacent {
            apply(&mut state, &mut agent, legacy_usage(3));
        }
        let terminal = AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: state.through_seq + 1,
            appended_at: if same_time {
                NOW
            } else {
                "2026-11-11T00:00:01Z"
            }
            .into(),
            kind: AgentLogKind::TurnFailed {
                turn_sequence: turn,
                failure: crate::sandbox::TypedFailure {
                    class: crate::sandbox::agent_io::SOURCE_DEFERRED_FAILURE.into(),
                    detail: "original-binding".into(),
                },
            },
        };
        // Round-trip the pending fact to model a publication/segment boundary.
        state = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        state.observe(&agent, &terminal, None).unwrap();
        agent.conversation.active_turn = None;
        agent.conversation.last_seq = terminal.seq;
        state.validate_aggregate(&agent).unwrap();
        let facts = state.latest_completed.as_ref().unwrap();
        assert_eq!(facts.end_sequence, Some(terminal.seq));
        assert_eq!(
            facts.prewrite_failure_sequence,
            (same_operation && adjacent && same_time).then_some(prewrite)
        );
    }
}

#[test]
fn missing_or_forged_response_facts_fail_closed() {
    let (mut agent, mut state) = opened();
    let turn = start(&mut agent, &mut state, None);
    apply(&mut state, &mut agent, legacy_usage(5));
    complete(&mut agent, &mut state, turn);
    let facts = state.latest_completed.as_ref().unwrap();
    for field in [
        "legacy_model_known",
        "response_usage",
        "response_usage_sequence",
        "prewrite_failure_sequence",
    ] {
        let mut encoded = serde_json::to_value(facts).unwrap();
        encoded.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<AgentLogTurnFacts>(encoded).is_err(),
            "{field}"
        );
    }
    let mut forged = facts.clone();
    forged.response_usage_sequence = Some(state.through_seq + 1);
    assert!(forged.validate(state.through_seq).is_err());
    forged = facts.clone();
    forged.response_usage.as_mut().unwrap().turn_sequence += 1;
    assert!(forged.validate(state.through_seq).is_err());
    forged = facts.clone();
    forged.prewrite_failure_sequence = forged.end_sequence.map(|seq| seq - 1);
    assert!(forged.validate(state.through_seq).is_err());
    for field in ["settings_changed", "pending_prewrite_failure"] {
        let mut encoded = serde_json::to_value(&state).unwrap();
        encoded.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<AgentLogProjectionState>(encoded).is_err(),
            "{field}"
        );
    }
}

#[test]
fn bootstrap_and_publication_produce_identical_response_evidence() {
    let (current, source) = opened();
    let kinds = vec![
        AgentLogKind::OperationDeliveryChanged {
            operation_sequence: 3,
            operation: AgentOperationKind::UpdateSettings,
            state: AgentOperationDelivery::DispatchFenced,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 4,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "input".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 4 },
        legacy_usage(19),
    ];
    let entries: Vec<_> = kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| AgentLogEntry {
            agent_id: current.id.clone(),
            seq: index as u64 + 3,
            appended_at: NOW.into(),
            kind,
        })
        .collect();
    let mut next = current.clone();
    next.conversation.last_seq = 7;
    next.revision += 1;
    let (published, changes) = source.plan_publication(&current, &next, &entries).unwrap();
    let mut bootstrap = BoundedAgentLogFold::default();
    for (seq, kind) in [
        (
            1,
            AgentLogKind::Created {
                workspace_id: next.workspace_id.clone(),
                worktree_id: next.worktree_id.clone(),
            },
        ),
        (
            2,
            AgentLogKind::ConversationOpened {
                conversation_id: "native-fixture".into(),
                runtime_generation: 1,
            },
        ),
    ] {
        bootstrap
            .observe(
                &next,
                &AgentLogEntry {
                    agent_id: next.id.clone(),
                    seq,
                    appended_at: NOW.into(),
                    kind,
                },
            )
            .unwrap();
    }
    let mut rebuilt_changes = Vec::new();
    for entry in &entries {
        rebuilt_changes.extend(bootstrap.observe(&next, entry).unwrap());
    }
    assert_eq!(bootstrap.finish(&next).unwrap(), published);
    assert_eq!(rebuilt_changes, changes);
    assert!(
        !published
            .latest_completed
            .as_ref()
            .unwrap()
            .legacy_model_known
    );
    assert_eq!(
        published
            .latest_completed
            .as_ref()
            .unwrap()
            .response_usage
            .unwrap()
            .input_tokens,
        19
    );
    // A refused publication must leave all original facts unchanged.
    let mut invalid = entries.clone();
    invalid.last_mut().unwrap().seq += 1;
    assert!(source.plan_publication(&current, &next, &invalid).is_err());
    assert_eq!(source.through_seq, 2);
    assert!(!source.settings_changed);
}
