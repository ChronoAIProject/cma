//! Public owner-authorized recent activity. Streams carry complete snapshots,
//! so reconnect restores the latest truth without inventing missed transitions.
use super::{
    management_responses::{management_error, parse_channel_id},
    problems::{no_store, problem},
};
use crate::{
    activity::{public_page, ActivityPage},
    auth::Creator,
    channels::management::{self, ManagementError, ManagementState},
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    Json,
};
use cmaeg_core::problem::ProblemCode;
use serde::Deserialize;
use std::{convert::Infallible, time::Duration};
use utoipa::IntoParams;

#[utoipa::path(get,path="/v1/channels/{channel_id}/activity",operation_id="getChannelActivity",tag="channels",
description="Bounded recent activity. Actor id is always redacted and display_name is a fixed role label (Participant, Bot, System, or Actor not reported); kind is preserved. This presentation copy is not canonical input for routing or replay. Event identity, cursor, timestamps and consumer/delivery state are preserved.",
security(("creatorBearer"=[])),params(("channel_id"=String,Path),("X-CMAEG-Owner"=Option<String>,Header)),
responses((status=200,body=ActivityPage),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn latest(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw): Path<String>,
) -> Response {
    let id = match parse_channel_id(&raw) {
        Ok(id) => id,
        Err(error) => return management_error(error),
    };
    let channel = match owned(&state, &creator, &id).await {
        Ok(c) => c,
        Err(error) => return management_error(error),
    };
    match state
        .activity
        .latest(&creator.subject, channel.definition.record_id.as_str())
        .await
    {
        Ok(page) => no_store(Json(public_page(&page)).into_response()),
        Err(error) => management_error(ManagementError::Record(error)),
    }
}

#[derive(Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in=Query)]
pub struct ActivityQuery {
    pub cursor: Option<String>,
}

#[utoipa::path(get,path="/v1/channels/{channel_id}/activity/stream",operation_id="streamChannelActivity",tag="channels",
description="SSE activity/reset events contain complete ActivityPage snapshots, including on every connection. Actor id is always redacted and display_name is a fixed role label (Participant, Bot, System, or Actor not reported); kind is preserved. This presentation copy is not canonical input for routing or replay. Apply each snapshot atomically. id is an opaque cursor; reconnect with Last-Event-ID or cursor. reset means the cursor predates retained activity. This is recent bounded activity, not a complete audit log.",
security(("creatorBearer"=[])),params(ActivityQuery,("channel_id"=String,Path),("Last-Event-ID"=Option<String>,Header),("X-CMAEG-Owner"=Option<String>,Header)),
responses((status=200,body=String,content_type="text/event-stream"),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn stream(
    State(state): State<ManagementState>,
    creator: Creator,
    Path(raw): Path<String>,
    Query(query): Query<ActivityQuery>,
    headers: HeaderMap,
) -> Response {
    let id = match parse_channel_id(&raw) {
        Ok(id) => id,
        Err(error) => return management_error(error),
    };
    let channel = match owned(&state, &creator, &id).await {
        Ok(c) => c,
        Err(error) => return management_error(error),
    };
    let record = channel.definition.record_id.to_string();
    let cursor = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .or(query.cursor.as_deref());
    let reset = match cursor
        .map(|cursor| state.activity.cursor_reset(&record, cursor))
        .transpose()
    {
        Ok(value) => value.unwrap_or(false),
        Err(_) => {
            return no_store(problem(
                StatusCode::BAD_REQUEST,
                ProblemCode::InvalidRequest,
                "invalid channel activity cursor",
            ))
        }
    };
    // Register first: a write committed between this read and consuming the
    // stream stays buffered and triggers another authoritative snapshot.
    let updates = state.activity.subscribe();
    let page = match state.activity.latest(&creator.subject, &record).await {
        Ok(page) => page,
        Err(error) => return management_error(ManagementError::Record(error)),
    };
    let first = event(&page, reset);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5 * 60);
    let mut auth_headers = HeaderMap::new();
    for name in ["authorization", "x-cmaeg-owner"] {
        for value in headers.get_all(name) {
            auth_headers.append(name, value.clone());
        }
    }
    let stream = futures_util::stream::unfold(
        (
            state,
            creator,
            id,
            record,
            updates,
            Some(first),
            page.cursor,
            deadline,
            auth_headers,
        ),
        |(
            state,
            creator,
            id,
            record,
            mut updates,
            mut first,
            mut cursor,
            deadline,
            auth_headers,
        )| async move {
            if let Some(event) = first.take() {
                return Some((
                    Ok::<_, Infallible>(event),
                    (
                        state,
                        creator,
                        id,
                        record,
                        updates,
                        first,
                        cursor,
                        deadline,
                        auth_headers,
                    ),
                ));
            }
            let mut refresh_at = tokio::time::Instant::now() + Duration::from_secs(1);
            loop {
                // A draining Gateway and its replacement can both commit to
                // shared records. Local notifications give immediate delivery;
                // a bounded server-side refresh also observes the other process.
                // Browsers keep one SSE connection and never poll snapshots.
                let (reset, refresh) = tokio::select! {
                    biased;
                    _ = tokio::time::sleep_until(deadline) => return None,
                    _ = tokio::time::sleep_until(refresh_at) => (false, true),
                    update = updates.recv() => match update {
                        Ok(updated) if updated == record => (false, false),
                        Ok(_) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => (true, false),
                        Err(_) => return None,
                    },
                };
                // Reuse the same credential validation/cache policy as public
                // reads. Token expiry, revoked read permission, owner change or
                // channel deletion closes the stream before further data.
                let read = async {
                    let current = state.creator_auth.authenticate(&auth_headers).await.ok()?;
                    if current != creator {
                        return None;
                    }
                    management::inspect(&state, &creator, &id).await.ok()?;
                    state
                        .activity
                        .latest_if_changed(
                            &creator.subject,
                            &record,
                            refresh.then_some(cursor.as_str()),
                        )
                        .await
                        .ok()
                };
                let bound = deadline.min(tokio::time::Instant::now() + Duration::from_secs(5));
                let page = tokio::time::timeout_at(bound, read).await.ok()??;
                refresh_at = tokio::time::Instant::now() + Duration::from_secs(1);
                let Some(page) = page else { continue };
                if !reset && page.cursor == cursor {
                    continue;
                }
                cursor = page.cursor.clone();
                let next = event(&page, reset);
                return Some((
                    Ok(next),
                    (
                        state,
                        creator,
                        id,
                        record,
                        updates,
                        first,
                        cursor,
                        deadline,
                        auth_headers,
                    ),
                ));
            }
        },
    );
    let mut response = no_store(
        Sse::new(stream)
            .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
            .into_response(),
    );
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    response
}
fn event(page: &ActivityPage, reset: bool) -> Event {
    Event::default()
        .event(if reset { "reset" } else { "activity" })
        .id(&page.cursor)
        .json_data(public_page(page))
        .expect("activity snapshot serializes")
}
pub(super) async fn owned(
    state: &ManagementState,
    creator: &Creator,
    id: &cmaeg_core::ids::ChannelId,
) -> Result<management::LoadedChannel, ManagementError> {
    let (channel, _) = tokio::time::timeout(
        Duration::from_secs(3),
        management::inspect(state, creator, id),
    )
    .await
    .map_err(|_| {
        ManagementError::Record(crate::records::RecordError::Unavailable(
            "channel activity authorization timed out".into(),
        ))
    })??;
    Ok(channel)
}
