use crate::stream_telemetry::{self, Kind, Poll};
use std::{collections::VecDeque, convert::Infallible, time::Duration};

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{sse::KeepAlive, IntoResponse, Response, Sse},
};
use futures_util::stream;
use layer2_core::{
    sandbox::{
        Agent, AgentEventStreamAuthorization, AgentLiveFragment, AgentLiveFragmentKind,
        AgentLiveSubscription, AgentLogEntry, AgentLogKind, AgentStatus, PendingAgentApprovals,
    },
    AgentId, Caller,
};
use serde::Deserialize;

#[cfg(test)]
#[path = "replay_tests.rs"]
mod tests;

#[path = "replay_authorized.rs"]
mod authorized;

use super::events::{ending, project, AguiEvent};
use crate::{
    conversation::{failure, read_failure},
    AgentConversationState, AuthenticatedCaller, AuthenticatedResourceCaller,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReplayQuery {
    thread_id: String,
    #[serde(default, alias = "after")]
    starting_after: Option<u64>,
    follow: Option<bool>,
}

#[utoipa::path(get, path = "/api/v1/agui/events", tag = "ag-ui",
    params(("thread_id" = String, Query), ("starting_after" = Option<u64>, Query),
    ("after" = Option<u64>, Query), ("follow" = Option<bool>, Query)),
    responses((status = 200, body = String, content_type = "text/event-stream"),
    (status = 400, body = crate::rest::ProblemDocument), (status = 404, body = crate::rest::ProblemDocument)))]
pub(super) async fn replay(
    State(state): State<AgentConversationState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    resource: Option<AuthenticatedResourceCaller>,
    query: Result<Query<ReplayQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(query) => query,
        Err(_) => {
            return failure(
                StatusCode::BAD_REQUEST,
                "invalid_replay_query",
                "The replay query is invalid.",
            )
        }
    };
    let id = AgentId(query.thread_id);
    let watching = state.watch(&id);
    if let Some(resource) = resource {
        return authorized::open(
            state,
            resource,
            id,
            query.starting_after.unwrap_or(0),
            query.follow.unwrap_or(true),
            None,
            watching,
        )
        .await;
    }
    open(
        state,
        caller,
        id,
        query.starting_after.unwrap_or(0),
        query.follow.unwrap_or(true),
        None,
        watching,
    )
    .await
}

pub(super) async fn open_run(
    state: AgentConversationState,
    resource: AuthenticatedResourceCaller,
    id: AgentId,
    after: u64,
    run: u64,
    watching: Option<AgentLiveSubscription>,
) -> Response {
    authorized::open(state, resource, id, after, true, Some(run), watching).await
}

pub(super) async fn open(
    state: AgentConversationState,
    caller: Caller,
    id: AgentId,
    after: u64,
    follow: bool,
    run: Option<u64>,
    watching: Option<AgentLiveSubscription>,
) -> Response {
    let (agent, history) = match state.reads.log_snapshot(&caller, &id).await {
        Ok(snapshot) => snapshot,
        Err(error) => return read_failure(error, "thread_not_found"),
    };
    if after > agent.conversation.last_seq {
        let mut response = crate::rest::problem_document(
            StatusCode::BAD_REQUEST,
            "cursor-beyond-end",
            "The replay cursor exceeds this Agent's durable log head.",
            "cursor_beyond_end",
        );
        response["head"] = agent.conversation.last_seq.into();
        return crate::rest::problem_response(StatusCode::BAD_REQUEST, response);
    }
    let mut open = Following {
        state,
        caller,
        id,
        after,
        turn: 0,
        pending: VecDeque::new(),
        follow,
        run,
        ended: false,
        watching,
        committed_message_seq: 0,
        held: None,
        authorization: None,
    };
    for entry in &history {
        if entry.seq > after {
            break;
        }
        if let AgentLogKind::TurnStarted { turn_sequence, .. } = entry.kind {
            open.turn = turn_sequence;
        }
    }
    if let Some(turn) = run {
        open.turn = turn;
        if after >= turn {
            open.pending.push_back(AguiEvent {
                kind: "RUN_STARTED",
                thread_id: open.id.to_string(),
                run_id: turn.to_string(),
                body: serde_json::json!({}),
                timestamp: None,
                seq: None,
                live: false,
            });
        }
    }
    open.committed_message_seq = history
        .iter()
        .rev()
        .find(|entry| matches!(entry.kind, AgentLogKind::AgentMessage { .. }))
        .map_or(0, |entry| entry.seq);
    open.queue(&history);
    open.interrupt_pending(&agent, &history);
    open.queue_live(&agent);
    response(open)
}

