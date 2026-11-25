//! Transport selection and projection around the fenced core operation.
use super::{
    cursor,
    error::Error,
    parse, projection,
    shapes::{EventHead, EventPage},
    snapshot, AgentV2State,
};
use crate::AuthenticatedResourceCaller;
use layer2_core::{
    sandbox::{AgentBufferedEventRequest, AgentEventStreamAuthorization},
    AgentId,
};

pub(super) async fn page(
    state: &AgentV2State,
    auth: &AuthenticatedResourceCaller,
    id: AgentId,
    raw: Option<String>,
    preferences: projection::Preferences,
) -> Result<(EventPage, super::external_approval::Preference), Error> {
    let (limit, page_cursor) = parse::snapshot(raw)?;
    let authorization = snapshot::authorize(state, auth, &id).await?;
    let external = super::external_approval::Preference::new(
        preferences.external_approval,
        &state.external_review_origin,
        &authorization,
    );
    let window = state
        .reads
        .buffered_event_window(&authorization, &state.commands, |header| {
            let identity = cursor::incarnation(header);
            let (through, after) = match page_cursor {
                Some(value) => cursor::parse_page(&value, &identity, header.head)?,
                None => (header.head, 0),
            };
            Ok::<_, Error>(AgentBufferedEventRequest::Page {
                after,
                through,
                limit,
                message_bindings: preferences.message_bindings,
            })
        })
        .await?;
    let identity = cursor::incarnation(&window.header);
    let mut items = projection::items(
        &window,
        &identity,
        preferences.tool_output,
        preferences.retained_output,
    )?;
    external.project(&mut items)?;
    // A successful forward window is empty only at its frozen upper bound;
    // oversized first entries and incomplete intervals fail in core.
    let delivered = items.last().map_or(window.through, |item| item.sequence);
    let page = EventPage {
        object: "cma.agent.event_page",
        schema_version: 1,
        agent_id: id.to_string(),
        incarnation: identity.clone(),
        through_cursor: cursor::event(&identity, window.through),
        items,
        next_page_cursor: (delivered < window.through)
            .then(|| cursor::page(&identity, window.through, delivered)),
    };
    if serde_json::to_vec(&page).map_err(|_| Error::History)?.len() > projection::MAX_BYTES {
        return Err(Error::History);
    }
    Ok((page, external))
}

pub(super) async fn head(
    state: &AgentV2State,
    auth: &AuthenticatedResourceCaller,
    id: AgentId,
    raw: Option<String>,
) -> Result<(EventHead, AgentEventStreamAuthorization), Error> {
    parse::head(raw)?;
    let authorization = snapshot::authorize(state, auth, &id).await?;
    let window = state
        .reads
        .buffered_event_window(&authorization, &state.commands, |_| {
            Ok::<_, Error>(AgentBufferedEventRequest::Head)
        })
        .await?;
    let identity = cursor::incarnation(&window.header);
    let active_turn = if window.header.active_turn.is_some() {
        Some(
            projection::items(&window, &identity, false, false)?
                .into_iter()
                .next()
                .ok_or(Error::History)?,
        )
    } else {
        None
    };
    let head = EventHead {
        object: "cma.agent.event_head",
        schema_version: 1,
        agent_id: id.to_string(),
        incarnation: identity.clone(),
        through_cursor: cursor::event(&identity, window.header.head),
        active_turn,
    };
    Ok((head, authorization))
}
