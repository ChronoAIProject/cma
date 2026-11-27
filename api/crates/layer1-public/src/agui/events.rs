use axum::response::sse::Event;
use layer2_core::sandbox::{AgentLogEntry, AgentLogKind};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct AguiEvent {
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "runId")]
    pub run_id: String,
    #[serde(flatten)]
    pub body: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip)]
    pub seq: Option<u64>,
    #[serde(rename = "cma:live", skip_serializing_if = "is_false")]
    pub live: bool,
}

impl AguiEvent {
    pub(super) fn sse(&self) -> Event {
        let frame = Event::default()
            .json_data(self)
            .expect("AG-UI values are serializable");
        match self.seq {
            Some(seq) => frame.id(seq.to_string()),
            None => frame,
        }
    }
}

pub(super) fn project(entry: &AgentLogEntry, turn: u64) -> Vec<AguiEvent> {
    project_with_chunks(entry, turn, false)
}

pub(super) fn project_bounded(entry: &AgentLogEntry, turn: u64) -> Vec<AguiEvent> {
    project_with_chunks(entry, turn, true)
}

fn project_with_chunks(entry: &AgentLogEntry, turn: u64, bounded: bool) -> Vec<AguiEvent> {
    let make = |kind, body| AguiEvent {
        kind,
        thread_id: entry.agent_id.to_string(),
        run_id: turn.to_string(),
        body,
        timestamp: Some(entry.appended_at.clone()),
        seq: None,
        live: false,
    };
    let delta = |path: &str, value: Value| {
        make(
            "STATE_DELTA",
            json!({"delta": [
                {"op": "replace", "path": path, "value": value}
            ]}),
        )
    };
    let mut frames = match &entry.kind {
        AgentLogKind::TurnStarted { .. } => vec![make("RUN_STARTED", json!({}))],
        AgentLogKind::OwnerMessage { content } | AgentLogKind::AgentMessage { content } => {
            let role = if matches!(entry.kind, AgentLogKind::OwnerMessage { .. }) {
                "user"
            } else {
                "assistant"
            };
            let id = format!("msg_{}", entry.seq);
            let mut frames = vec![make(
                "TEXT_MESSAGE_START",
                json!({"messageId": id, "role": role}),
            )];
            let mut remaining = content.as_str();
            loop {
                // Even six-byte JSON escaping stays below the client frame
                // ceiling. Split only on UTF-8 boundaries and acknowledge the
                // original durable event once, on its final END frame.
                let mut end = if bounded {
                    remaining.len().min(8 * 1024)
                } else {
                    remaining.len()
                };
                while !remaining.is_char_boundary(end) {
                    end -= 1;
                }
                frames.push(make(
                    "TEXT_MESSAGE_CONTENT",
                    json!({"messageId": id, "delta": &remaining[..end]}),
                ));
                remaining = &remaining[end..];
                if remaining.is_empty() {
                    break;
                }
            }
            frames.push(make("TEXT_MESSAGE_END", json!({"messageId": id})));
            frames
        }
        AgentLogKind::TurnCompleted { .. } => vec![make(
            "RUN_FINISHED",
            json!({"outcome": {"type": "complete"}}),
        )],
        AgentLogKind::TurnInterrupted { .. } => vec![make(
            "RUN_FINISHED",
            json!({"outcome": {"type": "interrupt", "interrupts": []}}),
        )],
        AgentLogKind::TurnFailed { failure, .. } => {
            let retained_output =
                layer2_core::sandbox::AgentRetainedOutputAvailability::from_failure(failure);
            let failure = layer2_core::sandbox::public_agent_turn_failure(failure);
            let mut body = json!({"code": failure.class, "message": failure.detail});
            if let Some(availability) = retained_output {
                body["cma:retainedOutput"] = json!(availability);
            }
            vec![make("RUN_ERROR", body)]
        }
        AgentLogKind::ToolCallStarted {
            call_id,
            title,
            kind,
            status,
        } => {
            let mut body =
                json!({"toolCallId": call_id, "toolCallName": title, "cma:toolKind": kind});
            if matches!(status.as_str(), "completed" | "failed") {
                body["cma:status"] = json!(status);
            }
            vec![make("TOOL_CALL_START", body)]
        }
        AgentLogKind::ToolCallUpdated {
            call_id,
            status,
            output,
            ..
        } => vec![make(
            "TOOL_CALL_END",
            json!({"toolCallId": call_id, "cma:status": status, "cma:output": output}),
        )],
        AgentLogKind::ToolOutputReported { call_id, output } => vec![make(
            "CUSTOM",
            json!({"name": "cma.tool_output_reported", "value": {"toolCallId": call_id, "output": output}}),
        )],
        AgentLogKind::ApprovalRequested {
            approval_id,
            action,
            detail,
            ..
        } => {
            let reason = action
                .as_ref()
                .map(|action| format!("cma:{action}"))
                .unwrap_or_else(|| "confirmation".into());
            let value = json!({"id": approval_id, "reason": reason, "message": detail.as_ref().or(action.as_ref())});
            vec![
                make(
                    "RUN_FINISHED",
                    json!({"outcome": {"type": "interrupt", "interrupts": [value]}}),
                ),
                make(
                    "STATE_DELTA",
                    json!({"delta": [{"op": "add", "path": approval_path(approval_id), "value": value}]}),
                ),
            ]
        }
        AgentLogKind::ApprovalResolved { approval_id, .. } => vec![make(
            "STATE_DELTA",
            json!({"delta": [{"op": "remove", "path": approval_path(approval_id)}]}),
        )],
        AgentLogKind::Usage {
            input_tokens,
            output_tokens,
            context_used,
            context_window,
        } => vec![make(
            "STATE_DELTA",
            json!({"delta": [
                {"op": "replace", "path": "/usage/inputTokens", "value": input_tokens},
                {"op": "replace", "path": "/usage/outputTokens", "value": output_tokens},
                {"op": "replace", "path": "/usage/contextUsed", "value": context_used},
                {"op": "replace", "path": "/usage/contextWindow", "value": context_window}
            ]}),
        )],
        AgentLogKind::RuntimeMetadataChanged { metadata } => vec![delta(
            "/runtimeMetadata",
            json!({"workingDirectory": metadata.working_directory, "description": metadata.description, "descriptionSource": metadata.description_source, "descriptionIsSetupPreview": metadata.description_is_setup_preview}),
        )],
        AgentLogKind::UsageUpdated {
            input_tokens,
            output_tokens,
            context_used,
            context_window,
            turn_sequence,
            turn_input_tokens,
            turn_output_tokens,
        } => vec![delta(
            "/usage",
            json!({
                "inputTokens": input_tokens, "outputTokens": output_tokens, "contextUsed": context_used, "contextWindow": context_window,
                "currentTurn": turn_sequence.map(|turn_sequence| json!({"turnSequence": turn_sequence, "inputTokens": turn_input_tokens, "outputTokens": turn_output_tokens}))
            }),
        )],
        AgentLogKind::ModeChanged { mode } => vec![delta("/mode", json!(mode))],
        AgentLogKind::PlanSet { entries } => vec![delta("/plan", json!(entries))],
        AgentLogKind::ConfigOptionsSet { options } => vec![delta("/configOptions", json!(options))],
        AgentLogKind::AvailableCommandsSet { commands } => {
            vec![delta("/availableCommands", json!(commands))]
        }
        AgentLogKind::ConversationGap { frames } => {
            vec![delta("/conversationGap", json!({"frames": frames}))]
        }
        AgentLogKind::SignInRequired { url, code, message } => vec![delta(
            "/signIn",
            json!({"url": url, "code": code, "message": message}),
        )],
        AgentLogKind::ConversationOpened { .. } => vec![delta("/signIn", Value::Null)],
        AgentLogKind::StatusChanged { to, .. } => vec![delta("/status", json!(to))],
        AgentLogKind::FirstMessageChanged {
            state,
            recovery_generation,
        } => vec![delta(
            "/firstMessage",
            json!({"state": state, "recoveryGeneration": recovery_generation}),
        )],
        AgentLogKind::TitleSet { title, source } => vec![delta(
            if source == "owner_override" {
                "/ownerTitle"
            } else {
                "/title"
            },
            json!(title),
        )],
        AgentLogKind::Stopped { reason } => vec![make(
            "RUN_ERROR",
            json!({"code": reason, "message": "The Agent stopped."}),
        )],
        AgentLogKind::Deleted => vec![make(
            "RUN_ERROR",
            json!({"code": "agent_deleted", "message": "The Agent was deleted."}),
        )],
        _ => Vec::new(),
    };
    if let Some(last) = frames.last_mut() {
        last.seq = Some(entry.seq);
    }
    frames
}

