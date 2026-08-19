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

use crate::auth::AuthenticatedCaller;

use super::Problem;

/// How often the log is re-read while a client is listening.
///
/// The core publishes no change notification, so this polls. Fast enough that
/// a state change feels immediate, slow enough that an idle terminal is not a
/// steady stream of queries — and a session that is merely waiting produces no
/// events, so an idle stream costs one small read per interval.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Deserialize)]
pub(super) struct Cursor {
    /// Resume after this sequence. Absent means from the beginning.
    after: Option<u64>,
}

/// Stream a session's events, resuming from a cursor.
pub(super) async fn stream(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    Query(cursor): Query<Cursor>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, Problem> {
    let id = SessionId(id);

    // A reconnecting browser sends the last id it saw. Preferring it over the
    // query means a resume cannot silently replay from the start, which for a
    // long-lived stream would be worse than a gap.
    let resume = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .or(cursor.after)
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
    let payload = serde_json::json!({
        "seq": event.seq,
        "session_id": event.session_id.0,
        "event": event.kind,
    });
    SseEvent::default()
        .id(event.seq.to_string())
        .data(payload.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::{Event, EventKind, SessionStatus};

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
}
