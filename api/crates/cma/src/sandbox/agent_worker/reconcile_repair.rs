//! Read-only operator evidence and dry-run inspection.

use std::sync::Arc;

use layer2_core::sandbox::store::{AgentLifecycleStore, SandboxStoreError};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentStatus, NativeCompletionEvidence, SandboxStatus,
};
use layer3_adapter::{AgentSlot, SandboxAdapter};
use sha2::{Digest, Sha256};

use super::reconcile_history::{parse_history, MAX_HISTORY_BYTES};
use super::RepairReport;
use layer2_core::AgentId;

const MAX_RECONCILIATION_LOG_ENTRIES: u64 = 4_096;

/// Read-only operator inspection used by the dry-run command. It has no
/// recorder, lifecycle worker, or checkpoint writer in its dependency graph.
pub(crate) async fn inspect_stale_turns_read_only(
    store: Arc<dyn AgentLifecycleStore>,
    adapter: Arc<dyn SandboxAdapter>,
    ids: &[AgentId],
) -> Result<RepairReport, SandboxStoreError> {
    let mut report = RepairReport::default();
    for id in ids.iter().take(64) {
        report.inspected += 1;
        let Some(agent) = store.get_agent(id).await? else {
            report.held += 1;
            continue;
        };
        if collect_operator_evidence(store.as_ref(), adapter.as_ref(), &agent)
            .await?
            .is_some()
        {
            report.eligible += 1;
        } else {
            report.held += 1;
        }
    }
    Ok(report)
}

async fn collect_operator_evidence(
    store: &dyn AgentLifecycleStore,
    adapter: &dyn SandboxAdapter,
    agent: &Agent,
) -> Result<Option<(NativeCompletionEvidence, String)>, SandboxStoreError> {
    let Some(turn_sequence) = agent.conversation.active_turn else {
        return Ok(None);
    };
    if agent.status != AgentStatus::Working || agent.requested_stop_reason.is_some() {
        return Ok(None);
    }
    let Some(generation) = agent.conversation.runtime_generation else {
        return Ok(None);
    };
    let Some(conversation) = agent.conversation.conversation_id.as_deref() else {
        return Ok(None);
    };
    let Some(sandbox) = store.get_sandbox(&agent.sandbox_id).await? else {
        return Ok(None);
    };
    if sandbox.status != SandboxStatus::Active {
        return Ok(None);
    }
    let Some(runtime) = sandbox.runtime.as_ref() else {
        return Ok(None);
    };
    let handle = layer3_adapter::RuntimeHandle::new(&runtime.runtime_id);
    let slot = AgentSlot::new(agent.id.as_str());
    let first = adapter.export_agent_history(&handle, &slot).await;
    let second = adapter.export_agent_history(&handle, &slot).await;
    let (Ok(first), Ok(second)) = (first, second) else {
        return Ok(None);
    };
    if first.len() > MAX_HISTORY_BYTES || first != second {
        return Ok(None);
    }
    let hash = format!("{:x}", Sha256::digest(&first));
    let Some(expected) = agent
        .conversation
        .observations
        .usage
        .as_ref()
        .filter(|usage| usage.conversation_id == conversation)
        .and_then(|usage| usage.current_turn)
        .filter(|turn| turn.turn_sequence == turn_sequence)
        .and(agent.conversation.observations.usage.as_ref())
        .map(|usage| usage.source_turn.as_str())
    else {
        return Ok(None);
    };
    let Some(native) = parse_history(&first, conversation, expected) else {
        return Ok(None);
    };
    let Some(start) = store.agent_log_entry(&agent.id, turn_sequence).await? else {
        return Ok(None);
    };
    if !matches!(start.kind, AgentLogKind::TurnStarted { .. }) {
        return Ok(None);
    }
    let Some(final_answer_durable) =
        durable_final_answer_from_store(store, agent, start.seq, &native.final_answer).await?
    else {
        return Ok(None);
    };
    let evidence = NativeCompletionEvidence {
        native_conversation_id: native.conversation_id,
        runtime_generation: generation,
        turn_sequence,
        native_turn_id: native.turn_id,
        admitted_at: start.appended_at,
        completed_at: native.completed_at,
        task_started: true,
        task_complete: true,
        no_later_open_turn: true,
        history_complete: true,
        history_prefix_unchanged: true,
        history_sha256: hash.clone(),
        final_answer: native.final_answer,
        final_answer_durable,
        operation_sequence: agent
            .conversation
            .operation
            .as_ref()
            .map_or(0, |op| op.sequence),
        sandbox_revision: sandbox.revision,
        sandbox_status: sandbox.status,
    };
    if evidence.validate_against(agent).is_err() {
        return Ok(None);
    }
    Ok(Some((evidence, hash)))
}

async fn durable_final_answer_from_store(
    store: &dyn AgentLifecycleStore,
    agent: &Agent,
    start_seq: u64,
    answer: &str,
) -> Result<Option<bool>, SandboxStoreError> {
    if answer.len() > layer2_core::sandbox::MAX_DURABLE_AGENT_MESSAGE_BYTES
        || start_seq > agent.conversation.last_seq
    {
        return Ok(None);
    }
    let count = agent.conversation.last_seq - start_seq;
    if count > MAX_RECONCILIATION_LOG_ENTRIES {
        return Ok(None);
    }
    let expected = format!("{:x}", Sha256::digest(answer.as_bytes()));
    let mut after = start_seq;
    while after < agent.conversation.last_seq {
        let limit = (agent.conversation.last_seq - after)
            .min(u64::from(layer2_core::sandbox::store::MAX_PAGE_SIZE)) as u16;
        let entries = store.agent_log(&agent.id, after, limit).await?;
        if entries.len() != usize::from(limit) {
            return Ok(None);
        }
        for entry in entries {
            if let AgentLogKind::AgentMessage { content } = entry.kind {
                if format!("{:x}", Sha256::digest(content.as_bytes())) == expected {
                    return Ok(Some(true));
                }
            }
            after = entry.seq;
        }
    }
    Ok(Some(false))
}
