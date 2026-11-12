use super::{
    projection::{events, project},
    V2ResponseObject,
};
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    sandbox::{
        agent_io::{
            AgentResponseReading, AgentResponseStatus, CommandReceiptView, CommandStatusView,
        },
        AgentEventStreamSnapshot, AgentLogKind, AgentVisibleLogEntry,
    },
    AgentId,
};
use serde_json::{json, Value};

fn response(kinds: &[AgentLogKind]) -> V2ResponseObject {
    let terminal = matches!(kinds.last(), Some(AgentLogKind::TurnCompleted { .. }));
    let at = "2026-09-22T00:00:00Z";
    let id = AgentId("agt_tools".into());
    let snapshot = AgentEventStreamSnapshot {
        id: id.clone(),
        created_at: at.into(),
        model: Some("test".into()),
        initial_model: Some("test".into()),
        runtime_generation: Some(1),
        active_turn: (!terminal).then_some(1),
        head: kinds.len() as u64,
        history: kinds
            .iter()
            .enumerate()
            .map(|(i, kind)| AgentVisibleLogEntry {
                deferred_failure_detail: None,
                seq: i as u64 + 1,
                appended_at: at.into(),
                kind: Some(kind.clone()),
            })
            .collect(),
    };
    let reading = AgentResponseReading {
        deferred_failure_detail: None,
        command: CommandStatusView {
            receipt: CommandReceiptView {
                command_id: AgentCommandId("cmd_tools".into()),
                response_id: Some(AgentResponseId("resp_tools".into())),
                agent_id: id,
                admitted_at: at.into(),
                agent_order: 1,
                event_cursor: 0,
            },
            status: if terminal {
                AgentResponseStatus::Completed
            } else {
                AgentResponseStatus::InProgress
            },
            queue_deadline: at.into(),
            turn_sequence: Some(1),
            observed_cursor: None,
            delivery: None,
            failure: None,
        },
        metadata: Default::default(),
        nyxid_trace: None,
    };
    project(
        &crate::agent_page::AgentPageOrigin::test(),
        reading,
        &snapshot,
    )
    .unwrap()
}
fn started(call: &str, title: &str) -> AgentLogKind {
    AgentLogKind::ToolCallStarted {
        call_id: call.into(),
        title: title.into(),
        kind: "execute".into(),
        status: "in_progress".into(),
    }
}
fn done(call: &str, status: &str) -> AgentLogKind {
    AgentLogKind::ToolCallUpdated {
        call_id: call.into(),
        status: status.into(),
        title: None,
        output: None,
    }
}
fn kinds(events: &[Value]) -> Vec<&str> {
    events.iter().map(|e| e["type"].as_str().unwrap()).collect()
}

#[test]
fn tool_completion_appends_after_text_without_renumbering_earlier_events() {
    let mut history = vec![
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 1,
            origin_key: None,
        },
        started("call-one", "Read source"),
    ];
    let initial = events(&response(&history), true);
    assert_eq!(
        kinds(&initial),
        [
            "response.created",
            "response.in_progress",
            "response.output_item.added",
            "response.mcp_call.in_progress"
        ]
    );
    assert_eq!(
        initial[2]["item"],
        json!({"type":"mcp_call","id":"mcp_call-one","server_label":"cma","name":"Read source","arguments":"","output":null,"error":null,"status":"in_progress"})
    );
    history.push(AgentLogKind::AgentMessage {
        content: "I found the file.".into(),
    });
    let text = events(&response(&history), true);
    assert_eq!(&text[..initial.len()], initial);
    history.push(done("call-one", "completed"));
    let completed = events(&response(&history), true);
    assert_eq!(&completed[..text.len()], text);
    assert_eq!(
        kinds(&completed[text.len()..]),
        ["response.mcp_call.completed", "response.output_item.done"]
    );
    assert_eq!(completed.last().unwrap()["item"]["status"], "completed");
    history.push(AgentLogKind::TurnCompleted { turn_sequence: 1 });
    let final_response = response(&history);
    let final_events = events(&final_response, true);
    assert_eq!(&final_events[..completed.len()], completed);
    assert_eq!(final_events.last().unwrap()["type"], "response.completed");
    for (i, event) in final_events.iter().enumerate() {
        assert_eq!(event["sequence_number"], i);
    }
    let wire = serde_json::to_value(final_response).unwrap();
    assert!(wire.get("output_events").is_none());
    assert_eq!(wire["output"][0]["status"], "completed");
    assert_eq!(wire["output"][1]["content"][0]["text"], "I found the file.");
    assert_eq!(
        wire["output"][1]["id"],
        super::super::retrieve::output_id(&AgentId("agt_tools".into()), 1, 0)
    );
}

#[test]
fn overlapping_tool_failure_has_bounded_names_empty_bodies_and_stable_indexes() {
    let history = [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 1,
            origin_key: None,
        },
        started("one", &"語".repeat(250)),
        started("two", "Check result"),
        done("two", "failed"),
        done("one", "completed"),
        done("two", "failed"),
        AgentLogKind::TurnCompleted { turn_sequence: 1 },
    ];
    let response = response(&history);
    let events = events(&response, true);
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "response.mcp_call.failed")
            .count(),
        1
    );
    let failed = events
        .iter()
        .find(|e| e["type"] == "response.mcp_call.failed")
        .unwrap();
    assert_eq!(failed["output_index"], 1);
    assert_eq!(failed["item_id"], "mcp_two");
    let wire = serde_json::to_value(response).unwrap();
    assert_eq!(
        wire["output"][0]["name"].as_str().unwrap().chars().count(),
        200
    );
    for item in wire["output"].as_array().unwrap() {
        assert_eq!(item["arguments"], "");
        assert_eq!(item["output"], Value::Null);
        assert_eq!(item["error"], Value::Null);
    }
}