pub(super) fn ending(kind: &AgentLogKind) -> bool {
    matches!(
        kind,
        AgentLogKind::TurnCompleted { .. }
            | AgentLogKind::TurnFailed { .. }
            | AgentLogKind::TurnInterrupted { .. }
            | AgentLogKind::ApprovalRequested { .. }
            | AgentLogKind::Stopped { .. }
            | AgentLogKind::Deleted
    )
}

fn approval_path(id: &str) -> String {
    format!(
        "/pendingApprovals/{}",
        id.replace('~', "~0").replace('/', "~1")
    )
}

fn is_false(value: &bool) -> bool {
    !value
}

#[cfg(test)]
mod observation_tests {
    use super::*;
    use layer2_core::sandbox::{
        agent_io::AgentEvent, AgentDescriptionSource, AgentRuntimeMetadata,
    };

    #[test]
    fn observation_agui_and_v2_preserve_setup_provenance_atomically() {
        let entry = AgentLogEntry {
            agent_id: layer2_core::AgentId("agt_observation".into()),
            seq: 12,
            appended_at: "2026-10-15T00:00:00Z".into(),
            kind: AgentLogKind::RuntimeMetadataChanged {
                metadata: AgentRuntimeMetadata {
                    description: Some("Setup input".into()),
                    description_source: Some(AgentDescriptionSource::PromptPreview),
                    description_is_setup_preview: true,
                    ..Default::default()
                },
            },
        };
        let frame = &project(&entry, 1)[0];
        assert_eq!(frame.body["delta"][0]["path"], "/runtimeMetadata");
        assert_eq!(
            frame.body["delta"][0]["value"],
            json!({
                "workingDirectory": null, "description": "Setup input",
                "descriptionSource": "prompt_preview", "descriptionIsSetupPreview": true
            })
        );
        let replay = serde_json::to_value(AgentEvent::try_from(entry).unwrap()).unwrap();
        assert_eq!(
            replay["kind"]["metadata"],
            json!({
                "description":"Setup input", "description_source":"prompt_preview",
                "description_is_setup_preview":true
            })
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_start_preserves_terminal_status_and_the_nonterminal_wire_shape() {
        for (status, terminal) in [
            ("pending", false),
            ("in_progress", false),
            ("completed", true),
            ("failed", true),
        ] {
            let entry: AgentLogEntry = serde_json::from_value(json!({
                "agent_id":"agt_fixture", "seq":1, "appended_at":"2026-11-05T14:33:00Z",
                "kind":{"type":"tool_call_started","call_id":"delivery",
                    "title":"mcp.cma_access.describe_deliverable","kind":"execute","status":status}
            }))
            .unwrap();
            let frames = project(&entry, 1);
            assert_eq!(frames.len(), 1);
            assert_eq!(frames[0].kind, "TOOL_CALL_START");
            let mut expected = json!({"toolCallId":"delivery",
                "toolCallName":"mcp.cma_access.describe_deliverable","cma:toolKind":"execute"});
            if terminal {
                expected["cma:status"] = json!(status);
            }
            assert_eq!(frames[0].body, expected, "{status}");
        }
    }
}
