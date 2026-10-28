use super::{error, ResponsesV2State};
use crate::AuthenticatedResourceCaller;
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    sandbox::{AgentEventStreamSnapshot, AgentLogKind},
    AgentId,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

type Auth = Result<AuthenticatedResourceCaller, crate::auth::NotAuthenticated>;
#[utoipa::path(get, path = "/api/v2/conversations/{conversation_id}", tag = "responses v2", params(("conversation_id" = String, Path)), responses((status = 200, body = super::super::Conversation), (status = 404, body = Value)))]
pub(super) async fn retrieve(
    State(state): State<ResponsesV2State>,
    auth: Auth,
    Path(id): Path<String>,
) -> Response {
    let snapshot = match snapshot(&state, auth, &AgentId(id)).await {
        Ok(value) => value,
        Err(error) => return *error,
    };
    Json(super::super::Conversation {
        id: snapshot.id.to_string(),
        object: "conversation",
        created_at: layer2_core::seconds_from_rfc3339(&snapshot.created_at).unwrap_or_default(),
        metadata: BTreeMap::new(),
    })
    .into_response()
}
#[utoipa::path(get, path = "/api/v2/conversations/{conversation_id}/items", tag = "responses v2", params(("conversation_id" = String, Path), ("after" = Option<String>, Query), ("before" = Option<String>, Query), ("limit" = Option<u16>, Query), ("order" = Option<String>, Query)), responses((status = 200, body = Value)))]
pub(super) async fn items(
    State(state): State<ResponsesV2State>,
    auth: Auth,
    Path(id): Path<String>,
    query: Result<Query<BTreeMap<String, String>>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(Query(mut query)) = query else {
        return error::field(
            "query",
            "invalid_items_query",
            "Invalid conversation items query.",
        );
    };
    for field in query.keys() {
        if !matches!(field.as_str(), "after" | "before" | "limit" | "order") {
            return error::field(
                field,
                "responses_field_unsupported",
                "This conversation items field is unsupported.",
            );
        }
    }
    let limit = match query.remove("limit") {
        None => 20,
        Some(value) => match value.parse::<usize>() {
            Ok(value @ 1..=100) => value,
            _ => {
                return error::field(
                    "limit",
                    "invalid_items_limit",
                    "limit must be between 1 and 100.",
                )
            }
        },
    };
    let ascending = match query.remove("order").as_deref() {
        None | Some("desc") => false,
        Some("asc") => true,
        _ => return error::field("order", "invalid_items_order", "order must be asc or desc."),
    };
    let after = query.remove("after");
    let before = query.remove("before");
    if after.is_some() && before.is_some() {
        return error::field(
            "before",
            "invalid_items_cursor",
            "Supply after or before, not both.",
        );
    }
    let snapshot = match snapshot(&state, auth, &AgentId(id)).await {
        Ok(value) => value,
        Err(error) => return *error,
    };
    let mut all = project(&snapshot);
    if !ascending {
        all.reverse();
    }
    let mut start = 0;
    let mut end = all.len();
    for (is_after, cursor) in [(true, after), (false, before)] {
        if let Some(cursor) = cursor {
            let Some(index) = all.iter().position(|item| item["id"] == cursor) else {
                return error::field(
                    if is_after { "after" } else { "before" },
                    "invalid_items_cursor",
                    "The cursor must identify an item in this conversation.",
                );
            };
            if is_after {
                start = index + 1;
            } else {
                end = index;
            }
        }
    }
    let has_more = end.saturating_sub(start) > limit;
    let data: Vec<_> = all
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start).min(limit))
        .collect();
    Json(json!({"object":"list","first_id":data.first().map(|item| &item["id"]),"last_id":data.last().map(|item| &item["id"]),"has_more":has_more,"data":data})).into_response()
}
async fn snapshot(
    state: &ResponsesV2State,
    auth: Auth,
    id: &AgentId,
) -> Result<AgentEventStreamSnapshot, Box<Response>> {
    let auth = auth.map_err(|_| Box::new(error::unauthenticated()))?;
    let id = state.resolve_conversation(&auth, id, None).await?;
    let authorization = state
        .reads
        .authorize_event_stream(
            &auth.context,
            auth.credential.as_ref().map(|value| value.value()),
            &id,
        )
        .await
        .map_err(|value| Box::new(error::read(value)))?;
    state
        .reads
        .event_stream_snapshot(&authorization)
        .await
        .map_err(|value| Box::new(error::read(value)))
}
fn project(snapshot: &AgentEventStreamSnapshot) -> Vec<Value> {
    let mut items = Vec::new();
    let mut turn = 0;
    let mut output_index = 0;
    for entry in &snapshot.history {
        let (id, role, kind, text) = match &entry.kind {
            Some(AgentLogKind::TurnStarted { turn_sequence, .. }) => {
                turn = *turn_sequence;
                output_index = 0;
                continue;
            }
            Some(AgentLogKind::OwnerMessage { content }) => (
                format!("msg_{}_{}", snapshot.id, entry.seq),
                "user",
                "input_text",
                content,
            ),
            Some(AgentLogKind::AgentMessage { content }) => {
                let id = super::super::retrieve::output_id(&snapshot.id, turn, output_index);
                output_index += 1;
                (id, "assistant", "output_text", content)
            }
            _ => continue,
        };
        let mut part = json!({"type":kind,"text":text});
        if role == "assistant" {
            part["annotations"] = json!([]);
            part["logprobs"] = json!([]);
        }
        items.push(
            json!({"id":id,"type":"message","status":"completed","role":role,"content":[part]}),
        );
    }
    items
}
