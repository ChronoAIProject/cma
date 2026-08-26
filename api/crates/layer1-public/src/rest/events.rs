//! A session's event log, as a stream.
//!
//! Creating a session returns before its runtime exists, so a client needs a
//! way to learn what happened next. This projects the durable log the core
//! already keeps: nothing here is new state, and the stream can be replayed
//! from any point because every event carries a per-session sequence.
//!
//! Server-sent events rather than a socket, deliberately. Progress travels one
//! way, and SSE reconnects on its own carrying the last id it saw — which maps
//! exactly onto the sequence the core allocates, so a dropped connection
//! resumes without a gap and without the client having to implement resume.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::sse::{Event as SseEvent, KeepAlive, Sse},
};
use futures_util::stream::{self, Stream};
use layer2_core::{SessionId, SessionService};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::auth::AuthenticatedCaller;

use super::RequestFailure;

/// How often the log is re-read while a client is listening.
///
/// The core publishes no change notification, so this polls. Fast enough that
/// a state change feels immediate, slow enough that an idle terminal is not a
/// steady stream of queries — and a session that is merely waiting produces no
/// events, so an idle stream costs one small read per interval.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct Cursor {
    /// Resume after this sequence. Absent means from the beginning.
    ///
    /// Named as the client sends it. The name is part of the contract and is
    /// documented; a server reading a different one silently replays
    /// everything on every reconnect, which is what happened here.
    #[serde(default, alias = "after")]
    starting_after: Option<u64>,
}

impl Cursor {
    /// Where this client is resuming from.
    ///
    /// Read through a method so a test asserts the parsed query rather than
    /// the struct's shape: the bug this guards was a name the server never
    /// read, and a test that constructs the struct directly cannot see one.
    fn starting_after(&self) -> Option<u64> {
        self.starting_after
    }
}

