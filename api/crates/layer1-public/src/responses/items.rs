use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    sandbox::{AgentLogEntry, AgentLogKind},
    AgentId,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::{conversation::read_failure, AgentConversationState, AuthenticatedCaller};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ItemsQuery {
    after: Option<String>,
    limit: Option<usize>,
}

#[derive(Serialize, ToSchema)]
pub(super) struct ConversationItemList {
    object: &'static str,
    data: Vec<Value>,
    first_id: Option<String>,
    last_id: Option<String>,
    has_more: bool,
}

#[utoipa::path(get, path = "/api/v1/conversations/{id}/items", tag = "responses",
    params(("id" = String, Path), ("after" = Option<String>, Query), ("limit" = Option<usize>, Query)),
    responses((status = 200, body = ConversationItemList)))]
pub(super) async fn list(
    State(state): State<AgentConversationState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    query: Result<Query<ItemsQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(query) => query,
        Err(_) => return super::field_refusal("query", "The items query is invalid."),
    };
    let limit = query.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return super::field_refusal("limit", "limit must be between 1 and 100.");
    }
    let id = AgentId(id);
    let (_, history) = match state.reads.log_snapshot(&caller, &id).await {
        Ok(snapshot) => snapshot,
        Err(error) => return read_failure(error, "conversation_not_found"),
    };
    let all = project(&id, &history);
    let start = match query.after {
        None => 0,
        Some(after) => match all.iter().position(|item| item["id"] == after) {
            Some(index) => index + 1,
            None => {
                return super::field_refusal(
                    "after",
                    "after must name an item in this conversation.",
                )
            }
        },
    };
    let has_more = all.len() > start.saturating_add(limit);
    let data: Vec<_> = all.into_iter().skip(start).take(limit).collect();
    let id_of = |item: &Value| item["id"].as_str().map(str::to_owned);
    Json(ConversationItemList {
        object: "list",
        first_id: data.first().and_then(id_of),
        last_id: data.last().and_then(id_of),
        data,
        has_more,
    })
    .into_response()
}

pub(super) fn project(id: &AgentId, history: &[AgentLogEntry]) -> Vec<Value> {
    let mut items = Vec::new();
    let mut tools: HashMap<String, usize> = HashMap::new();
    let mut turn = 0;
    let mut output_index = 0;
    for entry in history {
        match &entry.kind {
            AgentLogKind::TurnStarted { turn_sequence, .. } => {
                turn = *turn_sequence;
                output_index = 0;
                tools.clear();
            }
            AgentLogKind::OwnerMessage { content } => items.push(message(
                format!("msg_{}_{}", id, entry.seq),
                "user",
                "input_text",
                content,
            )),
            AgentLogKind::AgentMessage { content } => {
                items.push(message(
                    super::retrieve::output_id(id, turn, output_index),
                    "assistant",
                    "output_text",
                    content,
                ));
                output_index += 1;
            }
            AgentLogKind::ToolCallStarted {
                call_id,
                title,
                kind,
                status,
            } => {
                tools.insert(call_id.clone(), items.len());
                items.push(json!({"id": format!("tool_{}_{}", id, entry.seq), "type": "cma_chat.tool_activity",
                    "call_id": call_id, "kind": kind, "target": title, "status": status, "duration_ms": null}));
            }
            AgentLogKind::ToolCallUpdated {
                call_id,
                title,
                status,
                output,
            } => {
                if let Some(index) = tools.get(call_id) {
                    items[*index]["status"] = status.clone().into();
                    if let Some(output) = output {
                        items[*index]["output"] = json!(output);
                    }
                    if let Some(title) = title {
                        items[*index]["target"] = title.clone().into();
                    }
                } else {
                    tools.insert(call_id.clone(), items.len());
                    items.push(json!({"id": format!("tool_{}_{}", id, entry.seq), "type": "cma_chat.tool_activity",
                        "call_id": call_id, "kind": "unknown", "target": title, "status": status, "output": output, "duration_ms": null}));
                }
            }
            AgentLogKind::ToolOutputReported { call_id, output } => {
                if let Some(index) = tools.get(call_id) {
                    items[*index]["output"] = json!(output);
                }
            }
            _ => {}
        }
    }
    items.reverse();
    items
}

fn message(id: String, role: &str, kind: &str, text: &str) -> Value {
    let mut content = json!({"type": kind, "text": text});
    if role == "assistant" {
        content["annotations"] = json!([]);
        content["logprobs"] = json!([]);
    }
    json!({"id": id, "type": "message", "status": "completed", "role": role, "content": [content]})
}
