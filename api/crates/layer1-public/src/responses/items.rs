use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{CoreError, Event, EventKind, SessionId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::AuthenticatedCaller;
use crate::rest::{Problem, QueryProblem, SessionsState};

use super::{field_refusal, message_id, not_found, OBJECT_LIST};

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ItemsQuery {
    after: Option<String>,
    limit: Option<usize>,
}

/// One input or output text part in durable conversation history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct MessageContent {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<Vec<serde_json::Value>>,
}

/// Standard message item retained by a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct MessageItem {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub status: &'static str,
    pub role: &'static str,
    pub content: Vec<MessageContent>,
}

/// One bounded body attached to CMA's tool activity extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ToolActivityBody {
    pub text: String,
    pub truncated: bool,
}

/// Namespaced durable tool activity item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ToolActivityItem {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub call_id: String,
    #[serde(rename = "kind")]
    pub tool_kind: String,
    pub target: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<ToolActivityBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ToolActivityBody>,
}

/// Supported conversation item union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(untagged)]
pub enum ConversationItem {
    Message(MessageItem),
    ToolActivity(ToolActivityItem),
}

impl ConversationItem {
    fn id(&self) -> &str {
        match self {
            Self::Message(item) => &item.id,
            Self::ToolActivity(item) => &item.id,
        }
    }
}

/// OpenAI list envelope for conversation items.
#[derive(Debug, Serialize, ToSchema)]
pub struct ConversationItemList {
    pub object: &'static str,
    pub data: Vec<ConversationItem>,
    pub first_id: Option<String>,
    pub last_id: Option<String>,
    pub has_more: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/conversations/{id}/items",
    tag = "responses",
    params(
        ("id" = String, Path, description = "Chat conversation identifier"),
        ("after" = Option<String>, Query, description = "Return items older than this item"),
        ("limit" = Option<usize>, Query, description = "Page size from 1 through 100")
    ),
    responses(
        (status = 200, description = "Newest-first durable conversation items", body = ConversationItemList),
        (status = 404, description = "Unknown, foreign, or non-chat conversation", body = crate::rest::ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "Invalid pagination field", body = crate::rest::ProblemDocument, content_type = "application/problem+json")
    )
)]
pub(super) async fn list(
    State(state): State<SessionsState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    query: Result<Query<ItemsQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(query) => query,
        Err(error) => return QueryProblem(error).into_response(),
    };
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return field_refusal("limit", "limit must be an integer from 1 through 100.");
    }
    let id = SessionId(id);
    match state.sessions.get_session(&caller, &id).await {
        Ok(session) if session.chat.is_some() => {}
        Ok(_) => return not_found(),
        Err(CoreError::Forbidden | CoreError::SessionNotFound) => return not_found(),
        Err(error) => return Problem::from(error).into_response(),
    }
    let events = match state.sessions.events_since(&caller, &id, None).await {
        Ok(events) => events,
        Err(error) => return Problem::from(error).into_response(),
    };
    let all = project(&id, &events);
    let start = match query.after {
        None => 0,
        Some(after) => match all.iter().position(|item| item.id() == after) {
            Some(index) => index + 1,
            None => {
                return field_refusal(
                    "after",
                    "after must name an item in this conversation's durable history.",
                )
            }
        },
    };
    let has_more = all.len() > start.saturating_add(limit);
    let data = all.into_iter().skip(start).take(limit).collect::<Vec<_>>();
    let first_id = data.first().map(|item| item.id().to_string());
    let last_id = data.last().map(|item| item.id().to_string());
    Json(ConversationItemList {
        object: OBJECT_LIST,
        data,
        first_id,
        last_id,
        has_more,
    })
    .into_response()
}

struct OrderedItem {
    seq: u64,
    item: ConversationItem,
}

struct ToolFold {
    ordered_index: usize,
    started_at: Option<u64>,
}

