use super::{cursor, error::Error, parse, projection, shapes::StatePage, snapshot, AgentV2State};
use crate::AuthenticatedResourceCaller;
use axum::{
    extract::{Path, RawQuery, State},
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::AgentId;

#[utoipa::path(get, path = "/api/v2/agents/{agent_id}/state", tag = "agent events v2",
    params(("agent_id" = String, Path), ("through_cursor" = Option<String>, Query), ("after_approval_cursor" = Option<String>, Query)),
    responses((status = 200, body = StatePage), (status = 400, body = crate::rest::ProblemDocument),
    (status = 404, body = crate::rest::ProblemDocument), (status = 409, body = crate::rest::ProblemDocument),
    (status = 503, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<AgentV2State>,
    auth: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    match page(&state, &auth, AgentId(id), raw).await {
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
) -> Result<StatePage, Error> {
    let (through, after) = parse::current_state(raw)?;
    let authorization = snapshot::authorize(state, auth, &id).await?;
    let header = state.reads.event_stream_header(&authorization).await?;
    let identity = cursor::incarnation(&header);
    let through = through
        .as_ref()
        .map(|value| cursor::parse_event(value, &identity, header.head))
        .transpose()?
        .unwrap_or(header.head);
    if through != header.head {
        return Err(Error::StateChanged);
    }
    let after = after
        .as_ref()
        .map(|value| cursor::parse_event(value, &identity, through))
        .transpose()?
        .unwrap_or(0);
    let (context, approvals, next) = state
        .reads
        .event_current_state(&authorization, through, after, 100)
        .await?;
    if cursor::incarnation(&context.header) != identity
        || cursor::incarnation(&approvals.header) != identity
    {
        return Err(Error::Incarnation);
    }
    let context = projection::bind_window(state, &authorization, context, false).await?;
    let expected_context = context.entries.len();
    let expected_approvals = approvals.entries.len();
    let page = StatePage {
        object: "cma.agent.state_page",
        schema_version: 1,
        agent_id: id.to_string(),
        incarnation: identity.clone(),
        through_cursor: cursor::event(&identity, through),
        context: projection::items(&context, &identity, false)?,
        approvals: projection::items(&approvals, &identity, false)?,
        next_approval_cursor: next.map(|next| cursor::event(&identity, next)),
    };
    if page.context.len() != expected_context
        || page.approvals.len() != expected_approvals
        || serde_json::to_vec(&page).map_err(|_| Error::History)?.len() > projection::MAX_BYTES
    {
        return Err(Error::History);
    }
    state.reads.validate_event_stream(&authorization).await?;
    Ok(page)
}
