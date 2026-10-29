use async_trait::async_trait;

use crate::sandbox::agent_io::{
    command_ready_for_claim, AdmittedAgentCommand, AgentCommand, AgentCommandWorkerStore,
    CommandClaim, CommandClaimRequest, CommandFailureCode, CommandLedgerError,
};
use crate::sandbox::store::InMemorySandboxStore;

#[cfg(test)]
#[path = "agent_command_order_tests.rs"]
mod order_tests;

pub(super) fn bind_dispatch(
    state: &mut super::State,
    mutation: &crate::sandbox::store::AgentConversationMutation,
) -> Result<(), crate::sandbox::store::SandboxStoreError> {
    use crate::sandbox::store::SandboxStoreError;
    let Some(claim) = &mutation.command_claim else {
        return Ok(());
    };
    let lane = state
        .agent_command_orders
        .get(&claim.agent_id)
        .ok_or(SandboxStoreError::Conflict)?;
    let current = state
        .agents
        .get(&claim.agent_id)
        .ok_or(SandboxStoreError::Conflict)?;
    if let Some(fence) = &mutation.execution_fence {
        super::bindings::require_conversation_authority(state, fence.binding_authority())?;
        let snapshot = super::resource_access::snapshot(state, &fence.target())
            .map_err(|_| SandboxStoreError::Conflict)?;
        fence
            .validate_snapshot(&snapshot)
            .map_err(|_| SandboxStoreError::Conflict)?;
    } else if let Some(policy) = state.resource_policies.get(&current.sandbox_id) {
        let sandbox = state
            .sandboxes
            .get(&current.sandbox_id)
            .ok_or(SandboxStoreError::Conflict)?;
        if !policy
            .personal_owner_of(sandbox)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?
        {
            return Err(SandboxStoreError::Conflict);
        }
    }
    let latest_turn = state
        .agent_logs
        .get(&claim.agent_id)
        .into_iter()
        .flatten()
        .rev()
        .find_map(|entry| match entry.kind {
            crate::sandbox::AgentLogKind::TurnStarted { turn_sequence, .. } => Some(turn_sequence),
            _ => None,
        });
    let envelope = &state
        .agent_commands
        .get(&claim.command_id)
        .ok_or(SandboxStoreError::Conflict)?
        .receipt
        .envelope;
    if let Some(proof) = &envelope.continuation {
        let parent = state
            .agent_commands
            .get(&proof.parent_command_id)
            .ok_or(SandboxStoreError::Conflict)?;
        proof
            .require_parent(envelope, &parent.receipt)
            .map_err(|_| SandboxStoreError::Conflict)?;
    }
    let command = state
        .agent_commands
        .get_mut(&claim.command_id)
        .ok_or(SandboxStoreError::Conflict)?;
    let dispatch = crate::sandbox::agent_io::bind_command_dispatch(
        command,
        lane,
        current,
        latest_turn,
        mutation,
        &crate::now_rfc3339(),
    )
    .map_err(|error| match error {
        CommandLedgerError::Corrupt => SandboxStoreError::Corrupt(error.to_string()),
        _ => SandboxStoreError::Conflict,
    })?;
    if let Some(fence) = &mutation.execution_fence {
        let evidence = fence
            .authorization_audit()
            .map_err(|_| SandboxStoreError::Conflict)?;
        let audit = crate::sandbox::agent_io::CommandAuthorizationAudit::new(
            command,
            crate::sandbox::agent_io::CommandAuthorizationEffect::Dispatch { dispatch },
            vec![evidence],
        )
        .map_err(|_| SandboxStoreError::Conflict)?;
        super::command_audit::append(state, audit).map_err(|_| SandboxStoreError::Conflict)?;
    }
    Ok(())
}

