use super::*;
use crate::DelegatedCredential;
use layer2_core::sandbox::AgentEventStreamSnapshot;

pub(in crate::agui::replay) async fn open(
    state: AgentConversationState,
    resource: AuthenticatedResourceCaller,
    id: AgentId,
    after: u64,
    follow: bool,
    run: Option<u64>,
    watching: Option<AgentLiveSubscription>,
) -> Response {
    let authorization = match state
        .reads
        .authorize_event_stream(
            &resource.context,
            resource.credential.as_ref().map(DelegatedCredential::value),
            &id,
        )
        .await
    {
        Ok(value) => value,
        Err(error) => return read_failure(error, "thread_not_found"),
    };
    let snapshot = match state.reads.event_stream_snapshot(&authorization).await {
        Ok(value) => value,
        Err(error) => return read_failure(error, "thread_not_found"),
    };
    if after > snapshot.head {
        let mut problem = crate::rest::problem_document(
            StatusCode::BAD_REQUEST,
            "cursor-beyond-end",
            "The replay cursor exceeds this Agent's durable log head.",
            "cursor_beyond_end",
        );
        problem["head"] = snapshot.head.into();
        return crate::rest::problem_response(StatusCode::BAD_REQUEST, problem);
    }
    if run.is_some_and(|turn| !snapshot.history.iter().any(|entry| entry.seq == turn
        && matches!(entry.kind, Some(AgentLogKind::TurnStarted { turn_sequence, .. }) if turn_sequence == turn))) {
        return crate::rest::instance_problem::InstanceProblem::Unavailable.into_response();
    }
    let mut open = Following {
        state,
        caller: resource.context.caller,
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
        authorization: Some(authorization),
    };
    for entry in snapshot
        .history
        .iter()
        .take_while(|entry| entry.seq <= after)
    {
        if let Some(AgentLogKind::TurnStarted { turn_sequence, .. }) = entry.kind {
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
    open.committed_message_seq = snapshot
        .history
        .iter()
        .rev()
        .find(|entry| matches!(entry.kind, Some(AgentLogKind::AgentMessage { .. })))
        .map_or(0, |entry| entry.seq);
    open.queue_authorized(snapshot);
    response(open)
}

impl Following {
    pub(super) fn queue_authorized(&mut self, snapshot: AgentEventStreamSnapshot) {
        let retained: Vec<_> = snapshot
            .history
            .iter()
            .filter_map(|entry| {
                Some(AgentLogEntry {
                    agent_id: snapshot.id.clone(),
                    seq: entry.seq,
                    appended_at: entry.appended_at.clone(),
                    kind: entry.kind.clone()?,
                })
            })
            .collect();
        for entry in snapshot.history {
            if self.ended {
                break;
            }
            if entry.seq <= self.after {
                continue;
            }
            if let Some(kind) = entry.kind {
                self.queue(&[AgentLogEntry {
                    agent_id: snapshot.id.clone(),
                    seq: entry.seq,
                    appended_at: entry.appended_at,
                    kind,
                }]);
            } else {
                self.after = entry.seq;
                // Empty state patches carry only a durable cursor. They preserve
                // resume through withheld events without inventing public state.
                self.pending.push_back(AguiEvent {
                    kind: "STATE_DELTA",
                    thread_id: snapshot.id.to_string(),
                    run_id: self.turn.to_string(),
                    body: serde_json::json!({"delta": []}),
                    timestamp: Some(entry.appended_at),
                    seq: Some(entry.seq),
                    live: false,
                });
            }
        }
        self.interrupt_current(snapshot.active_turn, &retained);
        self.queue_fragments(
            &snapshot.id,
            snapshot.runtime_generation,
            snapshot.active_turn,
            snapshot.head,
        );
    }
}
