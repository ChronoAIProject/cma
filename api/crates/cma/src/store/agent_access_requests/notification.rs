//! Persist notification intent before dispatch and fence safe turn-change retries.
use super::*;
use layer2_core::sandbox::agent_io::{
    command_key_digest, AdmittedAgentCommand, AgentCommand, AgentInput, CommandFailureCode,
};
use layer2_core::sandbox::{Agent, AgentOperationDelivery, AgentStatus, Sandbox, SandboxStatus};

impl AgentAccessRecord {
    pub(crate) fn notification_text(&self) -> Result<String, AgentAccessError> {
        let services = self
            .approved_services()?
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        Ok(format!("The owner approved service access request {}. The updated agent key has been delivered to this Sandbox with access to: {}. Continue the original task in this conversation and retry the service operation that needed access. Do not print or reveal credentials.", self.id, services))
    }

    pub(crate) fn notification_key(&self) -> String {
        match self
            .progress
            .notification_intent
            .as_ref()
            .map(|intent| intent.attempt)
        {
            Some(attempt) if attempt > 0 => format!("agent-access:{}:ready:{attempt}", self.id),
            _ => format!("agent-access:{}:ready", self.id),
        }
    }

    pub(crate) fn notification_command(&self) -> Result<AgentCommand, AgentAccessError> {
        let text = self.notification_text()?;
        Ok(
            if self
                .progress
                .notification_intent
                .as_ref()
                .is_some_and(|intent| intent.target_turn.is_some())
            {
                AgentCommand::Steer {
                    input: AgentInput::text(text),
                }
            } else {
                AgentCommand::message(text)
            },
        )
    }
}

impl MongoSandboxStore {
    pub(crate) async fn prepare_access_notification(
        &self,
        lease: &AgentAccessLease,
    ) -> Result<AgentAccessRecord, AgentAccessError> {
        let at = layer2_core::now_rfc3339();
        transaction!(self, session, { prepare(self, lease, &at, session).await })
    }
}

async fn prepare(
    store: &MongoSandboxStore,
    lease: &AgentAccessLease,
    at: &str,
    session: &mut ClientSession,
) -> TxResult<AgentAccessRecord> {
    let mut record = super::worker::leased(store, lease, session).await?;
    if record.phase != AgentAccessPhase::Notifying || record.progress.hot_update.is_none() {
        return Err(AgentAccessError::Conflict.into());
    }
    let snapshot = validate_lineage_transaction(store, &record.requester, session, false).await?;
    let agent = snapshot
        .resource
        .lineage
        .agent
        .as_ref()
        .ok_or(AgentAccessError::NotFound)?;
    let sandbox = &snapshot.resource.lineage.sandbox;
    let previous = record.progress.clone();
    if let Some(intent) = previous.notification_intent.as_ref() {
        let command = admitted(store, &record, session).await?;
        if let Some(command) = command {
            if retryable_turn_change(&command) {
                select_unadmitted(
                    &mut record,
                    agent,
                    sandbox,
                    Some(intent),
                    Some(command.receipt.envelope.command_id),
                )?;
            } else {
                // A lost admission reply is pinned even when its turn ended.
                // Unknown delivery never changes kind, target or key.
                record.progress.notification_command_id = Some(command.receipt.envelope.command_id);
            }
        } else {
            if previous.notification_command_id.is_some() {
                return Err(AgentAccessError::Unavailable.into());
            }
            select_unadmitted(&mut record, agent, sandbox, Some(intent), None)?;
        }
    } else {
        // Only the new hot workflow selects intent. Legacy paused workflows
        // retain their existing Message key and are never silently reinterpreted.
        if previous.notification_command_id.is_some() {
            return Err(AgentAccessError::Invalid.into());
        }
        select_unadmitted(&mut record, agent, sandbox, None, None)?;
    }
    if record.phase == AgentAccessPhase::Notifying && record.progress == previous {
        return Ok(record);
    }
    // The command admission transaction writes this same Agent document. An
    // old observation cannot commit between the absent-command read and this
    // persisted replacement; a later old admission also fails its revision.
    // Terminal drift is committed with this proof too, so an obsolete callback
    // cannot retain the Sandbox's scope-update lock indefinitely.
    fence(store, &snapshot, session).await?;
    let revision = record.revision;
    record.revision += 1;
    record.updated_at = at.to_owned();
    replace_leased(store, lease, revision, &record, session).await?;
    Ok(record)
}