pub(super) fn project(id: &SessionId, events: &[Event]) -> Vec<ConversationItem> {
    let mut ordered = Vec::<OrderedItem>::new();
    let mut tools = HashMap::<String, ToolFold>::new();
    let mut turn = None;
    let mut assistant: Option<(u64, String)> = None;

    let flush_assistant = |ordered: &mut Vec<OrderedItem>,
                           turn: Option<u64>,
                           assistant: &mut Option<(u64, String)>| {
        if let (Some(turn), Some((seq, text))) = (turn, assistant.take()) {
            ordered.push(OrderedItem {
                seq,
                item: ConversationItem::Message(MessageItem {
                    id: message_id(id, turn),
                    kind: "message",
                    status: "completed",
                    role: "assistant",
                    content: vec![MessageContent {
                        kind: "output_text".into(),
                        text,
                        annotations: Some(Vec::new()),
                        logprobs: Some(Vec::new()),
                    }],
                }),
            });
        }
    };

    for event in events {
        match &event.kind {
            EventKind::TurnStarted => {
                flush_assistant(&mut ordered, turn, &mut assistant);
                turn = Some(event.seq);
            }
            EventKind::OwnerMessage { content } => ordered.push(OrderedItem {
                seq: event.seq,
                item: ConversationItem::Message(MessageItem {
                    id: format!("msg_{}_{}", id.as_str(), event.seq),
                    kind: "message",
                    status: "completed",
                    role: "user",
                    content: vec![MessageContent {
                        kind: "input_text".into(),
                        text: content.clone(),
                        annotations: None,
                        logprobs: None,
                    }],
                }),
            }),
            EventKind::MessageCommitted { content } if turn.is_some() => {
                assistant = Some((event.seq, content.clone()));
            }
            EventKind::ToolCallStarted {
                call_id,
                title,
                kind,
                arguments,
            } => {
                let ordered_index = ordered.len();
                ordered.push(OrderedItem {
                    seq: event.seq,
                    item: ConversationItem::ToolActivity(ToolActivityItem {
                        id: format!("tool_{}_{}", id.as_str(), event.seq),
                        kind: "cma_chat.tool_activity",
                        call_id: call_id.clone(),
                        tool_kind: kind.clone(),
                        target: title.clone(),
                        status: "started".into(),
                        duration_ms: None,
                        arguments: arguments.as_ref().map(body),
                        result: None,
                    }),
                });
                tools.insert(
                    call_id.clone(),
                    ToolFold {
                        ordered_index,
                        started_at: layer2_core::seconds_from_rfc3339(&event.appended_at),
                    },
                );
            }
            EventKind::ToolCallUpdated {
                call_id,
                status,
                result,
            } => {
                if let Some(fold) = tools.get(call_id) {
                    if let ConversationItem::ToolActivity(item) =
                        &mut ordered[fold.ordered_index].item
                    {
                        item.status = status.clone();
                        item.result = result.as_ref().map(body);
                        item.duration_ms = fold.started_at.and_then(|started| {
                            layer2_core::seconds_from_rfc3339(&event.appended_at)
                                .map(|ended| ended.saturating_sub(started).saturating_mul(1_000))
                        });
                    }
                } else {
                    let ordered_index = ordered.len();
                    ordered.push(OrderedItem {
                        seq: event.seq,
                        item: ConversationItem::ToolActivity(ToolActivityItem {
                            id: format!("tool_{}_{}", id.as_str(), event.seq),
                            kind: "cma_chat.tool_activity",
                            call_id: call_id.clone(),
                            tool_kind: "unknown".into(),
                            target: "unknown".into(),
                            status: status.clone(),
                            duration_ms: None,
                            arguments: None,
                            result: result.as_ref().map(body),
                        }),
                    });
                    tools.insert(
                        call_id.clone(),
                        ToolFold {
                            ordered_index,
                            started_at: None,
                        },
                    );
                }
            }
            _ => {}
        }
    }
    flush_assistant(&mut ordered, turn, &mut assistant);
    ordered.sort_by_key(|entry| std::cmp::Reverse(entry.seq));
    ordered.into_iter().map(|entry| entry.item).collect()
}

fn body(body: &layer2_core::ToolBody) -> ToolActivityBody {
    ToolActivityBody {
        text: body.text.clone(),
        truncated: body.truncated,
    }
}
