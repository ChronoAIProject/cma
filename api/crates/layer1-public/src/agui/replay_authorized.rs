use super::*;
use crate::DelegatedCredential;
use layer2_core::sandbox::AgentEventStreamSnapshot;

pub(super) async fn open(
    state: AgentConversationState,
    resource: AuthenticatedResourceCaller,
    id: AgentId,
    after: u64,
    follow: bool,
    run: Option<u64>,
    watching: Option<AgentLiveSubscription>,
) -> Response {
    if !state.bounded_replay {
        return legacy::authorized::open(state, resource, id, after, follow, run, watching).await;
    }
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
    begin(
        state,
        (resource.context.caller, Some(authorization)),
        id,
        after,
        follow,
        run,
        watching,
    )
    .await
}

impl Following {
    pub(super) fn queue_authorized(&mut self, snapshot: AgentEventStreamSnapshot) {
        if self.observation.is_some() {
            self.replay_queue.sources.extend(
                snapshot
                    .history
                    .into_iter()
                    .filter(|entry| entry.seq > self.after),
            );
            return;
        }
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
                replay_queue::retain_output_discovery(
                    &mut self.pending,
                    entry.seq,
                    entry.retained_output,
                );
            } else {
                self.after = entry.seq;
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
        if self.after == snapshot.head {
            let pending = self.approvals.clone().into_entries();
            self.interrupt_current(snapshot.active_turn, &pending);
            if self.pending.is_empty() {
                self.queue_fragments(
                    &snapshot.id,
                    snapshot.runtime_generation,
                    snapshot.active_turn,
                    snapshot.head,
                );
            }
        }
    }
}
