use super::super::{InputTokenDetails, OutputTokenDetails, ResponseError, ResponseUsage};
use layer2_core::sandbox::{
    agent_io::{AgentResponseReading, AgentResponseStatus, CommandFailureCode},
    AgentEventStreamSnapshot, AgentLogKind,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use utoipa::ToSchema;

#[path = "indexed_projection.rs"]
pub(in crate::responses) mod indexed;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum V2ResponseStatus {
    Queued,
    InProgress,
    Completed,
    Failed,
    Incomplete,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ResponseConversation {
    pub id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct V2ResponseObject {
    /// Browser page for the actual CMA Agent behind this response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callback_url: Option<String>,
    pub id: String,
    pub object: &'static str,
    pub created_at: u64,
    pub completed_at: Option<u64>,
    pub conversation: ResponseConversation,
    pub status: V2ResponseStatus,
    pub background: bool,
    pub error: Option<ResponseError>,
    pub incomplete_details: Option<Value>,
    pub instructions: Option<String>,
    pub max_output_tokens: Option<u64>,
    pub model: String,
    /// Preserve the legacy response.created model hint on deterministic replay.
    #[serde(skip)]
    #[schema(ignore)]
    pub(super) creation_model: String,
    pub output: Vec<super::V2OutputItem>,
    /// Internal deterministic log projection, absent from the public response.
    #[serde(skip)]
    #[schema(ignore)]
    pub(super) output_events: Vec<Value>,
    pub parallel_tool_calls: bool,
    pub previous_response_id: Option<String>,
    pub reasoning: Value,
    pub store: bool,
    pub temperature: Option<f64>,
    pub text: Value,
    pub tool_choice: &'static str,
    pub tools: Vec<Value>,
    pub top_p: Option<f64>,
    pub truncation: &'static str,
    pub usage: Option<ResponseUsage>,
    pub metadata: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<serde_json::Value>)]
    pub nyxid_trace: Option<layer2_core::sandbox::agent_io::NyxidTrace>,
}
impl V2ResponseObject {
    pub(super) fn terminal(&self) -> bool {
        matches!(
            self.status,
            V2ResponseStatus::Completed | V2ResponseStatus::Failed | V2ResponseStatus::Incomplete
        )
    }
}
pub(super) fn project(
    origin: &crate::agent_page::AgentPageOrigin,
    reading: AgentResponseReading,
    snapshot: &AgentEventStreamSnapshot,
) -> Option<V2ResponseObject> {
    let deferred_failure_detail = reading.deferred_failure_detail;
    let command = reading.command;
    if command.receipt.agent_id != snapshot.id {
        return None;
    }
    let mut response = V2ResponseObject {
        callback_url: origin.agent_url(snapshot.id.as_str()),
        id: command.receipt.response_id?.to_string(),
        object: "response",
        created_at: layer2_core::seconds_from_rfc3339(&command.receipt.admitted_at)?,
        completed_at: None,
        conversation: ResponseConversation {
            id: snapshot.id.to_string(),
        },
        status: match command.status {
            AgentResponseStatus::Queued => V2ResponseStatus::Queued,
            AgentResponseStatus::InProgress | AgentResponseStatus::WaitingAction => {
                V2ResponseStatus::InProgress
            }
            AgentResponseStatus::Completed => V2ResponseStatus::Completed,
            AgentResponseStatus::Failed => V2ResponseStatus::Failed,
            AgentResponseStatus::Cancelled => V2ResponseStatus::Incomplete,
        },
        background: false,
        error: command.failure.map(|code| ResponseError {
            code: failure_code(code).into(),
            message: code
                .budget_error()
                .map_or("The admitted Agent command could not complete.", |reason| {
                    reason.guidance()
                })
                .into(),
        }),
        incomplete_details: (command.status == AgentResponseStatus::Cancelled)
            .then(|| json!({"reason":"interrupted"})),
        instructions: None,
        max_output_tokens: None,
        // Before dispatch, retain the original creation-model hint and queued
        // GET/created-event equivalence. It is not evidence of execution.
        model: super::super::model::named_or_default(snapshot.initial_model.as_deref()),
        creation_model: super::super::model::named_or_default(snapshot.initial_model.as_deref()),
        output: vec![],
        output_events: vec![],
        parallel_tool_calls: false,
        previous_response_id: None,
        reasoning: json!({"effort":null,"summary":null}),
        store: true,
        temperature: None,
        text: json!({"format":{"type":"text"},"verbosity":"medium"}),
        tool_choice: "none",
        tools: vec![],
        top_p: None,
        truncation: "disabled",
        usage: None,
        metadata: reading.metadata,
        nyxid_trace: None,
    };
    let Some(turn) = command.turn_sequence else {
        return Some(response);
    };
    response.nyxid_trace = reading.nyxid_trace.filter(|trace| trace.validate().is_ok());
    response.model = super::super::model::for_turn(
        snapshot.initial_model.as_deref(),
        turn,
        snapshot
            .history
            .iter()
            .map(|entry| (entry.seq, entry.kind.as_ref())),
    )?;
    let start = snapshot.history.iter().position(|entry| entry.seq == turn && matches!(entry.kind, Some(AgentLogKind::TurnStarted { turn_sequence, .. }) if turn_sequence == turn))?;
    let mut output = super::output::OutputProjection::default();
    let mut lost_frames = 0_u64;
    let mut turn_active = true;
    let mut prewrite_failed = None;
    for entry in snapshot.history.iter().skip(start + 1) {
        if let Some(kind) = &entry.kind {
            output.observe(kind, &snapshot.id, turn);
        }
        match &entry.kind {
            Some(AgentLogKind::OperationDeliveryChanged {
                operation_sequence,
                state: layer2_core::sandbox::AgentOperationDelivery::PreWriteFailed,
                operation: layer2_core::sandbox::AgentOperationKind::SendMessage,
            }) if operation_sequence.checked_add(1) == Some(turn) => {
                prewrite_failed = Some((entry.seq, &entry.appended_at));
            }
            Some(AgentLogKind::TurnStarted { .. }) => break,
            Some(AgentLogKind::Usage {
                input_tokens,
                output_tokens,
                ..
            }) => {
                response.usage = Some(ResponseUsage {
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                    total_tokens: input_tokens.saturating_add(*output_tokens),
                    input_tokens_details: InputTokenDetails::default(),
                    output_tokens_details: OutputTokenDetails::default(),
                })
            }
            Some(AgentLogKind::UsageUpdated {
                turn_sequence: Some(observed_turn),
                turn_input_tokens,
                turn_output_tokens,
                ..
            }) if *observed_turn == turn => {
                response.usage = Some(ResponseUsage {
                    input_tokens: *turn_input_tokens,
                    output_tokens: *turn_output_tokens,
                    input_tokens_details: InputTokenDetails::default(),
                    output_tokens_details: OutputTokenDetails::default(),
                    total_tokens: turn_input_tokens.saturating_add(*turn_output_tokens),
                });
            }
            Some(AgentLogKind::ConversationGap { frames }) if turn_active => {
                lost_frames = lost_frames.saturating_add(*frames);
            }
            Some(AgentLogKind::TurnCompleted { turn_sequence }) if *turn_sequence == turn => {
                turn_active = false;
                response.status = V2ResponseStatus::Completed;
                response.completed_at = layer2_core::seconds_from_rfc3339(&entry.appended_at);
                response.error = None;
                response.incomplete_details = None;
            }
            Some(AgentLogKind::TurnFailed {
                turn_sequence,
                failure,
            }) if *turn_sequence == turn => {
                turn_active = false;
                if deferred_failure_detail.as_ref().is_some_and(|expected| {
                    entry.deferred_failure_detail.as_ref() == Some(expected)
                }) && prewrite_failed.is_some_and(|(seq, at)| {
                    seq.checked_add(1) == Some(entry.seq) && at == &entry.appended_at
                }) && !command.status.is_terminal()
                    && failure.class == layer2_core::sandbox::agent_io::SOURCE_DEFERRED_FAILURE
                {
                    response.status = V2ResponseStatus::Queued;
                    response.error = None;
                    continue;
                }
                response.status = V2ResponseStatus::Failed;
                response.error = Some(ResponseError {
                    code: failure.class.clone(),
                    message: failure.detail.clone(),
                });
            }
            Some(AgentLogKind::TurnInterrupted { turn_sequence, .. }) if *turn_sequence == turn => {
                turn_active = false;
                response.status = V2ResponseStatus::Incomplete;
                response.incomplete_details = Some(json!({"reason":"interrupted"}));
            }
            _ => {}
        }
    }
    response.output = output.items;
    response.output_events = output.events;
    if lost_frames > 0 && !turn_active {
        response.status = V2ResponseStatus::Incomplete;
        response.incomplete_details =
            Some(json!({"reason":"conversation_output_gap","frames":lost_frames}));
    }
    Some(response)
}
fn failure_code(code: CommandFailureCode) -> &'static str {
    match code {
        CommandFailureCode::OwnerBudgetExhausted => "owner_budget_exhausted",
        CommandFailureCode::TurnBudgetExhausted => "turn_budget_exhausted",
        CommandFailureCode::BudgetUnavailable => "budget_unavailable",
        CommandFailureCode::QueueTimeout => "agent_command_queue_timeout",
        CommandFailureCode::AuthorityRevoked => "agent_command_authority_revoked",
        CommandFailureCode::AuthorityUnavailable => "agent_command_authority_unavailable",
        CommandFailureCode::AgentUnavailable => "agent_unavailable",
        CommandFailureCode::TargetTurnChanged => "target_turn_changed",
        CommandFailureCode::RuntimeUnavailable => "runtime_unavailable",
        CommandFailureCode::RuntimeLost => "runtime_lost",
        CommandFailureCode::CredentialSetupRequired
        | CommandFailureCode::ConnectionsRequired
        | CommandFailureCode::ExecutionFailed => "agent_command_execution_failed",
    }
}

