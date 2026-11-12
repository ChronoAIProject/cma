use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    sandbox::{
        agent_io::{
            AgentResponseReading, AgentResponseStatus, CommandReceiptView, CommandStatusView,
        },
        AgentEventStreamSnapshot, AgentLogEntry, AgentLogKind, AgentStatus, AgentVisibleLogEntry,
    },
};
use serde_json::json;

use super::super::{ResponseObject, ResponseStatus};
use super::projection::{events, V2ResponseObject, V2ResponseStatus};

const AT: &str = "2026-09-17T00:00:00Z";

fn history() -> Vec<AgentLogKind> {
    use AgentLogKind::*;
    vec![
        TurnStarted {
            model: None,
            turn_sequence: 1,
            origin_key: None,
        },
        OwnerMessage {
            content: "First question".into(),
        },
        AgentMessage {
            content: "First answer".into(),
        },
        TurnCompleted { turn_sequence: 1 },
        ConversationGap { frames: 99 }, // Idle loss does not belong to either turn.
        TurnStarted {
            model: None,
            turn_sequence: 6,
            origin_key: None,
        },
        OwnerMessage {
            content: "Second question".into(),
        },
        AgentMessage {
            content: "Stored before the gap.".into(),
        },
        ConversationGap { frames: 3 },
        ConversationGap { frames: 2 },
        AgentMessage {
            content: "Stored after the gap.".into(),
        },
        TurnCompleted { turn_sequence: 6 },
        TurnStarted {
            model: None,
            turn_sequence: 13,
            origin_key: None,
        },
        OwnerMessage {
            content: "Third question".into(),
        },
        AgentMessage {
            content: "Third answer".into(),
        },
        TurnCompleted { turn_sequence: 13 },
    ]
}

fn project(kinds: &[AgentLogKind], turn: u64, active: bool) -> (ResponseObject, V2ResponseObject) {
    let agent = crate::rest::agents_tests::seeded_agent(
        "agt_output_gap",
        if active {
            AgentStatus::Working
        } else {
            AgentStatus::Ready
        },
        kinds.len() as u64,
        active.then_some(turn),
    );
    let entries: Vec<_> = kinds
        .iter()
        .enumerate()
        .map(|(index, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: index as u64 + 1,
            appended_at: AT.into(),
            kind: kind.clone(),
        })
        .collect();
    let legacy = super::super::retrieve::project(
        &crate::agent_page::AgentPageOrigin::test(),
        &agent,
        &entries,
        turn,
    )
    .unwrap();
    let snapshot = AgentEventStreamSnapshot {
        id: agent.id.clone(),
        created_at: AT.into(),
        model: agent.profile.content.model.clone(),
        initial_model: agent.profile.content.model.clone(),
        runtime_generation: Some(1),
        active_turn: active.then_some(turn),
        head: kinds.len() as u64,
        history: entries
            .into_iter()
            .map(|entry| AgentVisibleLogEntry {
                deferred_failure_detail: None,
                seq: entry.seq,
                appended_at: entry.appended_at,
                kind: Some(entry.kind),
            })
            .collect(),
    };
    let reading = AgentResponseReading {
        deferred_failure_detail: None,
        command: CommandStatusView {
            receipt: CommandReceiptView {
                command_id: AgentCommandId(format!("cmd_gap_{turn}")),
                agent_id: agent.id,
                response_id: Some(AgentResponseId(format!("resp_gap_{turn}"))),
                admitted_at: AT.into(),
                agent_order: turn,
                event_cursor: turn - 1,
            },
            status: if active {
                AgentResponseStatus::InProgress
            } else {
                AgentResponseStatus::Completed
            },
            queue_deadline: AT.into(),
            turn_sequence: Some(turn),
            observed_cursor: None,
            delivery: None,
            failure: None,
        },
        metadata: Default::default(),
        nyxid_trace: None,
    };
    (
        legacy,
        super::projection::project(
            &crate::agent_page::AgentPageOrigin::test(),
            reading,
            &snapshot,
        )
        .unwrap(),
    )
}

#[test]
fn completed_output_gap_retains_messages_and_emits_incomplete_replay() {
    let (legacy, response) = project(&history(), 6, false);
    let details = json!({"reason":"conversation_output_gap","frames":5});
    assert_eq!(legacy.status, ResponseStatus::Incomplete);
    assert_eq!(response.status, V2ResponseStatus::Incomplete);
    assert_eq!(legacy.incomplete_details.as_ref(), Some(&details));
    assert_eq!(response.incomplete_details.as_ref(), Some(&details));
    assert_eq!(
        serde_json::to_value(&legacy.output).unwrap(),
        serde_json::to_value(&response.output).unwrap()
    );
    assert_eq!(
        response
            .output
            .iter()
            .map(|item| match item {
                super::V2OutputItem::Message(item) => item.content[0].text.as_str(),
                _ => panic!("fixture contains only messages"),
            })
            .collect::<Vec<_>>(),
        ["Stored before the gap.", "Stored after the gap."]
    );
    assert_eq!(response.completed_at, layer2_core::seconds_from_rfc3339(AT));
    let replay = events(&response, true);
    assert_eq!(replay.last().unwrap()["type"], "response.incomplete");
    assert_eq!(
        replay.last().unwrap()["response"]["incomplete_details"],
        details
    );
    assert!(replay
        .iter()
        .all(|event| event["type"] != "response.completed"));
    for (index, event) in replay.iter().enumerate() {
        assert_eq!(event["sequence_number"], index);
    }
}

#[test]
fn output_gap_does_not_end_active_response_or_change_its_replay_prefix() {
    let history = history();
    let (legacy, active) = project(&history[..11], 6, true);
    assert_eq!(legacy.status, ResponseStatus::InProgress);
    assert_eq!(active.status, V2ResponseStatus::InProgress);
    assert!(legacy.incomplete_details.is_none() && active.incomplete_details.is_none());
    assert!(active.completed_at.is_none());
    assert_eq!(active.output.len(), 2);
    let active_events = events(&active, true);
    assert!(active_events.iter().all(|event| !matches!(
        event["type"].as_str(),
        Some("response.completed" | "response.incomplete" | "response.failed")
    )));
    let (_, terminal) = project(&history, 6, false);
    assert_eq!(
        &events(&terminal, true)[..active_events.len()],
        active_events
    );
}

#[test]
fn output_gap_is_scoped_to_its_active_turn_without_tainting_adjacent_turns() {
    for turn in [1, 13] {
        let (legacy, response) = project(&history(), turn, false);
        assert_eq!(legacy.status, ResponseStatus::Completed);
        assert_eq!(response.status, V2ResponseStatus::Completed);
        assert!(legacy.incomplete_details.is_none() && response.incomplete_details.is_none());
        assert_eq!(response.output.len(), 1);
        assert_eq!(
            events(&response, true).last().unwrap()["type"],
            "response.completed"
        );
    }
}
