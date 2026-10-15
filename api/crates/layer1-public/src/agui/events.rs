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
            vec![
                make("TEXT_MESSAGE_START", json!({"messageId": id, "role": role})),
                make(
                    "TEXT_MESSAGE_CONTENT",
                    json!({"messageId": id, "delta": content}),
                ),
                make("TEXT_MESSAGE_END", json!({"messageId": id})),
            ]
        }
        AgentLogKind::TurnCompleted { .. } => vec![make(
            "RUN_FINISHED",
            json!({"outcome": {"type": "complete"}}),
        )],
        AgentLogKind::TurnInterrupted { .. } => vec![make(
            "RUN_FINISHED",
            json!({"outcome": {"type": "interrupt", "interrupts": []}}),
        )],
        AgentLogKind::TurnFailed { failure, .. } => vec![make(
            "RUN_ERROR",
            json!({"code": failure.class, "message": failure.detail}),
        )],
        AgentLogKind::ToolCallStarted {
            call_id,
            title,
            kind,
            ..
        } => vec![make(
            "TOOL_CALL_START",
            json!({"toolCallId": call_id, "toolCallName": title, "cma:toolKind": kind}),
        )],
        AgentLogKind::ToolCallUpdated {
            call_id, status, ..
        } => vec![make(
            "TOOL_CALL_END",
            json!({"toolCallId": call_id, "cma:status": status}),
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
        AgentLogKind::TitleSet { title, .. } => vec![delta("/title", json!(title))],
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
        let replay = serde_json::to_value(AgentEvent::from(entry)).unwrap();
        assert_eq!(
            replay["kind"]["metadata"],
            json!({
                "description":"Setup input", "description_source":"prompt_preview",
                "description_is_setup_preview":true
            })
        );
    }
}
