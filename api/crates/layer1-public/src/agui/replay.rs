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

use axum::{
    extract::{Query, State},
    response::sse::{Event as SseEvent, KeepAlive, Sse},
};
use futures_util::stream::{self, Stream};
use layer2_core::SessionId;
use serde::Deserialize;

use super::project::project;
use super::resources::{next_frame, project_reading, Woke, POLL_INTERVAL};
use super::SessionsState;
use crate::auth::AuthenticatedCaller;
use crate::rest::Problem;

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
    /// Whether to keep the stream open once the log has been drained.
    ///
    /// Absent means yes, because that is what a client watching a session
    /// wants and what every existing caller expects.
    ///
    /// `follow=false` makes this a **bounded** read that ends when it reaches
    /// the end of the log, and a caller that needs history as a value rather
    /// than as a subscription has no other way to get it: reading an endless
    /// stream to completion never completes. The reference client did exactly
    /// that — `await response.text()` against this route — so its history read
    /// could only ever settle by failing, and everything downstream of it,
    /// including every live frame, waited behind a promise that never
    /// resolved.
    #[serde(default)]
    follow: Option<bool>,
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

    /// Whether this client wants the stream held open past the end of the log.
    pub(super) fn follows(&self) -> bool {
        self.follow.unwrap_or(true)
    }
}

/// Read a session's history, and keep following it unless asked not to.
///
/// Followed, this never ends on its own: it is the session's stream rather
/// than one turn's, so a client holds it open across turns and the run
/// lifecycle events arrive on it like any other.
pub(super) async fn replay(
    State(state): State<SessionsState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Query(query): Query<Replay>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, Problem> {
    let seq = query.starting_after().unwrap_or(0);
    let follows = query.follows();
    let id = SessionId(query.thread_id);

    // Read once before answering, so a caller who may not see this session is
    // refused with a status rather than handed an empty stream that never says
    // why.
    let first = state
        .sessions
        .events_since(&caller, &id, Some(seq))
        .await
        .map_err(Problem)?;

    // The live lane, on the session's own stream.
    //
    // This is the stream a client holds open across turns, so it is where what
    // is happening belongs. Offering liveness only on the run stream would
    // require a client to hold two connections and decide for itself how to
    // interleave them — which is exactly the ordering question §18.1 of
    // `Web_Style.md` says must have one answer — and a client that posts a run
    // without consuming its response would never see any of it.
    //
    // Absent for a session with no conversation, or none running. The durable
    // lane is unaffected either way: that is the difference between a turn
    // nobody can watch and a turn nobody can read.
    let watching = state.sessions.subscribe_live(&caller, &id).await.ok();
    let resources = state.sessions.subscribe_resources(&caller, &id).await.ok();

    struct Following {
        service: Arc<layer2_core::SessionService>,
        caller: layer2_core::Caller,
        id: SessionId,
        seq: u64,
        pending: std::collections::VecDeque<super::project::AguiEvent>,
        watching: Option<tokio::sync::broadcast::Receiver<layer2_core::LiveFrame>>,
        resources: Option<tokio::sync::broadcast::Receiver<layer2_core::ResourceReading>>,
        /// Polls since the last attempt to attach to the live lane.
        ///
        /// A client opens this stream as soon as it has a session, which is
        /// routinely before that session is running — so the first attempt
        /// fails and, without this, the client would never see a live frame
        /// however long it stayed connected. Retried on a much slower cadence
        /// than the log is polled, because attaching costs a read and the
        /// window it covers is a session starting rather than anything urgent.
        since_attempt: u8,
        /// Whether to hold the stream open once the log has been drained.
        follows: bool,
    }

    /// Poll cycles between attempts to attach to a session's live lane.
    const ATTACH_EVERY: u8 = 8;

    let mut opening = Following {
        service: Arc::clone(&state.sessions),
        caller,
        id: id.clone(),
        seq,
        pending: Default::default(),
        watching,
        resources,
        since_attempt: 0,
        follows,
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
                let frame = super::project::sse_frame(&event);
                return Some((Ok(frame), open));
            }
            // Everything the log held has been sent. A bounded read is done,
            // and ending it is what lets a caller await this route as a value
            // rather than as a subscription.
            if !open.follows {
                return None;
            }
            // Whichever lane speaks first. Sleeping the poll interval
            // unconditionally would hold a piece of a message for up to that
            // long, which on a fast agent is most of what makes the difference
            // between watching a turn and waiting on one.
            let run = open.id.0.clone();
            let woke = tokio::time::timeout(
                POLL_INTERVAL,
                next_frame(&mut open.watching, &mut open.resources),
            )
            .await
            .unwrap_or(Woke::Idle);
            match woke {
                Woke::Live(frame) => open
                    .pending
                    .extend(super::live::project_live(&frame, &run, &run)),
                Woke::Reading(reading) => {
                    open.pending
                        .push_back(project_reading(&reading, &run, &run));
                }
                Woke::Idle => {}
            }

            if open.watching.is_none() || open.resources.is_none() {
                open.since_attempt = open.since_attempt.saturating_add(1);
                if open.since_attempt >= ATTACH_EVERY {
                    open.since_attempt = 0;
                    if open.watching.is_none() {
                        open.watching = open
                            .service
                            .subscribe_live(&open.caller, &open.id)
                            .await
                            .ok();
                    }
                    if open.resources.is_none() {
                        open.resources = open
                            .service
                            .subscribe_resources(&open.caller, &open.id)
                            .await
                            .ok();
                    }
                }
            } else {
                open.since_attempt = 0;
            }
            if !open.pending.is_empty() {
                continue;
            }
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
    }))
    // The run stream has always carried one and this did not, so an idle
    // session's stream was torn down by an intermediary after roughly two
    // minutes and the surface silently stopped updating. This is the stream a
    // client holds open across turns, so idle is its normal state.
    .keep_alive(KeepAlive::default()))
}

#[cfg(test)]
#[path = "replay_tests.rs"]
mod replay_tests;
