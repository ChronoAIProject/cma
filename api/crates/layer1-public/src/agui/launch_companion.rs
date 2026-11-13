//! Owner-only SSE replay of one Launch's durable events.
use crate::stream_telemetry::{self, Kind, Poll};

use std::{collections::VecDeque, convert::Infallible, sync::Arc, time::Duration};

use axum::{
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::get,
    Router,
};
use futures_util::{stream, Stream};
use layer2_core::{
    trigger_domain::{
        LaunchEvent, LaunchStore, LaunchStoreError, DEFAULT_LAUNCH_EVENT_LIMIT,
        MAX_LAUNCH_EVENT_LIMIT,
    },
    LaunchId, SubjectId,
};
use serde::Deserialize;

use crate::{
    rest::{is_embed_identifier, problem_document, problem_response, LaunchEventView},
    AuthenticatedCaller,
};

const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const LAST_EVENT_ID: &str = "last-event-id";

/// Durable Launch storage needed by the AG-UI companion.
#[derive(Clone)]
pub struct LaunchCompanionState {
    pub launches: Arc<dyn LaunchStore>,
}

impl LaunchCompanionState {
    pub fn new(launches: Arc<dyn LaunchStore>) -> Self {
        Self { launches }
    }
}

/// Compose the owner-only Launch event companion independently of REST routes.
pub fn launch_companion_router(state: LaunchCompanionState) -> Router {
    Router::new()
        .route("/api/v1/agui/launches/{id}/events", get(events))
        .with_state(state)
}

async fn events(
    State(state): State<LaunchCompanionState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if !valid_launch_id(&id) {
        return CompanionProblem::NotFound.into_response();
    }
    let id = LaunchId(id);
    let owner = caller.subject;
    let launch = match state.launches.launch(&id, &owner).await {
        Ok(Some(launch)) => launch,
        Ok(None) => return CompanionProblem::NotFound.into_response(),
        Err(_) => return CompanionProblem::Unavailable.into_response(),
    };

    let input = match StreamInput::parse(raw_query.as_deref(), &headers) {
        Ok(input) => input,
        Err(problem) => return problem.into_response(),
    };
    if input.after > launch.event_head {
        return CompanionProblem::CursorBeyondEnd {
            head: launch.event_head,
        }
        .into_response();
    }

    let first_page = match state
        .launches
        .launch_events(&id, &owner, input.after, input.limit)
        .await
    {
        Ok(events) => events,
        Err(LaunchStoreError::NotFound) => return CompanionProblem::NotFound.into_response(),
        Err(_) => return CompanionProblem::Unavailable.into_response(),
    };
    if !valid_page(&first_page, &id, input.after, input.limit) {
        return CompanionProblem::Unavailable.into_response();
    }

    let replay = Replay {
        launches: state.launches,
        id,
        owner,
        after: input.after,
        limit: input.limit,
        follow: input.follow,
        page_was_full: first_page.len() == usize::from(input.limit),
        buffered: first_page.into_iter().map(Into::into).collect(),
    };
    Sse::new(stream_telemetry::observe(
        replay_stream(replay),
        Kind::LaunchEvents,
    ))
    .keep_alive(
        KeepAlive::new()
            .interval(KEEPALIVE_INTERVAL)
            .text("keepalive"),
    )
    .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryInput {
    #[serde(default, alias = "after")]
    starting_after: Option<String>,
    #[serde(default)]
    limit: Option<String>,
    #[serde(default)]
    follow: Option<String>,
}

struct StreamInput {
    after: u64,
    limit: u16,
    follow: bool,
}

impl StreamInput {
    fn parse(raw_query: Option<&str>, headers: &HeaderMap) -> Result<Self, CompanionProblem> {
        let query = match raw_query {
            Some(raw) if !well_formed_percent_encoding(raw) => {
                return Err(CompanionProblem::InvalidQuery)
            }
            Some(raw) => serde_urlencoded::from_str::<QueryInput>(raw)
                .map_err(|_| CompanionProblem::InvalidQuery)?,
            None => QueryInput {
                starting_after: None,
                limit: None,
                follow: None,
            },
        };
        let query_cursor = parse_optional(query.starting_after.as_deref(), parse_decimal)?;
        let header_cursor = last_event_id(headers)?;
        let limit = parse_optional(query.limit.as_deref(), parse_limit)?
            .unwrap_or(DEFAULT_LAUNCH_EVENT_LIMIT);
        let follow = parse_optional(query.follow.as_deref(), parse_boolean)?.unwrap_or(true);
        Ok(Self {
            after: header_cursor.or(query_cursor).unwrap_or(0),
            limit,
            follow,
        })
    }
}

fn parse_optional<T>(
    value: Option<&str>,
    parser: fn(&str) -> Option<T>,
) -> Result<Option<T>, CompanionProblem> {
    value
        .map(|value| parser(value).ok_or(CompanionProblem::InvalidQuery))
        .transpose()
}

fn last_event_id(headers: &HeaderMap) -> Result<Option<u64>, CompanionProblem> {
    let mut values = headers.get_all(LAST_EVENT_ID).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(CompanionProblem::InvalidQuery);
    }
    let value = value.to_str().map_err(|_| CompanionProblem::InvalidQuery)?;
    parse_decimal(value)
        .ok_or(CompanionProblem::InvalidQuery)
        .map(Some)
}