fn select_unadmitted(
    record: &mut AgentAccessRecord,
    agent: &Agent,
    sandbox: &Sandbox,
    previous: Option<&AgentAccessNotificationIntent>,
    failed_command: Option<layer2_core::ids::AgentCommandId>,
) -> Result<(), AgentAccessError> {
    let hot = record
        .progress
        .hot_update
        .as_ref()
        .ok_or(AgentAccessError::Invalid)?;
    if sandbox.lifecycle.next_authority != hot.lifecycle_next_authority
        || sandbox
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime != &hot.runtime)
        || matches!(
            sandbox.status,
            SandboxStatus::Pausing | SandboxStatus::Paused | SandboxStatus::Retiring
        )
    {
        // The command lookup precedes this check. Only an absent command or a
        // proved pre-write turn failure can reach it; uncertain delivery stays
        // pinned to its existing receipt even after the runtime changes.
        record.phase = AgentAccessPhase::Failed;
        record.error = Some("agent_notification_runtime_changed".into());
        record.lease = None;
        record.progress.notification_command_id = failed_command;
        return Ok(());
    }
    if sandbox.status != SandboxStatus::Active || sandbox.runtime.is_none() {
        // Ordinary credential rotation can settle on the same runtime.
        return Err(AgentAccessError::Conflict);
    }
    let mut next = choose(agent, previous, failed_command.is_some())?;
    if let Some(command) = failed_command {
        next.previous_commands.push(command);
    }
    record.progress.notification_intent = Some(next);
    record.progress.notification_command_id = None;
    Ok(())
}

fn choose(
    agent: &Agent,
    previous: Option<&AgentAccessNotificationIntent>,
    retry_failed: bool,
) -> Result<AgentAccessNotificationIntent, AgentAccessError> {
    let target_turn = match (agent.status, agent.conversation.active_turn) {
        (AgentStatus::Ready, None) => None,
        (AgentStatus::Working, Some(turn))
            if agent
                .capabilities
                .supports(layer2_core::RuntimeCapability::Steer) =>
        {
            Some(turn)
        }
        _ => return Err(AgentAccessError::Conflict),
    };
    if previous.is_some_and(|old| {
        agent.revision < old.agent_revision
            || (!retry_failed
                && agent.revision == old.agent_revision
                && target_turn != old.target_turn)
    }) {
        return Err(AgentAccessError::Conflict);
    }
    let attempt = previous.map_or(0, |old| {
        old.attempt + u32::from(retry_failed || old.target_turn != target_turn)
    });
    if attempt > 64 {
        return Err(AgentAccessError::Invalid);
    }
    Ok(AgentAccessNotificationIntent {
        attempt,
        agent_revision: agent.revision,
        target_turn,
        previous_commands: previous.map_or_else(Vec::new, |old| old.previous_commands.clone()),
    })
}

async fn admitted(
    store: &MongoSandboxStore,
    record: &AgentAccessRecord,
    session: &mut ClientSession,
) -> TxResult<Option<AdmittedAgentCommand>> {
    let Some(AgentAccessDecision::Approved { authorization, .. }) = &record.decision else {
        return Err(AgentAccessError::Invalid.into());
    };
    // Service consent has a stronger namespace than ordinary command custody.
    // The callback uses the same exact token through normal command admission.
    let mut command_authorization = authorization.clone();
    if let Some(api) = command_authorization
        .authority_id
        .strip_prefix("owner-api:")
    {
        command_authorization.authority_id = format!("api:{api}");
    }
    let expected = record.notification_command()?;
    let scope =
        command_authorization.idempotency_scope(&record.requester.agent_id, expected.kind());
    let key =
        command_key_digest(&record.notification_key()).map_err(|_| AgentAccessError::Invalid)?;
    let Some(value) = store
        .agent_commands
        .find_one(doc! {"scope": &scope,"key_sha256": &key})
        .session(&mut *session)
        .await?
    else {
        return Ok(None);
    };
    let command = crate::store::agent_commands::read_command(&value)
        .map_err(|_| AgentAccessError::Unavailable)?;
    let envelope = &command.receipt.envelope;
    if envelope.agent_id != record.requester.agent_id
        || command.scope() != scope
        || command.key_sha256 != key
        || envelope.command != expected
        || record
            .progress
            .notification_command_id
            .as_ref()
            .is_some_and(|id| id != &envelope.command_id)
        || record
            .progress
            .notification_intent
            .as_ref()
            .is_some_and(|intent| {
                intent.target_turn.is_some() && command.target_turn != intent.target_turn
            })
    {
        return Err(AgentAccessError::Unavailable.into());
    }
    Ok(Some(command))
}

fn retryable_turn_change(command: &AdmittedAgentCommand) -> bool {
    let delivery = command
        .work
        .observation
        .as_ref()
        .map(|value| value.delivery)
        .or_else(|| command.work.dispatch.as_ref().map(|value| value.delivery));
    command.status.is_terminal()
        && command.work.lease.is_none()
        && command
            .work
            .failure
            .as_ref()
            .is_some_and(|failure| failure.code == CommandFailureCode::TargetTurnChanged)
        && matches!(
            delivery,
            None | Some(AgentOperationDelivery::PreWriteFailed)
        )
}

#[cfg(test)]
#[path = "notification_tests.rs"]
mod tests;