#[async_trait]
impl AgentCommandWorkerStore for InMemorySandboxStore {
    async fn reject_unclaimed_command(
        &self,
        id: &crate::ids::AgentCommandId,
        expected_revision: u64,
        code: CommandFailureCode,
        now: String,
    ) -> Result<(), CommandLedgerError> {
        let mut state = self.lock();
        let mut command = state
            .agent_commands
            .get(id)
            .cloned()
            .ok_or(CommandLedgerError::NotFound)?;
        command.reject_unclaimed(expected_revision, code, now)?;
        command.validate()?;
        state.agent_commands.insert(id.clone(), command);
        Ok(())
    }

    async fn command_work_page(
        &self,
        page: &crate::sandbox::store::PageRequest,
    ) -> Result<crate::sandbox::store::Page<AdmittedAgentCommand>, CommandLedgerError> {
        crate::sandbox::store::PageRequest::new(page.limit, page.cursor.clone())
            .map_err(|_| CommandLedgerError::Invalid)?;
        let state = self.lock();
        let mut items: Vec<_> = state
            .agent_commands
            .values()
            .filter(|record| {
                !record.status.is_terminal()
                    && page
                        .cursor
                        .as_deref()
                        .is_none_or(|cursor| record.receipt.envelope.command_id.as_str() > cursor)
            })
            .collect();
        items.sort_by(|a, b| {
            a.receipt
                .envelope
                .command_id
                .cmp(&b.receipt.envelope.command_id)
        });
        crate::sandbox::agent_io::finish_command_work_page(
            items
                .into_iter()
                .take(usize::from(page.limit) + 1)
                .cloned()
                .collect(),
            page,
        )
    }

    async fn reconcile_command(
        &self,
        id: &crate::ids::AgentCommandId,
    ) -> Result<AdmittedAgentCommand, CommandLedgerError> {
        use crate::sandbox::agent_io::CommandEvidence;
        let mut state = self.lock();
        let mut command = state
            .agent_commands
            .get(id)
            .cloned()
            .ok_or(CommandLedgerError::NotFound)?;
        command.validate()?;
        if command.work.dispatch.is_none() || command.status.is_terminal() {
            return Ok(command);
        }
        let agent_id = command.receipt.envelope.agent_id.clone();
        let agent = state
            .agents
            .get(&agent_id)
            .ok_or(CommandLedgerError::NotFound)?;
        let mut lane = state
            .agent_command_orders
            .get(&agent_id)
            .cloned()
            .ok_or(CommandLedgerError::Corrupt)?;
        let mut evidence = CommandEvidence::new(&command)?;
        for entry in state.agent_logs.get(&agent_id).into_iter().flatten() {
            evidence.observe(entry)?;
        }
        let retry_current = evidence.has_source_deferred()
            && super::bindings::continuation_retry_authority(&state, &command.receipt.envelope);
        if evidence.apply_with_continuation_retry(&mut command, &mut lane, agent, retry_current)? {
            state.agent_commands.insert(id.clone(), command.clone());
            state.agent_command_orders.insert(agent_id, lane);
        }
        Ok(command)
    }

    async fn claim_next_command(
        &self,
        request: CommandClaimRequest,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        claim_next(self, request, None)
    }

    async fn claim_observed_command(
        &self,
        request: CommandClaimRequest,
        command_id: &crate::ids::AgentCommandId,
        expected_revision: u64,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        claim_next(self, request, Some((command_id, expected_revision)))
    }

    async fn renew_command_claim(
        &self,
        claim: &CommandClaim,
        now: String,
        expires_at: String,
    ) -> Result<(), CommandLedgerError> {
        let request = CommandClaimRequest {
            agent_id: claim.agent_id.clone(),
            holder: claim.holder.clone(),
            now: now.clone(),
            expires_at: expires_at.clone(),
        };
        request.validate()?;
        change(self, claim, &now, |command, lane| {
            let previous = command
                .work
                .lease
                .as_ref()
                .ok_or(CommandLedgerError::StaleClaim)?;
            if crate::seconds_from_rfc3339(&expires_at)
                < crate::seconds_from_rfc3339(&previous.expires_at)
            {
                return Err(CommandLedgerError::Invalid);
            }
            command.work.lease.as_mut().unwrap().expires_at = expires_at.clone();
            lane.lease.as_mut().unwrap().expires_at = expires_at;
            command.revision = command
                .revision
                .checked_add(1)
                .ok_or(CommandLedgerError::Corrupt)?;
            Ok(())
        })
        .map(|_| ())
    }

