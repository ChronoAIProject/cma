#[path = "replay_queue.rs"]
mod replay_queue;
use crate::stream_telemetry::{self, Kind, Poll};
use std::{collections::VecDeque, time::Duration};

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{sse::KeepAlive, IntoResponse, Response, Sse},
};
use futures_util::stream;
use layer2_core::{
    sandbox::{
        AgentEventStreamAuthorization, AgentLiveDelivery, AgentLiveFragmentKind,
        AgentLiveSubscription, AgentLogEntry, AgentLogKind, PendingAgentApprovals,
    },
    AgentId, Caller,
};
use serde::Deserialize;

#[cfg(test)]
#[path = "replay_compatibility_tests.rs"]
mod compatibility_tests;

#[cfg(test)]
#[path = "replay_bounded_tests.rs"]
mod bounded_tests;
#[cfg(test)]
#[path = "replay_live_tests.rs"]
mod live_tests;
#[cfg(test)]
#[path = "replay_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "replay_observation_tests.rs"]
mod observation_tests;

#[path = "replay_authorized.rs"]
mod authorized;

#[cfg(test)]
use layer2_core::sandbox::{Agent, AgentStatus};

use super::events::{ending, project_bounded as project, AguiEvent};

#[path = "replay_legacy.rs"]
mod legacy;
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
    let watching = match state.watch(&id).await {
        Ok(watching) => watching,
        Err(response) => return response,
    };
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
    if !state.bounded_replay {
        return legacy::open(state, caller, id, after, follow, run, watching).await;
    }
    begin(state, (caller, None), id, after, follow, run, watching).await
}

async fn begin(
    state: AgentConversationState,
    authority: (Caller, Option<AgentEventStreamAuthorization>),
    id: AgentId,
    after: u64,
    follow: bool,
    run: Option<u64>,
    watching: Option<AgentLiveSubscription>,
) -> Response {
    let (caller, mut authorization) = authority;
    if authorization.is_none() {
        authorization = match state
            .reads
            .owner_observation_authorization(&caller, &id)
            .await
        {
            Ok(value) => value,
            Err(error) => return read_failure(error, "thread_not_found"),
        };
    }
    let observation = if let Some(authorization) = &mut authorization {
        match state.reads.observe_event_stream(authorization) {
            Ok(value) => value,
            Err(error) => return read_failure(error, "thread_not_found"),
        }
    } else {
        None
    };
    let heads = state.watch_head(&id);
    let mut open = Following {
        state,
        caller,
        id,
        after,
        turn: 0,
        pending: VecDeque::new(),
        replay_queue: Default::default(),
        follow,
        run,
        ended: false,
        watching,
        committed_message_seq: 0,
        held: None,
        authorization,
        observation,
        cursor: Default::default(),
        header: None,
        through: None,
        approvals: Default::default(),
        heads,
        fallback_due: tokio::time::Instant::now() + Duration::from_secs(1),
        discarded_fragments: 0,
        observed_at: tokio::time::Instant::now(),
        observed_header: None,
    };
    if let Err(error) = open.initialize().await {
        let error = match error {
            following::OpenError::UnknownRun => {
                return crate::rest::instance_problem::InstanceProblem::Unavailable.into_response();
            }
            following::OpenError::Read(error) => error,
        };
        if matches!(error, layer2_core::sandbox::AgentReadError::InvalidPage) {
            if let Some(header) = &open.header {
                if after > header.head {
                    let mut problem = crate::rest::problem_document(
                        StatusCode::BAD_REQUEST,
                        "cursor-beyond-end",
                        "The replay cursor exceeds this Agent's durable log head.",
                        "cursor_beyond_end",
                    );
                    problem["head"] = header.head.into();
                    return crate::rest::problem_response(StatusCode::BAD_REQUEST, problem);
                }
            }
        }
        return read_failure(error, "thread_not_found");
    }
    response(open)
}

#[path = "replay_follow.rs"]
mod following;
use following::response;

struct Following {
    state: AgentConversationState,
    caller: Caller,
    id: AgentId,
    after: u64,
    turn: u64,
    pending: VecDeque<AguiEvent>,
    replay_queue: replay_queue::ReplayQueue,
    follow: bool,
    run: Option<u64>,
    ended: bool,
    watching: Option<AgentLiveSubscription>,
    committed_message_seq: u64,
    held: Option<AgentLiveDelivery>,
    authorization: Option<AgentEventStreamAuthorization>,
    observation: Option<layer2_core::sandbox::agent_observation::AgentObservationSubscription>,
    cursor: layer2_core::sandbox::AgentHistoryCursor,
    header: Option<layer2_core::sandbox::AgentEventStreamHeader>,
    through: Option<u64>,
    approvals: PendingAgentApprovals,
    heads: Option<layer2_core::sandbox::AgentHeadSubscription>,
    fallback_due: tokio::time::Instant,
    discarded_fragments: u64,
    observed_at: tokio::time::Instant,
    observed_header: Option<layer2_core::sandbox::AgentEventStreamHeader>,
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
            self.approvals.observe(entry);
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

    #[cfg(test)]
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

    #[cfg(test)]
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
