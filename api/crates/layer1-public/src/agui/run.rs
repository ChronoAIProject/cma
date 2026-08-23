//! Running a turn, and streaming what happens.
//!
//! One request per turn: the response is the turn, and it ends when the turn
//! does. What is streamed is the durable log projected into this protocol's
//! vocabulary, which is what makes a dropped connection recoverable — the
//! events carry the sequence every other interface cursors on, and a client
//! resumes by reading the log rather than by asking for the turn again.
//!
//! ## The thread is the session
//!
//! `threadId` names a session, and one that does not exist is created. That is
//! how this protocol projects session creation: there is no separate call, and
//! inventing one would put an operation in this interface that the core does
//! not have.
//!
//! ## Resuming is a new run
//!
//! An approval ends the run and the answer starts another. §4 of
//! `docs/API_AGUI.md` explains why at length; the short version is that the
//! asking is durable, so a client that closed the tab finds it waiting.
//!
//! ## A dropped stream cancels nothing
//!
//! The stream ending means somebody stopped watching, and nothing more. A
//! connection dropped because a laptop slept is indistinguishable from one
//! closed on purpose, and reading the first as a cancellation would end
//! somebody's turn because their network blinked — which §5 forbids in the
//! strongest terms it has. The turn carries on, its events land in the log,
//! and a client that comes back finds them. Interrupting is an act and has a
//! route of its own.

use std::sync::Arc;

use axum::{
    extract::State,
    response::sse::{Event as SseEvent, KeepAlive, Sse},
    Json,
};
use futures_util::stream::{self, Stream};
use layer2_core::{
    ApprovalDecision, CoreError, SessionCapability, SessionId, SessionService, SessionSpec,
};
use serde::Deserialize;

use crate::auth::{AuthenticatedCaller, DelegatedCredential};

use super::project::project;
use super::resources::{next_frame, project_reading, Woke, POLL_INTERVAL};
use super::Problem;

/// One run, as a client posts it.
#[derive(Debug, Deserialize)]
pub(super) struct RunRequest {
    /// The session. One that does not exist is created.
    #[serde(rename = "threadId")]
    thread_id: String,
    /// The client's identifier for this turn, echoed on every event.
    #[serde(rename = "runId")]
    run_id: String,
    /// The conversation. The last user message is this turn's input.
    #[serde(default)]
    messages: Vec<RunMessage>,
    /// Answers to what a previous run was interrupted to ask.
    #[serde(default)]
    resume: Vec<ResumeItem>,
    /// Session creation parameters, honoured only when the thread is new.
    #[serde(rename = "forwardedProps", default)]
    forwarded_props: Option<ForwardedProps>,
}

/// One message in the conversation a client sent.
#[derive(Debug, Deserialize)]
struct RunMessage {
    role: String,
    #[serde(default)]
    content: Option<String>,
}

/// An answer to an interrupt.
#[derive(Debug, Deserialize)]
struct ResumeItem {
    #[serde(rename = "interruptId")]
    interrupt_id: String,
    /// `resolved` carries a decision; `cancelled` is a refusal.
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    payload: Option<ResumePayload>,
}

#[derive(Debug, Deserialize)]
struct ResumePayload {
    #[serde(default)]
    decision: Option<String>,
}

/// What a new thread is created with.
///
/// Ignored on an existing one, because these are frozen at creation: changing
/// the kernel a session runs on is a different session, and pretending
/// otherwise would silently ignore what a client asked for.
///
/// A kernel is not among them, and `kernel` is accepted only so that a client
/// sending one is not answered with a deserialization error. Which kernel a
/// session runs on is decided by the profile it names, or by what this
/// deployment configured — never by an interface, and never by a caller.
#[derive(Debug, Deserialize)]
struct ForwardedProps {
    #[serde(default)]
    #[allow(dead_code)]
    kernel: Option<String>,
    #[serde(rename = "environmentProfile", default)]
    environment_profile: Option<String>,
}

