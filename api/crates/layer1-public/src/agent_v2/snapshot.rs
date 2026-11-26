use super::{cursor, error::Error, parse, projection, shapes::EventPage, AgentV2State};
use crate::AuthenticatedResourceCaller;
use axum::{
    extract::{Path, RawQuery, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use layer2_core::{sandbox::AgentEventStreamAuthorization, AgentId};

pub(super) async fn authorize(
    state: &AgentV2State,
    auth: &AuthenticatedResourceCaller,
    id: &AgentId,
) -> Result<AgentEventStreamAuthorization, Error> {
    Ok(state
        .reads
        .authorize_event_stream(
            &auth.context,
            auth.credential
                .as_ref()
                .map(crate::DelegatedCredential::value),
            id,
        )
        .await?)
}

#[utoipa::path(get, path = "/api/v2/agents/{agent_id}/events/snapshot", tag = "agent events v2",
    params(("agent_id" = String, Path), ("limit" = Option<u16>, Query), ("page_cursor" = Option<String>, Query), ("Prefer" = Option<String>, Header)),
    responses((status = 200, body = EventPage), (status = 400, body = crate::rest::ProblemDocument),
    (status = 404, body = crate::rest::ProblemDocument), (status = 409, body = crate::rest::ProblemDocument),
    (status = 503, body = crate::rest::ProblemDocument)))]
pub(super) async fn read(
    State(state): State<AgentV2State>,
    auth: AuthenticatedResourceCaller,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    match page(
        &state,
        &auth,
        AgentId(id),
        raw,
        projection::Preferences::from_headers(&headers),
    )
    .await
    {
        Ok((page, external)) => {
            let mut response = Json(page).into_response();
            external.headers(response.headers_mut());
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
    preferences: projection::Preferences,
) -> Result<(EventPage, super::external_approval::Preference), Error> {
    if state.single_fence_pages {
        return super::buffered::page(state, auth, id, raw, preferences).await;
    }
    let (limit, page_cursor) = parse::snapshot(raw)?;
    let authorization = authorize(state, auth, &id).await?;
    let external = super::external_approval::Preference::new(
        preferences.external_approval,
        &state.external_review_origin,
        &authorization,
    );
    let header = state.reads.event_stream_header(&authorization).await?;
    let identity = cursor::incarnation(&header);
    let (through, after) = match page_cursor {
        Some(value) => cursor::parse_page(&value, &identity, header.head)?,
        None => (header.head, 0),
    };
    let window = projection::window(
        state,
        &authorization,
        after,
        Some(through),
        limit,
        preferences.message_bindings,
    )
    .await?;
    let mut items = projection::items(
        &window,
        &identity,
        preferences.tool_output,
        preferences.retained_output,
    )?;
    external.project(&mut items)?;
    let delivered = items.last().map_or(after, |item| item.sequence);
    let page = EventPage {
        object: "cma.agent.event_page",
        schema_version: 1,
        agent_id: id.to_string(),
        incarnation: identity.clone(),
        through_cursor: cursor::event(&identity, through),
        items,
        next_page_cursor: (delivered < through)
            .then(|| cursor::page(&identity, through, delivered)),
    };
    if serde_json::to_vec(&page).map_err(|_| Error::History)?.len() > projection::MAX_BYTES {
        return Err(Error::History);
    }
    state.reads.validate_event_stream(&authorization).await?;
    Ok((page, external))
}