    async fn release_command_claim(
        &self,
        claim: &CommandClaim,
        now: String,
    ) -> Result<(), CommandLedgerError> {
        change(self, claim, &now, |command, lane| {
            command.work.lease = None;
            lane.lease = None;
            command.revision = command
                .revision
                .checked_add(1)
                .ok_or(CommandLedgerError::Corrupt)?;
            Ok(())
        })
        .map(|_| ())
    }

    async fn reject_command(
        &self,
        claim: &CommandClaim,
        code: CommandFailureCode,
        now: String,
    ) -> Result<AdmittedAgentCommand, CommandLedgerError> {
        change(self, claim, &now, |command, lane| {
            command.reject_before_dispatch(claim, code, now.clone())?;
            lane.lease = None;
            Ok(())
        })
    }
}

fn change(
    store: &InMemorySandboxStore,
    claim: &CommandClaim,
    now: &str,
    update: impl FnOnce(
        &mut AdmittedAgentCommand,
        &mut crate::sandbox::agent_io::CommandLane,
    ) -> Result<(), CommandLedgerError>,
) -> Result<AdmittedAgentCommand, CommandLedgerError> {
    let mut state = store.lock();
    let mut lane = state
        .agent_command_orders
        .get(&claim.agent_id)
        .cloned()
        .ok_or(CommandLedgerError::StaleClaim)?;
    let mut command = state
        .agent_commands
        .get(&claim.command_id)
        .cloned()
        .ok_or(CommandLedgerError::StaleClaim)?;
    lane.require_claim(claim, now)?;
    command.require_claim(claim, now)?;
    if lane.lease != command.work.lease {
        return Err(CommandLedgerError::Corrupt);
    }
    update(&mut command, &mut lane)?;
    command.validate()?;
    state
        .agent_command_orders
        .insert(claim.agent_id.clone(), lane);
    state
        .agent_commands
        .insert(claim.command_id.clone(), command.clone());
    Ok(command)
}

fn claim_next(
    store: &InMemorySandboxStore,
    request: CommandClaimRequest,
    observed: Option<(&crate::ids::AgentCommandId, u64)>,
) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
    request.validate()?;
    let mut state = store.lock();
    let Some(mut lane) = state.agent_command_orders.get(&request.agent_id).cloned() else {
        return Ok(None);
    };
    if lane
        .lease
        .as_ref()
        .is_some_and(|lease| lease.live_at(&request.now))
    {
        return Ok(None);
    }
    let agent = state.agents.get(&request.agent_id);
    let mut waiting: Vec<_> = state
        .agent_commands
        .values()
        .filter(|command| command.receipt.envelope.agent_id == request.agent_id)
        .collect();
    waiting.sort_by_key(|command| command.queue_priority());
    let mut selected = None;
    let mut message_held = false;
    for command in waiting {
        command.validate()?;
        message_held |= command.holds_message_order(&request.now);
        if message_held
            && matches!(
                command.receipt.envelope.command,
                AgentCommand::Message { .. }
            )
        {
            continue;
        }
        if command_ready_for_claim(command, agent, &request.now) {
            selected = Some(command.clone());
            break;
        }
    }
    let Some(mut command) = selected else {
        return Ok(None);
    };
    if observed.is_some_and(|(id, revision)| {
        id != &command.receipt.envelope.command_id || revision != command.revision
    }) {
        return Ok(None);
    }
    if !lane.claim(&mut command, &request)? {
        return Ok(None);
    }
    state.agent_command_orders.insert(request.agent_id, lane);
    state
        .agent_commands
        .insert(command.receipt.envelope.command_id.clone(), command.clone());
    Ok(Some(command))
}
