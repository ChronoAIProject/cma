//! Reading a session's history, and following it, in AG-UI's own vocabulary.
//!
//! §7 of the document tells a client that a dropped stream is recovered by
//! reading the durable log rather than by re-posting the run — which would
//! start a second turn. It then sent them to a REST route, which answers in
//! the core's vocabulary and not this one. A client that followed the
//! instruction received frames it could not read, and drew nothing.
//!
//! So the replay lives here, beside the run it recovers, and projects through
//! exactly the same `project` the run stream uses. One projection, two ways in:
//! a client that reconnects sees the frames it would have seen live, which is
//! the equivalence §18.1 of `Web_Style.md` asks the surface to preserve.
//!
//! An interface projects the core in its own idiom (`CLAUDE.md` §3). That is
//! why this is not a mode on the REST route: REST emitting AG-UI frames would
//! be one interface wearing another's vocabulary, and a client would have no
//! way to know which it was being handed.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Query, State},
    response::sse::{Event as SseEvent, Sse},
};
use futures_util::stream::{self, Stream};
use layer2_core::SessionId;
use serde::Deserialize;

use super::project::project;
use super::SessionsState;
use crate::auth::AuthenticatedCaller;
use crate::rest::Problem;

/// How often the log is re-read while a client is following it.
///
/// The same cadence the run stream polls at, because it is the same log and a
/// client should not see one lag the other.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Where a client is resuming from, and which conversation it wants.
#[derive(Debug, Deserialize)]
pub(super) struct Replay {
    /// The session, in this protocol's word for one.
    thread_id: String,
    /// Resume after this sequence. Absent means from the beginning.
    ///
    /// Named as the client sends it. The name is part of the contract and is
    /// documented; a server reading a different one silently replays
    /// everything on every reconnect, which is what happened here.
    #[serde(default, alias = "after")]
    starting_after: Option<u64>,
}

impl Replay {
    /// Where this client is resuming from.
    ///
    /// Read through a method so a test asserts the parsed query rather than
    /// the struct's shape: the bug this guards was a name the server never
    /// read, and a test that constructs the struct directly cannot see one.
    pub(super) fn starting_after(&self) -> Option<u64> {
        self.starting_after
    }
}

/// Read a session's history and keep following it.
///
/// Unlike a run, this never ends on its own: it is the session's stream rather
/// than one turn's, so a client holds it open across turns and the run
/// lifecycle events arrive on it like any other.
pub(super) async fn replay(
    State(state): State<SessionsState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Query(query): Query<Replay>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, Problem> {
    let seq = query.starting_after().unwrap_or(0);
    let id = SessionId(query.thread_id);

    // Read once before answering, so a caller who may not see this session is
    // refused with a status rather than handed an empty stream that never says
    // why.
    let first = state
        .sessions
        .events_since(&caller, &id, Some(seq))
        .await
        .map_err(Problem)?;

    struct Following {
        service: Arc<layer2_core::SessionService>,
        caller: layer2_core::Caller,
        id: SessionId,
        seq: u64,
        pending: std::collections::VecDeque<super::project::AguiEvent>,
    }

    let mut opening = Following {
        service: Arc::clone(&state.sessions),
        caller,
        id: id.clone(),
        seq,
        pending: Default::default(),
    };
    for event in first {
        opening.seq = event.seq;
        // The run identifier a replayed frame carries is the session's own.
        // A client reconnecting has no run in flight to attribute history to,
        // and inventing one would make the same turn read differently
        // depending on how it was reached.
        opening.pending.extend(project(&event, &id.0, &id.0).events);
    }

    Ok(Sse::new(stream::unfold(opening, |mut open| async move {
        loop {
            if let Some(event) = open.pending.pop_front() {
                let frame = SseEvent::default()
                    .id(event.seq.to_string())
                    .event(event.kind)
                    .json_data(&event)
                    .unwrap_or_else(|_| SseEvent::default().event("RUN_ERROR"));
                return Some((Ok(frame), open));
            }
            tokio::time::sleep(POLL_INTERVAL).await;
            let read = open
                .service
                .events_since(&open.caller, &open.id, Some(open.seq))
                .await;
            let Ok(events) = read else {
                // Gone, or no longer readable by this caller. Ending the
                // stream says so; polling on would keep asking about
                // something they may no longer be entitled to.
                return None;
            };
            for event in events {
                open.seq = event.seq;
                let id = open.id.0.clone();
                open.pending.extend(project(&event, &id, &id).events);
            }
        }
    })))
}

#[cfg(test)]
#[path = "replay_tests.rs"]
mod replay_tests;
