use super::{AgentCommand, AgentCommandService, AgentEventKind, AgentMessageRole};
use crate::resource_access::ResourceTarget;
use crate::sandbox::{
    AgentEventStreamAuthorization, AgentReadError, AgentReadService, AgentReplayWindow,
};

impl AgentCommandService {
    /// Optional correlation for exact same-transaction steering messages, under held history access.
    pub async fn bind_message_window(
        &self,
        reads: &AgentReadService,
        authorization: &AgentEventStreamAuthorization,
        mut window: AgentReplayWindow,
    ) -> Result<AgentReplayWindow, AgentReadError> {
        let result = authorization
            .bounded(async {
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
                    if !matches!(
                        entry.kind,
                        Some(AgentEventKind::Message {
                            role: AgentMessageRole::User,
                            ..
                        })
                    ) {
                        continue;
                    }
                    let Some(operation) = reads.steering_operation(authorization, entry).await?
                    else {
                        continue;
                    };
                    let Some(record) = self
                        .store
                        .get_operation_command(id, operation)
                        .await
                        .map_err(|_| {
                            AgentReadError::Unavailable("message binding read unavailable".into())
                        })?
                    else {
                        // Historical steering through a non-ledger interface remains unbound.
                        continue;
                    };
                    record.validate().map_err(|_| corrupt())?;
                    let envelope = &record.receipt.envelope;
                    let dispatch = record.work.dispatch.as_ref().ok_or_else(corrupt)?;
                    if &envelope.agent_id != id
                        || envelope.owner != owner
                        || !matches!(envelope.command, AgentCommand::Steer { .. })
                        || record.receipt.response_id.is_some()
                        || dispatch.operation_sequence != operation
                        || dispatch.turn_sequence >= operation
                        || record.target_turn != Some(dispatch.turn_sequence)
                        || dispatch.claim.command_id != envelope.command_id
                        || &dispatch.claim.agent_id != id
                        || dispatch.fenced_at != entry.appended_at
                        || timestamp(&record.receipt.admitted_at)?
                            < timestamp(&original.created_at)?
                    {
                        return Err(corrupt());
                    }
                    entry.message_command_id = Some(envelope.command_id.clone());
                }
                Ok(window)
            })
            .await;
        reads.validate_event_stream(authorization).await?;
        result
    }
}
fn timestamp(value: &str) -> Result<time::OffsetDateTime, AgentReadError> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|_| corrupt())
}
fn corrupt() -> AgentReadError {
    AgentReadError::Corrupt("Agent message command binding is unavailable".into())
}
