use super::{
    cursor, error::Error, parse, projection, shapes::HistoryWindow, snapshot, AgentV2State,
};
use crate::AuthenticatedResourceCaller;
use axum::{
    extract::{Path, RawQuery, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::AgentId;

#[utoipa::path(get, path = "/api/v2/agents/{agent_id}/history", tag = "agent events v2",
    params(("agent_id" = String, Path), ("limit" = Option<u16>, Query), ("through_cursor" = Option<String>, Query), ("Prefer" = Option<String>, Header)),
    responses((status = 200, body = HistoryWindow), (status = 400, body = crate::rest::ProblemDocument),
    (status = 404, body = crate::rest::ProblemDocument), (status = 409, body = crate::rest::ProblemDocument),
    (status = 503, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<AgentV2State>,
    auth: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    match page(&state, &auth, AgentId(id), raw, &headers).await {
        Ok(page) => {
            let mut response = Json(page).into_response();
            response
                .headers_mut()
                .insert("cache-control", "private, no-store".parse().unwrap());
            response
        }
        Err(error) => error.response(),
    }
}

async fn page(
    state: &AgentV2State,
    auth: &AuthenticatedResourceCaller,
    id: AgentId,
    raw: Option<String>,
    headers: &HeaderMap,
) -> Result<HistoryWindow, Error> {
    let (limit, through_cursor) = parse::history(raw)?;
    let authorization = snapshot::authorize(state, auth, &id).await?;
    let header = state.reads.event_stream_header(&authorization).await?;
    let identity = cursor::incarnation(&header);
    let through = through_cursor
        .as_ref()
        .map(|value| cursor::parse_event(value, &identity, header.head))
        .transpose()?
        .unwrap_or(header.head);
    let window = state
        .reads
        .event_history_window(&authorization, through, limit)
        .await?;
    if cursor::incarnation(&window.header) != identity {
        return Err(Error::Incarnation);
    }
    let bindings = projection::message_bindings(headers);
    let window = projection::bind_window(state, &authorization, window, bindings).await?;
    let items = projection::items(&window, &identity, projection::tool_output(headers))?;
    if items.last().map_or(0, |item| item.sequence) != through {
        return Err(Error::History);
    }
    let after = items.first().map_or(0, |item| item.sequence - 1);
    let (context, gap) = match state
        .reads
        .event_history_context(&authorization, after)
        .await?
    {
        None => (None, None),
        Some(context) => {
            if cursor::incarnation(&context.header) != identity {
                return Err(Error::Incarnation);
            }
            let context = projection::bind_window(state, &authorization, context, bindings).await?;
            let mut items = projection::items(&context, &identity, false)?;
            if items.is_empty() || items.len() > 2 {
                return Err(Error::History);
            }
            let gap = (items.len() == 2).then(|| items.pop().unwrap());
            (items.pop(), gap)
        }
    };
    let page = HistoryWindow {
        object: "cma.agent.history_window",
        schema_version: 1,
        agent_id: id.to_string(),
        incarnation: identity.clone(),
        start_cursor: cursor::event(&identity, after),
        through_cursor: cursor::event(&identity, through),
        context,
        gap,
        items,
        previous_cursor: (after > 0).then(|| cursor::event(&identity, after)),
    };
    if serde_json::to_vec(&page).map_err(|_| Error::History)?.len() > projection::MAX_BYTES {
        return Err(Error::History);
    }
    state.reads.validate_event_stream(&authorization).await?;
    Ok(page)
}