/// Stream a session's events, resuming from a cursor.
#[utoipa::path(
    get,
    path = "/api/v1/sessions/{id}/events",
    tag = "events",
    params(
        ("id" = String, Path),
        ("starting_after" = Option<u64>, Query),
        ("Last-Event-Id" = Option<u64>, Header)
    ),
    responses((status = 200, description = "Resumable server-sent event stream", body = String, content_type = "text/event-stream"))
)]
pub(super) async fn stream(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    cursor: Result<Query<Cursor>, axum::extract::rejection::QueryRejection>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, RequestFailure> {
    let Query(cursor) = cursor?;
    let id = SessionId(id);

    // A reconnecting browser sends the last id it saw. Preferring it over the
    // query means a resume cannot silently replay from the start, which for a
    // long-lived stream would be worse than a gap.
    let resume = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .or(cursor.starting_after())
        .unwrap_or(0);

    // Authorised once, here, so a stream cannot outlive the caller's right to
    // read it by having been opened before that right was checked.
    let first = service.events_since(&caller, &id, Some(resume)).await?;

    // Carries what has been read but not yet sent, so a batch is delivered one
    // frame at a time without re-querying.
    struct Cursored {
        service: Arc<SessionService>,
        caller: layer2_core::Caller,
        id: SessionId,
        seq: u64,
        pending: std::collections::VecDeque<layer2_core::Event>,
    }

    let state = Cursored {
        service,
        caller,
        id,
        seq: resume,
        pending: first.into_iter().collect(),
    };

    let stream = stream::unfold(state, |mut state| async move {
        loop {
            if let Some(event) = state.pending.pop_front() {
                state.seq = event.seq;
                return Some((Ok(to_sse(&event)), state));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
            match state
                .service
                .events_since(&state.caller, &state.id, Some(state.seq))
                .await
            {
                Ok(events) => state.pending.extend(events),
                Err(_) => {
                    // The session is gone or no longer readable. Ending the
                    // stream says so; continuing would poll something the
                    // caller may no longer be entitled to.
                    return None;
                }
            }
        }
    });

    // Keep-alive because an idle session emits nothing for long stretches, and
    // an intermediary is entitled to drop a connection that looks dead.
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// Render a core event as an SSE frame.
///
/// The id carries the sequence, which is what makes resume work: the browser
/// hands it back on reconnect and the stream continues from there.
fn to_sse(event: &layer2_core::Event) -> SseEvent {
    SseEvent::default()
        .id(event.seq.to_string())
        .data(event_payload(event).to_string())
}

/// The JSON object carried in an SSE frame.
fn event_payload(event: &layer2_core::Event) -> serde_json::Value {
    let mut kind = serde_json::to_value(&event.kind)
        .expect("the core event vocabulary is always JSON serializable");
    // Delivery locators are durable adapter state, not part of the owner-facing
    // event contract. The public request id is the only value a client returns.
    if let Some(object) = kind.as_object_mut() {
        object.remove("locator");
    }
    serde_json::json!({
        "seq": event.seq,
        "session_id": event.session_id.0,
        "event": kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::{Event, EventKind, RuntimeLossCause, SessionStatus};

    #[test]
    fn both_cursor_spellings_parse_to_the_same_resume_position() {
        for query in ["starting_after=42", "after=42"] {
            let cursor: Cursor = serde_urlencoded::from_str(query).expect("cursor parses");
            assert_eq!(cursor.starting_after(), Some(42), "for {query}");
        }

        let cursor: Cursor = serde_urlencoded::from_str("").expect("an absent cursor parses");
        assert_eq!(cursor.starting_after(), None);
    }

    #[test]
    fn the_frame_id_is_the_sequence_so_a_client_can_resume() {
        let e = Event {
            session_id: SessionId("sesn_1".into()),
            seq: 7,
            kind: EventKind::SessionCreated,
            appended_at: String::new(),
        };
        // Serialised form is checked rather than the builder's fields, since
        // the id is only load-bearing once it reaches the wire.
        let rendered = format!("{:?}", to_sse(&e));
        assert!(rendered.contains('7'), "got {rendered}");
    }

    #[test]
    fn a_status_change_carries_the_status_a_client_waits_for() {
        let e = Event {
            session_id: SessionId("sesn_1".into()),
            seq: 2,
            kind: EventKind::StatusChanged {
                status: SessionStatus::Ready,
                stop_reason: None,
                detail: None,
                capability: None,
            },
            appended_at: String::new(),
        };
        let payload = serde_json::json!({
            "seq": e.seq,
            "session_id": e.session_id.0,
            "event": e.kind,
        })
        .to_string();
        assert!(payload.contains("ready"), "got {payload}");
        assert!(payload.contains("status_changed"), "got {payload}");
    }

    #[test]
    fn a_lost_runtime_reaches_a_rest_client_in_the_core_vocabulary() {
        let e = Event {
            session_id: SessionId("sesn_1".into()),
            seq: 9,
            kind: EventKind::RuntimeLost {
                runtime_id: "runtime-gone".into(),
                cause: Some(RuntimeLossCause::OutOfMemory),
            },
            appended_at: "2026-08-23T04:12:00Z".into(),
        };

        let rendered = format!("{:?}", to_sse(&e));
        assert!(rendered.contains("runtime_lost"), "got {rendered}");
        assert!(rendered.contains("runtime-gone"), "got {rendered}");
    }

    #[test]
    fn the_sdk_event_fixture_is_produced_by_the_server_serializer() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/rest-sdk/event.json"))
                .expect("the checked-in fixture is JSON");
        let event = Event {
            session_id: SessionId("sesn_fixture".into()),
            seq: 8,
            kind: EventKind::ApprovalRequested {
                request_id: "req_fixture".into(),
                action: Some("Run the release command".into()),
                detail: Some("This command writes a release tag".into()),
                locator: None,
            },
            appended_at: "2026-08-24T08:01:00Z".into(),
        };

        assert_eq!(event_payload(&event), fixture);
    }

    #[test]
    fn adapter_delivery_locators_never_enter_the_public_event_stream() {
        let event = Event {
            session_id: SessionId("sesn_locator_private".into()),
            seq: 9,
            kind: EventKind::ApprovalRequested {
                request_id: "req_public".into(),
                action: Some("run a command".into()),
                detail: None,
                locator: Some(layer2_core::PermissionLocator::new(
                    "private-json-rpc-call-and-options",
                )),
            },
            appended_at: "2026-08-26T08:00:00Z".into(),
        };

        let payload = event_payload(&event);
        assert!(payload["event"].get("locator").is_none());
        assert!(!payload.to_string().contains("private-json-rpc"));
    }
}