fn response(open: Following) -> Response {
    let stream = stream::unfold(open, |mut open| async move {
        loop {
            if let Some(event) = open.pending.pop_front() {
                if let Some(authorization) = &open.authorization {
                    if open
                        .state
                        .reads
                        .validate_event_stream(authorization)
                        .await
                        .is_err()
                    {
                        return None;
                    }
                }
                return Some((Ok::<_, Infallible>(event.sse()), open));
            }
            if open.ended || !open.follow {
                return None;
            }
            let wake = tokio::time::Instant::now() + Duration::from_millis(100);
            let wake = open
                .authorization
                .as_ref()
                .and_then(AgentEventStreamAuthorization::revalidation_deadline)
                .map_or(wake, |deadline| deadline.min(wake));
            tokio::time::sleep_until(wake).await;
            let poll = Poll::new(Kind::Agui);
            if let Some(authorization) = &open.authorization {
                match open.state.reads.event_stream_snapshot(authorization).await {
                    Ok(snapshot) => open.queue_authorized(snapshot),
                    Err(_) => return None,
                }
                poll.complete(!open.pending.is_empty());
                continue;
            }
            match open.state.reads.log_snapshot(&open.caller, &open.id).await {
                Ok((agent, history)) => {
                    open.queue(&history);
                    open.interrupt_pending(&agent, &history);
                    open.queue_live(&agent);
                }
                Err(_) => return None,
            }
            poll.complete(!open.pending.is_empty());
        }
    });
    let mut response = Sse::new(stream_telemetry::observe(stream, Kind::Agui))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-cache, no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    response
}

struct Following {
    state: AgentConversationState,
    caller: Caller,
    id: AgentId,
    after: u64,
    turn: u64,
    pending: VecDeque<AguiEvent>,
    follow: bool,
    run: Option<u64>,
    ended: bool,
    watching: Option<AgentLiveSubscription>,
    committed_message_seq: u64,
    held: Option<AgentLiveFragment>,
    authorization: Option<AgentEventStreamAuthorization>,
}

impl Following {
    fn queue(&mut self, history: &[AgentLogEntry]) {
        for entry in history {
            if entry.seq <= self.after {
                continue;
            }
            if let AgentLogKind::TurnStarted { turn_sequence, .. } = entry.kind {
                if self.run.is_some_and(|run| run != turn_sequence) {
                    self.ended = true;
                    break;
                }
                self.turn = turn_sequence;
            }
            self.after = entry.seq;
            if matches!(entry.kind, AgentLogKind::AgentMessage { .. }) {
                self.committed_message_seq = entry.seq;
            }
            self.pending.extend(project(entry, self.turn));
            if self.run.is_some() && ending(&entry.kind) {
                self.ended = true;
                break;
            }
        }
    }

    fn interrupt_pending(&mut self, agent: &Agent, history: &[AgentLogEntry]) {
        if self.ended
            || self.run.is_none()
            || self.run != agent.conversation.active_turn
            || agent.status != AgentStatus::WaitingApproval
        {
            return;
        }
        self.interrupt_current(agent.conversation.active_turn, history);
    }

    fn interrupt_current(&mut self, active_turn: Option<u64>, history: &[AgentLogEntry]) {
        if self.ended || self.run.is_none() || self.run != active_turn {
            return;
        }
        let mut approvals = PendingAgentApprovals::default();
        for entry in history {
            approvals.observe(entry);
        }
        let mut interrupts = Vec::new();
        for entry in approvals.into_entries() {
            for mut event in project(&entry, self.turn) {
                // These are current pending state, not a replay of older cursor positions.
                event.seq = None;
                if event.kind == "RUN_FINISHED" {
                    if let Some(values) = event.body["outcome"]["interrupts"].as_array() {
                        interrupts.extend(values.iter().cloned());
                    }
                } else {
                    self.pending.push_back(event);
                }
            }
        }
        if !interrupts.is_empty() {
            // Complete this committed-state projection at the current head, never an old request.
            if let Some(event) = self
                .pending
                .iter_mut()
                .rev()
                .find(|event| event.seq == Some(self.after))
            {
                event.seq = None;
            }
            self.pending.push_back(AguiEvent {
                kind: "RUN_FINISHED",
                thread_id: self.id.to_string(),
                run_id: self.turn.to_string(),
                body: serde_json::json!({"outcome": {"type": "interrupt", "interrupts": interrupts}}),
                timestamp: None,
                seq: Some(self.after),
                live: false,
            });
            self.ended = true;
        }
    }

    fn queue_live(&mut self, agent: &Agent) {
        self.queue_fragments(
            &agent.id,
            agent.conversation.runtime_generation,
            agent.conversation.active_turn,
            agent.conversation.last_seq,
        );
    }

    fn queue_fragments(
        &mut self,
        id: &AgentId,
        generation: Option<u64>,
        turn: Option<u64>,
        head: u64,
    ) {
        if self.ended || !self.follow {
            return;
        }
        let Some(watching) = self.watching.as_mut() else {
            return;
        };
        for _ in 0..32 {
            match self
                .held
                .take()
                .map(Ok)
                .unwrap_or_else(|| watching.try_recv())
            {
                Ok(fragment)
                    if &fragment.agent_id == id
                        && Some(fragment.runtime_generation) == generation
                        && Some(fragment.turn_sequence) == turn
                        && (fragment.kind == AgentLiveFragmentKind::Thought
                            || fragment.after_seq >= self.committed_message_seq) =>
                {
                    if fragment.after_seq > head {
                        self.held = Some(fragment);
                        break;
                    }
                    self.pending.push_back(super::live::project(fragment));
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(frames)) => {
                    self.pending.push_back(AguiEvent { kind: "STATE_DELTA", thread_id: self.id.to_string(), run_id: self.turn.to_string(),
                        body: serde_json::json!({"delta": [{"op": "replace", "path": "/conversationGap", "value": {"frames": frames}}]}),
                        timestamp: None, seq: None, live: true });
                }
                Err(_) => break,
            }
        }
    }
}
