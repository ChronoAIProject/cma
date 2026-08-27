//! Read-only Trigger companions for AG-UI clients.

use std::collections::VecDeque;
use std::convert::Infallible;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use futures_util::stream::{self, Stream};
use serde::Deserialize;

use crate::auth::AuthenticatedCaller;
use crate::rest::triggers::{
    load_diff_review, load_launch, load_launch_progress, load_public_preview,
    public_preview_response, DiffReviewView, LaunchEventView, LaunchView, TriggerProblem,
};
use crate::rest::{RequestFailure, TriggersState};

use super::resources::POLL_INTERVAL;
use super::KEEPALIVE_INTERVAL;

const DEFAULT_PROGRESS_LIMIT: usize = 100;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProgressParams {
    #[serde(default, alias = "after")]
    starting_after: Option<u64>,
    #[serde(default = "default_progress_limit")]
    limit: usize,
    #[serde(default)]
    follow: Option<bool>,
}

const fn default_progress_limit() -> usize {
    DEFAULT_PROGRESS_LIMIT
}

pub(crate) fn router(state: TriggersState) -> Router {
    Router::new()
        .route("/api/v1/agui/triggers/{public_id}", get(preview))
        .route("/api/v1/agui/launches/{launch_id}", get(launch))
        .route("/api/v1/agui/launches/{launch_id}/events", get(progress))
        .route("/api/v1/agui/launches/{launch_id}/diff", get(diff))
        .with_state(state)
}

async fn preview(
    State(state): State<TriggersState>,
    Path(public_id): Path<String>,
) -> Result<Response, TriggerProblem> {
    Ok(public_preview_response(
        load_public_preview(&state, public_id).await?,
    ))
}

async fn launch(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
) -> Result<Json<LaunchView>, TriggerProblem> {
    Ok(Json(load_launch(&state, &caller.subject, launch_id).await?))
}

async fn progress(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
    headers: HeaderMap,
    params: Result<Query<ProgressParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, RequestFailure> {
    let Query(params) = params?;
    let header_cursor = headers
        .get("last-event-id")
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| {
                    layer2_core::TriggerValidationError::field(
                        "last_event_id",
                        "must be a non-negative Launch event sequence",
                    )
                })
        })
        .transpose()
        .map_err(layer2_core::TriggerServiceError::from)?;
    let resume = header_cursor.or(params.starting_after).unwrap_or_default();
    let follows = params.follow.unwrap_or(true);
    let first = load_launch_progress(
        &state,
        &caller.subject,
        launch_id.clone(),
        resume,
        params.limit,
    )
    .await?;

    struct Following {
        state: TriggersState,
        owner: layer2_core::SubjectId,
        launch_id: String,
        sequence: u64,
        limit: usize,
        pending: VecDeque<LaunchEventView>,
        follows: bool,
    }

    let opening = Following {
        state,
        owner: caller.subject,
        launch_id,
        sequence: resume,
        limit: params.limit,
        pending: first.events.into(),
        follows,
    };
    let stream = stream::unfold(opening, |mut open| async move {
        loop {
            if let Some(event) = open.pending.pop_front() {
                open.sequence = event.sequence;
                return Some((Ok(launch_event_frame(&event)), open));
            }
            if !open.follows {
                return None;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
            match load_launch_progress(
                &open.state,
                &open.owner,
                open.launch_id.clone(),
                open.sequence,
                open.limit,
            )
            .await
            {
                Ok(progress) => open.pending.extend(progress.events),
                // The Launch disappeared, ownership changed, or durable
                // progress became unreadable. End rather than continuing a
                // stream the caller may no longer be entitled to read.
                Err(_) => return None,
            }
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(KEEPALIVE_INTERVAL)))
}

fn launch_event_frame(event: &LaunchEventView) -> SseEvent {
    let payload = serde_json::to_string(event)
        .expect("the credential-free Launch event projection is serializable");
    SseEvent::default()
        .id(event.sequence.to_string())
        .data(payload)
}

async fn diff(
    State(state): State<TriggersState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(launch_id): Path<String>,
) -> Result<Json<DiffReviewView>, TriggerProblem> {
    Ok(Json(
        load_diff_review(&state, &caller.subject, launch_id).await?,
    ))
}
