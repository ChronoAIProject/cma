use std::collections::BTreeMap;

use super::*;
use crate::sandbox::agent_io::AgentResponseStatus;
use crate::sandbox::{AgentStatus, SandboxWorkStep};
use serde::Serialize;

pub(super) fn capture(
    state: &State,
    id: &SandboxId,
    at_ms: i64,
) -> Result<RuntimeActivityVector, SandboxStoreError> {
    let sandbox = state.sandboxes.get(id).ok_or(SandboxStoreError::NotFound)?;
    sandbox.validate().map_err(|_| invalid())?;
    let home = sandbox
        .lifecycle
        .home
        .as_ref()
        .and_then(|home| home.attachment())
        .map(|home| home.identity().map_err(|_| invalid()))
        .transpose()?;
    let seq = state
        .sandbox_events
        .get(id)
        .and_then(|events| events.last())
        .map_or(0, |event| event.seq);
    // A failed admitted update deliberately retains its phase for an explicit
    // restart. Its stopped work can earn a new interval; queued work cannot.
    let parked_update = sandbox.work.step == SandboxWorkStep::None
        && sandbox.work.lease.is_none()
        && sandbox.runtime_update_request().is_some_and(|request| {
            request.started_at.is_some()
                && request.completed_at.is_none()
                && request.failure.is_some()
                && sandbox.admitted_runtime_update_target() == Some(request.target_version.as_str())
        });
    let sandbox_row = row(
        id.as_str(),
        Some(sandbox.revision),
        Some(seq),
        &(
            (
                &sandbox.id,
                &sandbox.created_at,
                &sandbox.updated_at,
                sandbox.status,
                &sandbox.profile.id,
                sandbox.profile.revision,
                &sandbox.profile.digest,
            ),
            (
                sandbox.effective_custom_runtime_image(),
                sandbox.retained_runtime_binding(),
                home,
                sandbox.lifecycle.next_authority,
                sandbox.lifecycle.runtime_generation_high_water,
            ),
            (
                &sandbox.lifecycle.operation,
                &sandbox.last_activity_at,
                sandbox.work.next_fence,
                &sandbox.work.lease,
                sandbox.work.step,
            ),
        ),
        !matches!(
            sandbox.work.step,
            SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
        ) || sandbox.work.lease.is_some()
            || (sandbox.lifecycle.operation.is_some() && !parked_update),
    )?;
    let mut vector = RuntimeActivityVector {
        sandbox_id: id.clone(),
        sandbox: sandbox_row,
        workspaces: vec![],
        agents: vec![],
        command_orders: vec![],
        commands: vec![],
        log_heads: vec![],
        log_tails: vec![],
        desktop_leases: vec![],
    };
    for workspace in state
        .workspaces
        .values()
        .filter(|workspace| &workspace.sandbox_id == id)
    {
        bounded(&vector.workspaces)?;
        vector.workspaces.push(row(
            workspace.id.as_str(),
            Some(workspace.revision),
            None,
            &(
                &workspace.id,
                &workspace.sandbox_id,
                &workspace.created_at,
                &workspace.ready_at,
                workspace.status,
                workspace.next_worktree_number,
                workspace.work.step,
                workspace.work.next_fence,
                &workspace.work.lease,
            ),
            workspace.work.step.is_pending() || workspace.work.lease.is_some(),
        )?);
    }
    for agent in state
        .agents
        .values()
        .filter(|agent| &agent.sandbox_id == id)
    {
        bounded(&vector.agents)?;
        let busy = agent.work.step.is_pending()
            || agent.work.lease.is_some()
            || !agent.conversation.queued_messages.is_empty()
            || (agent.status != AgentStatus::Deleted && agent.conversation.active_turn.is_some())
            || agent
                .conversation
                .operation
                .as_ref()
                .is_some_and(|operation| operation.blocks_dispatch())
            || (agent.status == AgentStatus::Deleted
                && (agent.deleted_at.is_none() || !agent.work.cleanup_debt.is_empty()))
            || (agent.status.is_live()
                && !matches!(agent.status, AgentStatus::Ready | AgentStatus::Paused));
        let metadata = (
            (
                &agent.id,
                &agent.sandbox_id,
                &agent.workspace_id,
                &agent.worktree_id,
                &agent.creation_key,
                &agent.creation_fingerprint,
                &agent.created_at,
                &agent.updated_at,
            ),
            (
                agent.status,
                agent.revision,
                agent.conversation.last_seq,
                &agent.deleted_at,
                &agent.stopped_at,
                &agent.conversation.conversation_id,
                agent.conversation.runtime_generation,
            ),
            (
                agent.conversation.active_turn,
                agent.conversation.queued_messages.len(),
                agent.work.step,
                agent.work.next_fence,
                &agent.work.lease,
                agent.requested_stop_reason.is_some(),
            ),
        );
        vector.agents.push(row(
            agent.id.as_str(),
            Some(agent.revision),
            Some(agent.conversation.last_seq),
            &metadata,
            busy,
        )?);
        if let Some(lane) = state.agent_command_orders.get(&agent.id) {
            let mut record = row(agent.id.as_str(), None, None, lane, lane.lease.is_some())?;
            record.counters.insert(
                RuntimeActivityCounter::CommandLastOrder,
                Some(lane.last_order),
            );
            record.counters.insert(
                RuntimeActivityCounter::CommandNextFence,
                Some(lane.next_fence),
            );
            vector.command_orders.push(record);
        }
        let logs = state
            .agent_logs
            .get(&agent.id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if logs.len() > 4096
            || logs.len() != agent.conversation.last_seq as usize
            || logs
                .iter()
                .enumerate()
                .any(|(index, entry)| entry.agent_id != agent.id || entry.seq != index as u64 + 1)
        {
            return Err(invalid());
        }
        if let Some(last) = logs.last() {
            vector.log_tails.push(row(
                agent.id.as_str(),
                None,
                Some(last.seq),
                &(&last.agent_id, last.seq, &last.appended_at),
                false,
            )?);
        }
    }
    for command in state.agent_commands.values().filter(|command| {
        state
            .agents
            .get(&command.receipt.envelope.agent_id)
            .is_some_and(|agent| &agent.sandbox_id == id)
    }) {
        bounded(&vector.commands)?;
        let envelope = &command.receipt.envelope;
        vector.commands.push(row(
            envelope.command_id.as_str(),
            Some(command.revision),
            None,
            &(
                &envelope.command_id,
                &envelope.agent_id,
                command.receipt.agent_order,
                &command.receipt.admitted_at,
                command.status,
                command.revision,
                command.target_turn,
                &command.work.lease,
                &command.work.dispatch,
                &command.work.failure,
            ),
            matches!(
                command.status,
                AgentResponseStatus::Queued
                    | AgentResponseStatus::InProgress
                    | AgentResponseStatus::WaitingAction
            ) || command.work.lease.is_some(),
        )?);
    }
    for ((sandbox_id, session), expires) in &state.sandbox_sessions {
        if sandbox_id != id {
            continue;
        }
        bounded(&vector.desktop_leases)?;
        let session_key = crate::profiles::content_digest(session).map_err(|_| invalid())?;
        let expires_ms = crate::seconds_from_rfc3339(expires)
            .and_then(|at| i64::try_from(at).ok())
            .and_then(|at| at.checked_mul(1000))
            .ok_or_else(invalid)?;
        vector.desktop_leases.push(row(
            &session_key,
            None,
            None,
            &(session_key.clone(), expires),
            expires_ms > at_ms,
        )?);
    }
    for rows in [
        &mut vector.workspaces,
        &mut vector.agents,
        &mut vector.command_orders,
        &mut vector.commands,
        &mut vector.log_heads,
        &mut vector.log_tails,
        &mut vector.desktop_leases,
    ] {
        rows.sort_by(|a, b| a.id.cmp(&b.id));
    }
    vector.validate()?;
    Ok(vector)
}

fn row(
    id: &str,
    revision: Option<u64>,
    last_seq: Option<u64>,
    metadata: &impl Serialize,
    active: bool,
) -> Result<RuntimeActivityRecord, SandboxStoreError> {
    Ok(RuntimeActivityRecord {
        id: id.into(),
        metadata_sha256: crate::profiles::content_digest(metadata).map_err(|_| invalid())?,
        revision,
        last_seq,
        counters: BTreeMap::new(),
        active,
    })
}

fn bounded(rows: &[RuntimeActivityRecord]) -> Result<(), SandboxStoreError> {
    if rows.len() >= RUNTIME_ACTIVITY_MAX_ROWS {
        return Err(invalid());
    }
    Ok(())
}
