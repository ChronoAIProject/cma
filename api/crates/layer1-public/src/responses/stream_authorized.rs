use super::*;
use crate::{conversation::read_failure, AuthenticatedResourceCaller};
use layer2_core::sandbox::{AgentLogEntry, AgentReadError};

pub(in crate::responses) async fn open(
    state: AgentConversationState,
    resource: AuthenticatedResourceCaller,
    agent: Agent,
    turn: u64,
    watching: Option<layer2_core::sandbox::AgentLiveSubscription>,
) -> Response {
    let authorization = match state
        .reads
        .authorize_event_stream(
            &resource.context,
            resource.credential.as_ref().map(|value| value.value()),
            &agent.id,
        )
        .await
    {
        Ok(value) => value,
        Err(error) => return read_failure(error, "response_not_found"),
    };
    let indexed = match state.reads.output_is_indexed(&authorization).await {
        Ok(value) => value,
        Err(error) => return read_failure(error, "response_not_found"),
    };
    if !indexed {
        let snapshot = match state.reads.event_stream_snapshot(&authorization).await {
            Ok(value) => value,
            Err(error) => return read_failure(error, "response_not_found"),
        };
        if !snapshot.history.iter().any(|entry| entry.seq == turn
        && matches!(entry.kind, Some(AgentLogKind::TurnStarted { turn_sequence, .. }) if turn_sequence == turn)) {
        return crate::rest::instance_problem::InstanceProblem::Unavailable.into_response();
    }
    }
    open_inner(
        state,
        resource.context.caller,
        agent,
        turn,
        watching,
        Some(authorization),
        indexed,
    )
}

impl Open {
    pub(super) async fn read(&self) -> Result<(Agent, Vec<AgentLogEntry>), AgentReadError> {
        let Some(authorization) = &self.authorization else {
            return self
                .state
                .reads
                .log_snapshot(&self.caller, &self.agent.id)
                .await;
        };
        let snapshot = self
            .state
            .reads
            .event_stream_snapshot(authorization)
            .await?;
        let mut agent = self.agent.clone();
        agent.conversation.last_seq = snapshot.head;
        agent.conversation.active_turn = snapshot.active_turn;
        agent.conversation.runtime_generation = snapshot.runtime_generation;
        agent.profile.content.model = snapshot.initial_model;
        let history = snapshot
            .history
            .into_iter()
            .filter_map(|entry| {
                Some(AgentLogEntry {
                    agent_id: snapshot.id.clone(),
                    seq: entry.seq,
                    appended_at: entry.appended_at,
                    kind: entry.kind?,
                })
            })
            .collect();
        Ok((agent, history))
    }
}
