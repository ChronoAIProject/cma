use super::{cursor, error::Error, parse, projection, shapes::EventHead, snapshot, AgentV2State};
use crate::AuthenticatedResourceCaller;
use axum::{
    extract::{Path, RawQuery, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{
    sandbox::{agent_io::AgentEventKind, AgentEventStreamAuthorization, AgentEventStreamHeader},
    AgentId,
};

/// Establish live observation independently of historical snapshot pagination.
#[utoipa::path(get, path = "/api/v2/agents/{agent_id}/events/head", tag = "agent events v2",
    params(("agent_id" = String, Path), ("Prefer" = Option<String>, Header)),
    responses((status = 200, body = EventHead), (status = 400, body = crate::rest::ProblemDocument),
    (status = 404, body = crate::rest::ProblemDocument), (status = 409, body = crate::rest::ProblemDocument),
    (status = 503, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<AgentV2State>,
    auth: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    match capture(&state, &auth, AgentId(id), raw).await {
        Ok((head, authorization)) => {
            let external = super::external_approval::Preference::new(
                projection::preference(&headers, super::external_approval::PREFERENCE),
                &state.external_review_origin,
                &authorization,
            );
            let mut response = Json(head).into_response();
            external.headers(response.headers_mut());
            response
                .headers_mut()
                .insert("cache-control", "private, no-store".parse().unwrap());
            response
        }
        Err(error) => error.response(),
    }
}

async fn capture(
    state: &AgentV2State,
    auth: &AuthenticatedResourceCaller,
    id: AgentId,
    raw: Option<String>,
) -> Result<(EventHead, AgentEventStreamAuthorization), Error> {
    if state.single_fence_pages {
        return super::buffered::head(state, auth, id, raw).await;
    }
    parse::head(raw)?;
    let authorization = snapshot::authorize(state, auth, &id).await?;
    let header = state.reads.event_stream_header(&authorization).await?;
    let head = project(state, &authorization, header).await?;
    Ok((head, authorization))
}

pub(super) async fn project(
    state: &AgentV2State,
    authorization: &AgentEventStreamAuthorization,
    header: AgentEventStreamHeader,
) -> Result<EventHead, Error> {
    let identity = cursor::incarnation(&header);
    let active_turn = match header.active_turn {
        None => None,
        Some(turn) => {
            if turn == 0 || turn > header.head {
                return Err(Error::History);
            }
            // Read only the captured turn start, even if completion/new work
            // has committed since capture. Those later events belong to the tail.
            let window =
                projection::window(state, authorization, turn - 1, Some(turn), 1, false).await?;
            if window.header.id != header.id || cursor::incarnation(&window.header) != identity {
                return Err(Error::Incarnation);
            }
            let mut items = projection::items(&window, &identity, false, false)?;
            if items.len() != 1 {
                return Err(Error::History);
            }
            let item = items.remove(0);
            if item.sequence != turn
                || !matches!(item.kind, Some(AgentEventKind::TurnStarted { turn_sequence }) if turn_sequence == turn)
            {
                return Err(Error::History);
            }
            Some(item)
        }
    };
    let result = EventHead {
        object: "cma.agent.event_head",
        schema_version: 1,
        agent_id: header.id.to_string(),
        incarnation: identity.clone(),
        through_cursor: cursor::event(&identity, header.head),
        active_turn,
    };
    state.reads.validate_event_stream(authorization).await?;
    Ok(result)
}
