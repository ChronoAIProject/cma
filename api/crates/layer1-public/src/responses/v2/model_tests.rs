use super::projection::{events, project};
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    sandbox::{
        agent_io::{
            AgentResponseReading, AgentResponseStatus, CommandReceiptView, CommandStatusView,
        },
        AgentEventStreamSnapshot, AgentLogEntry, AgentLogKind, AgentOperationDelivery,
        AgentOperationKind, AgentSettings, AgentStatus, AgentVisibleLogEntry,
    },
    AgentId,
};

const AT: &str = "2026-10-15T00:00:00Z";

fn start(turn_sequence: u64, model: Option<&str>) -> AgentLogKind {
    AgentLogKind::TurnStarted {
        turn_sequence,
        model: model.map(str::to_owned),
        origin_key: None,
    }
}

fn settings_edit() -> AgentLogKind {
    AgentLogKind::OperationDeliveryChanged {
        operation_sequence: 3,
        operation: AgentOperationKind::UpdateSettings,
        state: AgentOperationDelivery::ExecutionObserved,
    }
}

fn snapshot(kinds: Vec<AgentLogKind>) -> AgentEventStreamSnapshot {
    AgentEventStreamSnapshot {
        id: AgentId("agt_models".into()),
        created_at: AT.into(),
        model: Some("today-model".into()),
        initial_model: Some("creation-model".into()),
        runtime_generation: Some(1),
        active_turn: None,
        head: kinds.len() as u64,
        history: kinds
            .into_iter()
            .enumerate()
            .map(|(index, kind)| AgentVisibleLogEntry {
                seq: index as u64 + 1,
                appended_at: AT.into(),
                kind: Some(kind),
            })
            .collect(),
    }
}

fn reading(turn: Option<u64>) -> AgentResponseReading {
    AgentResponseReading {
        command: CommandStatusView {
            receipt: CommandReceiptView {
                command_id: AgentCommandId("cmd_models".into()),
                response_id: Some(AgentResponseId("resp_models".into())),
                agent_id: AgentId("agt_models".into()),
                admitted_at: AT.into(),
                agent_order: 1,
                event_cursor: 0,
            },
            status: if turn.is_some() {
                AgentResponseStatus::Completed
            } else {
                AgentResponseStatus::Queued
            },
            queue_deadline: AT.into(),
            turn_sequence: turn,
            observed_cursor: None,
            delivery: None,
            failure: None,
        },
        metadata: Default::default(),
    }
}

#[test]
fn changing_current_settings_never_relabels_completed_response_models_or_replay() {
    let mut history = snapshot(vec![
        start(1, Some("creation-model")),
        AgentLogKind::TurnCompleted { turn_sequence: 1 },
        settings_edit(),
        start(4, Some("selected-alternate")),
        AgentLogKind::TurnCompleted { turn_sequence: 4 },
    ]);
    let original = project(reading(Some(1)), &history).unwrap();
    let alternate = project(reading(Some(4)), &history).unwrap();
    assert_eq!(original.model, "creation-model");
    assert_eq!(alternate.model, "selected-alternate");
    history.model = Some("another-selection".into());
    assert_eq!(project(reading(Some(1)), &history).unwrap(), original);
    assert_eq!(project(reading(Some(4)), &history).unwrap(), alternate);
    assert_eq!(
        events(&alternate, true)[1]["response"]["model"],
        "selected-alternate"
    );

    let mut agent =
        crate::rest::agents_tests::seeded_agent("agt_models", AgentStatus::Ready, 5, None);
    agent.profile.content.model = history.initial_model.clone();
    agent.conversation.settings = Some(AgentSettings {
        model: "another-selection".into(),
        ..agent.settings()
    });
    let entries: Vec<_> = history
        .history
        .iter()
        .map(|entry| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: entry.seq,
            appended_at: entry.appended_at.clone(),
            kind: entry.kind.clone().unwrap(),
        })
        .collect();
    assert_eq!(
        super::super::retrieve::project(&agent, &entries, 1)
            .unwrap()
            .model,
        "creation-model"
    );
    assert_eq!(
        super::super::retrieve::project(&agent, &entries, 4)
            .unwrap()
            .model,
        "selected-alternate"
    );
}

#[test]
fn legacy_turn_after_settings_edit_is_unknown_instead_of_inventing_historical_model() {
    let history = snapshot(vec![
        start(1, None),
        AgentLogKind::TurnCompleted { turn_sequence: 1 },
        settings_edit(),
        start(4, None),
        AgentLogKind::TurnCompleted { turn_sequence: 4 },
    ]);
    assert_eq!(
        project(reading(Some(1)), &history).unwrap().model,
        "creation-model"
    );
    assert_eq!(
        project(reading(Some(4)), &history).unwrap().model,
        "unknown"
    );
    // Old serialized turns remain readable; missing evidence remains missing.
    let old: AgentLogKind =
        serde_json::from_str(r#"{"type":"turn_started","turn_sequence":4}"#).unwrap();
    assert_eq!(old, start(4, None));
}

#[test]
fn queued_created_event_remains_identical_after_dispatch_and_model_changes() {
    let mut history = snapshot(vec![]);
    let queued = project(reading(None), &history).unwrap();
    let created = events(&queued, false);
    assert_eq!(queued.model, "creation-model");
    assert_eq!(created[0]["response"]["model"], "creation-model");
    history = snapshot(vec![
        start(1, Some("selected-alternate")),
        AgentLogKind::TurnCompleted { turn_sequence: 1 },
    ]);
    let completed = project(reading(Some(1)), &history).unwrap();
    assert_eq!(events(&completed, true)[..1], created);
    assert_eq!(completed.model, "selected-alternate");
}

#[test]
fn explicit_runtime_default_is_not_confused_with_missing_legacy_evidence() {
    let history = snapshot(vec![
        settings_edit(),
        start(2, Some("")),
        AgentLogKind::TurnCompleted { turn_sequence: 2 },
    ]);
    assert_eq!(
        project(reading(Some(2)), &history).unwrap().model,
        "deployment-default"
    );
}
