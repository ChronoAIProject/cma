use super::*;

fn window(full: &AgentEventStreamSnapshot, start: usize, end: usize) -> AgentEventStreamSnapshot {
    AgentEventStreamSnapshot {
        id: full.id.clone(),
        created_at: full.created_at.clone(),
        model: full.model.clone(),
        initial_model: full.initial_model.clone(),
        runtime_generation: full.runtime_generation,
        active_turn: full.active_turn,
        head: full.head,
        history: full.history[start..end].to_vec(),
    }
}

#[test]
fn proven_target_turn_preserves_response_objects_and_all_durable_replay_numbers() {
    let full = snapshot(vec![
        start(1, Some("old-model")),
        AgentLogKind::AgentMessage {
            content: "Older answer".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 1 },
        settings_edit(),
        start(5, Some("execution-model")),
        AgentLogKind::OwnerMessage {
            content: "Current question".into(),
        },
        AgentLogKind::AgentMessage {
            content: "First answer part".into(),
        },
        AgentLogKind::ConversationGap { frames: 2 },
        AgentLogKind::Usage {
            input_tokens: 10,
            output_tokens: 20,
            context_used: 30,
            context_window: None,
        },
        AgentLogKind::AgentMessage {
            content: "Second answer part".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 5 },
        start(12, Some("later-model")),
        AgentLogKind::AgentMessage {
            content: "Later answer must not leak".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 12 },
    ]);
    for status in [
        AgentResponseStatus::InProgress,
        AgentResponseStatus::Completed,
        AgentResponseStatus::Failed,
    ] {
        let read = || {
            let mut result = reading(Some(5));
            result.command.status = status;
            result
                .metadata
                .insert("request-label".into(), "retained".into());
            result
        };
        let baseline = project(read(), &full).unwrap();
        let bounded = project(read(), &window(&full, 4, 12)).unwrap();
        assert_eq!(bounded, baseline);
        assert_eq!(events(&bounded, true), events(&baseline, true));
        assert_eq!(bounded.model, "execution-model");
        assert!(!serde_json::to_string(&bounded)
            .unwrap()
            .contains("Later answer"));
    }
}

#[test]
fn unbound_response_projection_does_not_need_any_history_rows() {
    let full = snapshot(vec![
        start(1, Some("executed-model")),
        AgentLogKind::TurnCompleted { turn_sequence: 1 },
    ]);
    let baseline = project(reading(None), &full).unwrap();
    let bounded = project(reading(None), &window(&full, 0, 0)).unwrap();
    assert_eq!(bounded, baseline);
    assert_eq!(events(&bounded, false), events(&baseline, false));
    assert_eq!(bounded.model, "creation-model");
}

#[test]
fn a_legacy_turn_without_retained_model_requires_the_preceding_settings_history() {
    let full = snapshot(vec![
        settings_edit(),
        start(2, None),
        AgentLogKind::TurnCompleted { turn_sequence: 2 },
    ]);
    let baseline = project(reading(Some(2)), &full).unwrap();
    let unsafe_partial = project(reading(Some(2)), &window(&full, 1, 3)).unwrap();
    assert_eq!(baseline.model, "unknown");
    assert_ne!(
        unsafe_partial.model, baseline.model,
        "the store must request full fallback for this case"
    );
}
