use super::{AgentCommand, AgentCommandService, AgentEventKind};
use crate::resource_access::ResourceTarget;
use crate::sandbox::{
    AgentEventStreamAuthorization, AgentReadError, AgentReadService, AgentReplayWindow,
    AgentResponseBinding,
};

impl AgentCommandService {
    /// Correlation is established from the durable source command under the
    /// held history authority, before the public projection can leave core.
    pub async fn bind_replay_window(
        &self,
        reads: &AgentReadService,
        authorization: &AgentEventStreamAuthorization,
        window: AgentReplayWindow,
    ) -> Result<AgentReplayWindow, AgentReadError> {
        let result = authorization
            .bounded(self.bind_replay_window_unfenced(authorization, window))
            .await;
        // Dependency failures cannot extend revoked or expired authority.
        reads.validate_event_stream(authorization).await?;
        result
    }

    /// The caller must bound all private IO and fence every result before release.
    pub(in crate::sandbox) async fn bind_replay_window_unfenced(
        &self,
        authorization: &AgentEventStreamAuthorization,
        mut window: AgentReplayWindow,
    ) -> Result<AgentReplayWindow, AgentReadError> {
        let (held, id, _) = authorization.resource();
        let target = ResourceTarget::Agent {
            agent_id: id.clone(),
        };
        let owner = held.snapshot.scope(&target).map_err(|_| corrupt())?.owner;
        let original = held.snapshot.lineage.agent.as_ref().ok_or_else(corrupt)?;
        if &window.header.id != id || window.header.created_at != original.created_at {
            return Err(corrupt());
        }
        for entry in &mut window.entries {
            let Some(candidate) = entry.command_candidate.take() else {
                continue;
            };
            let Some(AgentEventKind::TurnStarted { turn_sequence }) = &entry.kind else {
                return Err(corrupt());
            };
            let command_id = crate::ids::AgentCommandId(candidate);
            let record = self
                .store
                .get_command(&command_id)
                .await
                .map_err(|_| {
                    AgentReadError::Unavailable("command binding read unavailable".into())
                })?
                .ok_or_else(corrupt)?;
            record.validate().map_err(|_| corrupt())?;
            let envelope = &record.receipt.envelope;
            let dispatch = record.work.dispatch.as_ref().ok_or_else(corrupt)?;
            let timestamp = |value: &str| {
                time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                    .map_err(|_| corrupt())
            };
            if envelope.command_id != command_id
                || &envelope.agent_id != id
                || envelope.owner != owner
                || !matches!(envelope.command, AgentCommand::Message { .. })
                || dispatch.turn_sequence != *turn_sequence
                || entry.sequence != *turn_sequence
                || dispatch.claim.command_id != command_id
                || &dispatch.claim.agent_id != id
                || dispatch.fenced_at != entry.appended_at
                || timestamp(&record.receipt.admitted_at)? < timestamp(&original.created_at)?
            {
                return Err(corrupt());
            }
            entry.response_binding = Some(AgentResponseBinding {
                command_id,
                response_id: record.receipt.response_id.clone().ok_or_else(corrupt)?,
                turn_sequence: *turn_sequence,
            });
        }
        Ok(window)
    }
}

fn corrupt() -> AgentReadError {
    AgentReadError::Corrupt("Agent turn command binding is unavailable".into())
}