fn parse_decimal(value: &str) -> Option<u64> {
    if value == "0" {
        return Some(0);
    }
    let bytes = value.as_bytes();
    if !matches!(bytes.first(), Some(b'1'..=b'9')) || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    value.parse().ok()
}

fn parse_limit(value: &str) -> Option<u16> {
    let value = u16::try_from(parse_decimal(value)?).ok()?;
    (value > 0 && value <= MAX_LAUNCH_EVENT_LIMIT).then_some(value)
}

fn parse_boolean(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn well_formed_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len()
            || !bytes[index + 1].is_ascii_hexdigit()
            || !bytes[index + 2].is_ascii_hexdigit()
        {
            return false;
        }
        index += 3;
    }
    true
}

fn valid_launch_id(value: &str) -> bool {
    value
        .strip_prefix("lch_")
        .is_some_and(|suffix| !suffix.is_empty() && is_embed_identifier(value))
}

struct Replay {
    launches: Arc<dyn LaunchStore>,
    id: LaunchId,
    owner: SubjectId,
    after: u64,
    limit: u16,
    follow: bool,
    page_was_full: bool,
    buffered: VecDeque<LaunchEventView>,
}

fn replay_stream(replay: Replay) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
    stream::unfold(replay, |mut replay| async move {
        loop {
            if let Some(event) = replay.buffered.pop_front() {
                replay.after = event.seq;
                return Some((Ok(sse_event(event)), replay));
            }
            if !replay.follow {
                return None;
            }
            if !replay.page_was_full {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            replay.page_was_full = false;
            let poll = Poll::new(Kind::LaunchEvents);
            let launch = match replay.launches.launch(&replay.id, &replay.owner).await {
                Ok(Some(launch)) => launch,
                Ok(None) | Err(_) => return None,
            };
            if replay.after > launch.event_head {
                return None;
            }
            let events = match replay
                .launches
                .launch_events(&replay.id, &replay.owner, replay.after, replay.limit)
                .await
            {
                Ok(events) => events,
                Err(_) => return None,
            };
            if !valid_page(&events, &replay.id, replay.after, replay.limit) {
                return None;
            }
            poll.complete(!events.is_empty());
            replay.page_was_full = events.len() == usize::from(replay.limit);
            replay.buffered = events.into_iter().map(Into::into).collect();
        }
    })
}

fn valid_page(events: &[LaunchEvent], id: &LaunchId, after: u64, limit: u16) -> bool {
    if events.len() > usize::from(limit) {
        return false;
    }
    let mut previous = after;
    for event in events {
        if &event.launch_id != id || event.seq <= previous {
            return false;
        }
        previous = event.seq;
    }
    true
}

fn sse_event(event: LaunchEventView) -> Event {
    let id = event.seq.to_string();
    let data = serde_json::to_string(&event)
        .expect("the closed Launch event wire projection always serializes");
    Event::default().id(id).data(data)
}

enum CompanionProblem {
    NotFound,
    InvalidQuery,
    CursorBeyondEnd { head: u64 },
    Unavailable,
}

impl IntoResponse for CompanionProblem {
    fn into_response(self) -> Response {
        let (status, slug, detail, code, head) = match self {
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "launch-not-found",
                "No Launch matches this request.",
                "launch_not_found",
                None,
            ),
            Self::InvalidQuery => (
                StatusCode::BAD_REQUEST,
                "invalid-launch-event-query",
                "The Launch event stream parameters are not valid.",
                "invalid_launch_event_query",
                None,
            ),
            Self::CursorBeyondEnd { head } => (
                StatusCode::BAD_REQUEST,
                "cursor-beyond-end",
                "The Launch event cursor is beyond the durable event log.",
                "cursor_beyond_end",
                Some(head),
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "launch-store-unavailable",
                "The Launch store could not complete this operation.",
                "launch_store_unavailable",
                None,
            ),
        };
        let mut body = problem_document(status, slug, detail, code);
        if let (Some(head), Some(object)) = (head, body.as_object_mut()) {
            object.insert("head".into(), head.into());
        }
        problem_response(status, body)
    }
}