/// Deterministic events over durable messages. Polling, reconnect and HTTP aliases
/// cannot alter event numbering or introduce provider-private event shapes.
pub(super) fn events(response: &V2ResponseObject, bound: bool) -> Vec<Value> {
    let mut events = Vec::new();
    let mut push = |kind: &str, mut body: Value| {
        body["type"] = kind.into();
        body["sequence_number"] = events.len().into();
        events.push(body);
    };
    let mut opening = response.clone();
    opening.status = V2ResponseStatus::Queued;
    // This frame also exists before dispatch. Its replay bytes must not change
    // once a turn has selected its model or later settings have changed.
    opening.model.clone_from(&response.creation_model);
    opening.output.clear();
    opening.error = None;
    opening.incomplete_details = None;
    opening.completed_at = None;
    opening.usage = None;
    opening.nyxid_trace = None;
    push("response.created", json!({"response":opening}));
    if bound {
        opening.status = V2ResponseStatus::InProgress;
        opening.model.clone_from(&response.model);
        push("response.in_progress", json!({"response":opening}));
        if let Some(trace) = &response.nyxid_trace {
            push(
                "response.nyxid_trace",
                json!({"response_id":response.id,"nyxid_trace":trace}),
            );
        }
    }
    for event in &response.output_events {
        push(
            event["type"].as_str().expect("projected event type"),
            event.clone(),
        );
    }
    if response.terminal() {
        push(
            match response.status {
                V2ResponseStatus::Completed => "response.completed",
                V2ResponseStatus::Failed => "response.failed",
                _ => "response.incomplete",
            },
            json!({"response":response}),
        );
    }
    events
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    fn trace() -> layer2_core::sandbox::agent_io::NyxidTrace {
        layer2_core::sandbox::agent_io::NyxidTrace {
            version: 1,
            turn_id: "11111111-1111-4111-8111-111111111111".into(),
            trace_id: "11111111111141118111111111111111".into(),
            parent_span_id: "0123456789abcdef".into(),
            ingress_request_id: "22222222-2222-4222-8222-222222222222".into(),
        }
    }

    #[test]
    fn retained_prerequisite_response_errors_use_the_legacy_code() {
        for code in [
            CommandFailureCode::CredentialSetupRequired,
            CommandFailureCode::ConnectionsRequired,
        ] {
            assert_eq!(failure_code(code), "agent_command_execution_failed");
        }
    }

    #[test]
    fn bound_trace_is_one_ordered_event_and_is_absent_from_created() {
        let response = V2ResponseObject {
            callback_url: None,
            id: "resp_trace".into(),
            object: "response",
            created_at: 1,
            completed_at: None,
            conversation: ResponseConversation { id: "agt".into() },
            status: V2ResponseStatus::InProgress,
            background: false,
            error: None,
            incomplete_details: None,
            instructions: None,
            max_output_tokens: None,
            model: "model".into(),
            creation_model: "model".into(),
            output: vec![],
            output_events: vec![],
            parallel_tool_calls: false,
            previous_response_id: None,
            reasoning: json!({}),
            store: true,
            temperature: None,
            text: json!({}),
            tool_choice: "none",
            tools: vec![],
            top_p: None,
            truncation: "disabled",
            usage: None,
            metadata: BTreeMap::new(),
            nyxid_trace: Some(trace()),
        };
        let events = events(&response, true);
        assert_eq!(events[0]["type"], "response.created");
        assert!(events[0]["response"].get("nyxid_trace").is_none());
        assert_eq!(events[1]["type"], "response.in_progress");
        assert_eq!(events[2]["type"], "response.nyxid_trace");
        assert_eq!(events[2]["sequence_number"], 2);
        assert_eq!(events[2]["response_id"], "resp_trace");
        assert_eq!(events[2]["nyxid_trace"]["turn_id"], trace().turn_id);
    }
}
