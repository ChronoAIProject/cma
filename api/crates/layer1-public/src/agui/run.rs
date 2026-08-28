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
    extract::{rejection::JsonRejection, State},
    response::sse::{Event as SseEvent, KeepAlive, Sse},
    Json,
};
use futures_util::stream::{self, Stream};
use layer2_core::{
    ApprovalDecision, ApprovalResolution, CoreError, Event, OperationStore, SessionId,
    SessionService,
};
use serde::Deserialize;

use crate::auth::{AuthenticatedCaller, DelegatedCredential};

use super::admission::{RunFailure, StoredFailure, StoredProblem};
use super::project::project;
use super::resources::{next_frame, project_reading, Woke, POLL_INTERVAL};
use super::thread_alias::session_for_run;
use super::turn_admission::{self, RunAction, StreamPlan};
use super::KEEPALIVE_INTERVAL;

/// One run, as a client posts it.
#[derive(Debug, Deserialize)]
pub(super) struct RunRequest {
    /// The session. One that does not exist is created.
    #[serde(rename = "threadId")]
    thread_id: String,
    /// The client's identifier for this turn, echoed on every event.
    ///
    /// AG-UI permits it to be absent or null. In either case this service
    /// mints one so the response carries a non-empty run identifier a client
    /// can use.
    #[serde(rename = "runId", default)]
    run_id: Option<String>,
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

/// A run identifier for a request that did not supply one.
fn generated_run_id() -> String {
    format!("run_{}", uuid::Uuid::new_v4().simple())
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
    State(operations): State<Arc<dyn OperationStore>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    credential: Option<axum::Extension<DelegatedCredential>>,
    body: Result<Json<RunRequest>, JsonRejection>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, std::convert::Infallible>>>, RunFailure> {
    let Json(body) = body?;

    if body
        .run_id
        .as_ref()
        .is_some_and(|run_id| run_id.trim().is_empty())
    {
        return Err(CoreError::InvalidRequest(
            "runId must contain a non-whitespace character".into(),
        )
        .into());
    }

    // Establish the complete semantic operation before looking up or creating
    // a session. In particular, a malformed fresh request must not leave a
    // provisioning session behind merely to discover that it has no prompt.
    let action = action_of(&body)?;
    let opening_service = Arc::clone(&service);
    let opening_caller = caller.clone();
    let credential = credential.map(|axum::Extension(credential)| credential);
    let opening = tokio::spawn(async move {
        prepare_run(
            opening_service,
            operations,
            opening_caller,
            credential,
            body,
            action,
        )
        .await
    });
    let prepared = match opening.await {
        Ok(result) => result?,
        Err(error) => {
            tracing::error!(%error, "owned AG-UI run opening task failed");
            return Err(RunFailure::Stored(StoredProblem::opening_task_failed()));
        }
    };

    Ok(Sse::new(events(
        service,
        caller,
        prepared.id,
        RunBoundary {
            id: prepared.run_id,
        },
        prepared.plan,
    ))
    .keep_alive(KeepAlive::new().interval(KEEPALIVE_INTERVAL)))
}

struct PreparedRun {
    id: SessionId,
    run_id: String,
    plan: StreamPlan,
}

/// Own preparation through creation and admission-task handoff.
///
/// If the HTTP request disappears while this future is running, dropping its
/// `JoinHandle` detaches it. It still resolves the creation alias and reaches
/// `turn_admission::prepare`, whose own task then owns the prompt or resume.
async fn prepare_run(
    service: Arc<SessionService>,
    operations: Arc<dyn OperationStore>,
    caller: layer2_core::Caller,
    credential: Option<DelegatedCredential>,
    body: RunRequest,
    action: RunAction,
) -> Result<PreparedRun, RunFailure> {
    let requested = SessionId(body.thread_id.clone());
    let session = session_for_run(
        Arc::clone(&service),
        operations.clone(),
        caller.clone(),
        requested,
        credential,
        body.forwarded_props
            .as_ref()
            .and_then(|props| props.environment_profile.clone()),
        action.is_resume(),
    )
    .await?;

    // The session's own identifier from here on, which is not always the one
    // that was asked for: a thread that did not exist is created, and the core
    // mints the id rather than taking one from a request. Everything below —
    // the prompt, the cursor, the events — belongs to the session that exists,
    // and the events carry its id so a client learns what its thread is called.
    let id = session.id.clone();
    let explicit_run_id = body.run_id.clone();
    let run_id = match explicit_run_id.as_ref() {
        Some(run_id) if run_id == &id.0 => {
            return Err(CoreError::InvalidRequest(
                "runId must differ from the effective threadId".into(),
            )
            .into());
        }
        Some(run_id) => run_id.clone(),
        None => loop {
            let generated = generated_run_id();
            if generated != id.0 {
                break generated;
            }
        },
    };

    let plan = turn_admission::prepare(
        Arc::clone(&service),
        operations,
        caller.clone(),
        session,
        explicit_run_id.as_deref(),
        action,
    )
    .await?;

    Ok(PreparedRun { id, run_id, plan })
}

fn action_of(body: &RunRequest) -> Result<RunAction, CoreError> {
    if !body.resume.is_empty() {
        return Ok(RunAction::Resume(
            body.resume
                .iter()
                .map(|item| ApprovalResolution::new(item.interrupt_id.clone(), decision_of(item)))
                .collect(),
        ));
    }
    let prompt = body
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .and_then(|message| message.content.clone())
        .ok_or_else(|| CoreError::InvalidRequest("a run needs a user message to send".into()))?;
    Ok(RunAction::Prompt(prompt))
}

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

/// Open an interface run before or around its durable turn boundary.
fn synthetic_started(run_id: &str, thread_id: &str) -> super::project::AguiEvent {
    super::project::AguiEvent {
        kind: "RUN_STARTED",
        run_id: run_id.to_string(),
        thread_id: thread_id.to_string(),
        body: serde_json::json!({}),
        timestamp: None,
        // This brackets the interface's run rather than projecting a log
        // record. A continuation has no second turn, while a provisioning run
        // has not admitted its first one yet. Neither may advance the cursor.
        seq: None,
        live: false,
    }
}

fn admission_error(
    run_id: &str,
    thread_id: &str,
    failure: &StoredFailure,
) -> super::project::AguiEvent {
    let (message, code, capability) = failure.problem.stream_fields();
    let mut body = serde_json::json!({
        "message": message,
        "code": code,
    });
    if let Some(capability) = capability {
        body["capability"] = capability.into();
    }
    super::project::AguiEvent {
        kind: "RUN_ERROR",
        run_id: run_id.to_string(),
        thread_id: thread_id.to_string(),
        body,
        timestamp: None,
        seq: None,
        live: false,
    }
}

/// Queue one store read, stopping after the first event that ends this run.
///
/// Every frame belonging to the ending event remains part of the response. In
/// particular, an approval request projects to an unnumbered `RUN_FINISHED`
/// followed by its numbered pending-state delta. Events later in the same store
/// result belong to a subsequent run and must not cross that complete boundary.
fn queue_durable_batch(
    events: impl IntoIterator<Item = Event>,
    pending: &mut std::collections::VecDeque<super::project::AguiEvent>,
    seq: &mut u64,
    run_id: &str,
    thread_id: &str,
) -> bool {
    for event in events {
        *seq = event.seq;
        let projected = project(&event, run_id, thread_id);
        let ending = projected.ending;
        pending.extend(projected.events);
        if let Some(ending) = ending {
            tracing::debug!(session_id = %event.session_id, ?ending, "run ended");
            return true;
        }
    }
    false
}

struct RunBoundary {
    id: String,
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
/// not arrive behind a piece of itself. A continuation boundary is queued
/// ahead of all three lanes because it brackets this run rather than the turn
/// already in progress.
fn events(
    service: Arc<SessionService>,
    caller: layer2_core::Caller,
    id: SessionId,
    run: RunBoundary,
    mut plan: StreamPlan,
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
        /// The owned prompt admission this stream observes without owning.
        admission: Option<tokio::task::JoinHandle<turn_admission::AdmissionReport>>,
        /// Set once something has ended the run, so the last event is still
        /// delivered before the stream closes.
        ended: bool,
    }

    let mut pending = std::collections::VecDeque::new();
    if plan.synthetic_start {
        pending.push_back(synthetic_started(&run.id, &id.0));
    }
    let ended = match plan.failure.take() {
        Some(failure) => {
            pending.push_back(admission_error(&run.id, &id.0, &failure));
            true
        }
        None => false,
    };

    stream::unfold(
        Open {
            service,
            caller,
            id,
            run_id: run.id,
            seq: plan.after,
            pending,
            watching: plan.watching,
            resources: plan.resources,
            admission: plan.admission,
            ended,
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
                if let Some(admission) = open.admission.take() {
                    let settled = turn_admission::settle_deferred(admission).await;
                    match settled {
                        Ok(mut plan) => {
                            open.seq = plan.after;
                            open.watching = plan.watching.take();
                            open.resources = plan.resources.take();
                            if let Some(failure) = plan.failure.take() {
                                open.pending.push_back(admission_error(
                                    &open.run_id,
                                    &open.id.0,
                                    &failure,
                                ));
                                open.ended = true;
                            }
                        }
                        Err(_) => {
                            let failure = StoredFailure {
                                problem: StoredProblem::task_failed(),
                                streamed: true,
                            };
                            open.pending.push_back(admission_error(
                                &open.run_id,
                                &open.id.0,
                                &failure,
                            ));
                            open.ended = true;
                        }
                    }
                    continue;
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
                // Hold the cursorless lane until the log has been drained. A
                // runtime can answer immediately after accepting a prompt; if
                // that live piece were queued first, the stream would still
                // violate RUN_STARTED-first even though TurnStarted was already
                // durable. The same ordering keeps a committed message ahead of
                // the live piece it supersedes.
                let cursorless = match woke {
                    Woke::Live(frame) => {
                        super::live::project_live(&frame, &open.run_id, &open.id.0)
                    }
                    Woke::Reading(reading) => {
                        Some(project_reading(&reading, &open.run_id, &open.id.0))
                    }
                    Woke::Idle => None,
                };
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
                open.ended = queue_durable_batch(
                    events,
                    &mut open.pending,
                    &mut open.seq,
                    &open.run_id,
                    &open.id.0,
                );
                // RUN_FINISHED/RUN_ERROR must remain the last frame. A
                // cursorless reading that happened alongside the durable ending
                // belongs before it in time but cannot safely be reordered past
                // the run boundary, so the durable outcome wins.
                if !open.ended {
                    open.pending.extend(cursorless);
                }
            }
        },
    )
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod run_tests;