/// Open a run: prompt or resume, then stream until the turn ends.
pub(super) async fn run(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    credential: Option<axum::Extension<DelegatedCredential>>,
    Json(body): Json<RunRequest>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, std::convert::Infallible>>>, Problem> {
    let requested = SessionId(body.thread_id.clone());
    let session = match service.get_session(&caller, &requested).await {
        Ok(session) => session,
        // A thread that does not exist is created, which is how this protocol
        // projects session creation. Anything else — a session belonging to
        // somebody else, a store that cannot be reached — is not a missing
        // thread and must not be answered by making one.
        Err(CoreError::SessionNotFound) => {
            let props = body.forwarded_props.as_ref();
            service
                .create_session(
                    &caller,
                    SessionSpec {
                        environment_profile: props
                            .and_then(|p| p.environment_profile.clone())
                            .map(layer2_core::EnvironmentProfileId),
                        geometry: None,
                        options: layer2_core::AgentOptions {
                            mode: layer2_core::SessionMode::Conversation,
                            ..Default::default()
                        },
                        // Said explicitly, because `options` is filled in from
                        // the profile or the deployment and what is set above
                        // is discarded. This interface holds conversations and
                        // nothing else, so a thread it creates has to be one —
                        // and without this it created a terminal session on a
                        // deployment defaulting to terminal, then refused it
                        // for lacking the ability it had just asked for.
                        requested_mode: Some(layer2_core::SessionMode::Conversation),
                        quoted_rate_card: None,
                        // The caller's own token, for asking the platform as them. It
                        // is never what the session runs on: the core mints that.
                        caller_credential: credential.map(|axum::Extension(c)| c.0),
                        runtime_credential: None,
                    },
                )
                .await?
        }
        Err(e) => return Err(e.into()),
    };

    // The session's own identifier from here on, which is not always the one
    // that was asked for: a thread that did not exist is created, and the core
    // mints the id rather than taking one from a request. Everything below —
    // the prompt, the cursor, the events — belongs to the session that exists,
    // and the events carry its id so a client learns what its thread is called.
    let id = session.id.clone();

    // Refused before anything is sent. A terminal session has no conversation
    // to run, and a stream that opened and then said nothing would look like
    // an agent thinking.
    session
        .capabilities
        .require(SessionCapability::Conversation)?;

    // Read before the turn is opened, so the stream starts exactly where the
    // client's own actions begin and replays nothing it has already seen.
    let from = service
        .events_since(&caller, &id, None)
        .await?
        .last()
        .map(|e| e.seq)
        .unwrap_or(0);

    match body.resume.is_empty() {
        false => {
            for item in &body.resume {
                service
                    .resolve_approval(&caller, &id, item.interrupt_id.clone(), decision_of(item))
                    .await?;
            }
        }
        true => {
            let prompt = body
                .messages
                .iter()
                .rev()
                .find(|m| m.role == "user")
                .and_then(|m| m.content.clone())
                .ok_or_else(|| {
                    CoreError::InvalidRequest("a run needs a user message to send".to_string())
                })?;
            service.send_prompt(&caller, &id, prompt).await?;
        }
    }

    // Subscribed after the prompt is sent, so the turn is already underway and
    // the receiver exists before the runtime has anything to say. A session
    // whose kernel offers no liveness — or whose entry has gone — simply has
    // none, and the durable stream is unaffected: that is the difference
    // between a turn nobody can watch and a turn nobody can read.
    let watching = service.subscribe_live(&caller, &id).await.ok();
    let resources = service.subscribe_resources(&caller, &id).await.ok();

    Ok(Sse::new(events(
        service,
        caller,
        id,
        body.run_id,
        from,
        watching,
        resources,
    ))
    .keep_alive(KeepAlive::default()))
}

/// What the owner decided, read from the protocol's own vocabulary.
///
/// `cancelled` is a refusal — the document says so — and an unfamiliar status
/// is read the same way. Defaulting to allow would grant on a word this build
/// does not understand.
fn decision_of(item: &ResumeItem) -> ApprovalDecision {
    if item.status.as_deref() != Some("resolved") {
        return ApprovalDecision::Deny;
    }
    match item.payload.as_ref().and_then(|p| p.decision.as_deref()) {
        Some("allow") => ApprovalDecision::Allow,
        Some("abort") => ApprovalDecision::Abort,
        _ => ApprovalDecision::Deny,
    }
}

/// The run's event stream, from `after` until the turn ends.
///
/// Three lanes on one stream. The durable one is polled from the log and is
/// what a client resumes on; conversation liveness and resource readings each
/// have their own receiver and carry no cursor at all. They are merged here
/// rather than offered as separate connections because they describe one turn,
/// and a client holding several would have to decide how to interleave them.
///
/// The durable lane is drained first on every pass. A committed message must
/// not arrive behind a piece of itself.
fn events(
    service: Arc<SessionService>,
    caller: layer2_core::Caller,
    id: SessionId,
    run_id: String,
    after: u64,
    watching: Option<tokio::sync::broadcast::Receiver<layer2_core::LiveFrame>>,
    resources: Option<tokio::sync::broadcast::Receiver<layer2_core::ResourceReading>>,
) -> impl Stream<Item = Result<SseEvent, std::convert::Infallible>> {
    struct Open {
        service: Arc<SessionService>,
        caller: layer2_core::Caller,
        id: SessionId,
        run_id: String,
        seq: u64,
        pending: std::collections::VecDeque<super::project::AguiEvent>,
        /// What is happening right now, when anything is watching for it.
        watching: Option<tokio::sync::broadcast::Receiver<layer2_core::LiveFrame>>,
        /// What this session is using right now, on its independent lag budget.
        resources: Option<tokio::sync::broadcast::Receiver<layer2_core::ResourceReading>>,
        /// Set once something has ended the run, so the last event is still
        /// delivered before the stream closes.
        ended: bool,
    }

    stream::unfold(
        Open {
            service,
            caller,
            id,
            run_id,
            seq: after,
            pending: Default::default(),
            watching,
            resources,
            ended: false,
        },
        |mut open| async move {
            loop {
                if let Some(event) = open.pending.pop_front() {
                    let frame = super::project::sse_frame(&event);
                    return Some((Ok(frame), open));
                }
                if open.ended {
                    return None;
                }
                // Wait for whichever lane speaks first. Sleeping the poll
                // interval unconditionally would hold a piece of a message for
                // up to that long, which on a fast agent is most of what makes
                // the difference between watching a turn and waiting on one.
                let woke = tokio::time::timeout(
                    POLL_INTERVAL,
                    next_frame(&mut open.watching, &mut open.resources),
                )
                .await
                .unwrap_or(Woke::Idle);
                match woke {
                    Woke::Live(frame) => open.pending.extend(super::live::project_live(
                        &frame,
                        &open.run_id,
                        &open.id.0,
                    )),
                    Woke::Reading(reading) => {
                        open.pending
                            .push_back(project_reading(&reading, &open.run_id, &open.id.0))
                    }
                    Woke::Idle => {}
                }
                if !open.pending.is_empty() {
                    continue;
                }
                let read = open
                    .service
                    .events_since(&open.caller, &open.id, Some(open.seq))
                    .await;
                let Ok(events) = read else {
                    // The session is gone or no longer readable. Ending the
                    // stream says so; polling on would query something this
                    // caller may no longer be entitled to.
                    return None;
                };
                for event in events {
                    open.seq = event.seq;
                    let projected = project(&event, &open.run_id, &open.id.0);
                    open.pending.extend(projected.events);
                    if let Some(ending) = projected.ending {
                        tracing::debug!(session_id = %open.id, ?ending, "run ended");
                        open.ended = true;
                    }
                }
            }
        },
    )
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod run_tests;
