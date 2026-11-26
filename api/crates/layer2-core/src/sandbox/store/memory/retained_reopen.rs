use super::*;
use crate::sandbox::{
    apply_retained_reopen, RetainedRecoverySnapshot, RetainedReopenAccepted, RetainedReopenRequest,
    RetainedReopenSnapshot,
};
use sha2::{Digest, Sha256};

pub(super) fn snapshot(
    state: &State,
    id: &SandboxId,
) -> Result<RetainedReopenSnapshot, SandboxStoreError> {
    // This stronger read excludes every publication/retirement of the retained
    // generation, including terminal children. The assessment read does not.
    let original = super::unpublished_recovery::snapshot(state, id)?;
    super::maintenance::validate_event_stream(state, id)?;
    let mut history_digest = Sha256::new();
    for agent in &original.agents {
        let entries = state
            .agent_logs
            .get(&agent.id)
            .map_or(&[][..], Vec::as_slice);
        crate::sandbox::AgentLogCheckpoint::validate_retained_semantics(agent, entries)?;
        for entry in entries {
            let bytes = serde_json::to_vec(entry).map_err(|_| {
                SandboxStoreError::Corrupt("recovery history encoding failed".into())
            })?;
            history_digest.update((bytes.len() as u64).to_be_bytes());
            history_digest.update(bytes);
        }
    }
    let mut worktrees = state
        .worktrees
        .values()
        .filter(|tree| tree.sandbox_id == *id)
        .cloned()
        .collect::<Vec<_>>();
    worktrees.sort_by(|a, b| a.id.cmp(&b.id));
    let ids = original
        .agents
        .iter()
        .map(|a| &a.id)
        .collect::<std::collections::BTreeSet<_>>();
    let mut commands = state
        .agent_commands
        .values()
        .filter(|c| ids.contains(&c.receipt.envelope.agent_id))
        .collect::<Vec<_>>();
    commands.sort_by(|a, b| {
        a.receipt
            .envelope
            .command_id
            .cmp(&b.receipt.envelope.command_id)
    });
    if commands.len() > 4096 {
        return Err(SandboxStoreError::Conflict);
    }
    for command in &commands {
        command
            .validate()
            .map_err(|_| SandboxStoreError::Corrupt("recovery command is invalid".into()))?;
        if !command.status.is_terminal() || command.work.lease.is_some() {
            return Err(SandboxStoreError::Conflict);
        }
    }
    let command_lanes: Vec<_> = original
        .agents
        .iter()
        .map(|a| {
            (
                a.id.clone(),
                state
                    .agent_command_orders
                    .get(&a.id)
                    .cloned()
                    .unwrap_or_default(),
            )
        })
        .collect();
    for (id, lane) in &command_lanes {
        let orders = commands
            .iter()
            .filter(|c| c.receipt.envelope.agent_id == *id)
            .map(|c| c.receipt.agent_order)
            .collect::<std::collections::BTreeSet<_>>();
        if lane.last_order != orders.len() as u64 || orders.iter().copied().ne(1..=lane.last_order)
        {
            return Err(SandboxStoreError::Conflict);
        }
    }
    Ok(RetainedReopenSnapshot {
        retained: RetainedRecoverySnapshot {
            sandbox: original.sandbox,
            agents: original.agents,
            workspaces: original.workspaces,
        },
        worktrees,
        history_state_sha256: format!("{:x}", history_digest.finalize()),
        event_seq: state
            .sandbox_events
            .get(id)
            .map_or(0, |events| events.len() as u64),
        command_state_sha256: crate::sandbox::retained_reopen::digest(
            &commands
                .iter()
                .map(|c| (*c, &c.queued_input))
                .collect::<Vec<_>>(),
        )?,
        command_lanes,
    })
}

pub(super) fn admit(
    state: &mut State,
    request: RetainedReopenRequest,
) -> Result<RetainedReopenAccepted, SandboxStoreError> {
    let id = &request.snapshot.retained.sandbox.id;
    let parent = state
        .sandboxes
        .get(id)
        .filter(|parent| parent.owner == request.snapshot.retained.sandbox.owner)
        .ok_or(SandboxStoreError::NotFound)?;
    if let Some(receipts) = state.retained_reopen_receipts.get(id) {
        if let Some(receipt) = receipts
            .iter()
            .find(|receipt| receipt.key_sha256 == request.key_sha256)
        {
            if receipt.expected_revision != request.snapshot.retained.sandbox.revision
                || receipt.owner != parent.owner
            {
                return Err(SandboxStoreError::Conflict);
            }
            return Ok(receipt.accepted.clone());
        }
        if receipts.last().is_some_and(|latest| {
            !request.repeat_authorized
                || parent.status != crate::sandbox::SandboxStatus::Failed
                || parent.revision <= latest.accepted.revision
                || parent.revision != request.snapshot.retained.sandbox.revision
        }) {
            return Err(SandboxStoreError::Conflict);
        }
    }
    let current = snapshot(state, id)?;
    let (next, receipt, event) = apply_retained_reopen(&current, &request)?;
    let mut candidate = state.clone();
    candidate.sandboxes.insert(id.clone(), next);
    candidate
        .retained_reopen_receipts
        .entry(id.clone())
        .or_default()
        .push(receipt.clone());
    append_sandbox_event(&mut candidate, id, request.requested_at, event)?;
    *state = candidate;
    Ok(receipt.accepted)
}
