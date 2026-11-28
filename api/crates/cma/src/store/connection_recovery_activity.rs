//! Keep the inactivity clock separate from the durable connection audit event.
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    Agent, AgentLogEntry, AgentLogKind, AgentOperationDelivery, AgentSkillInstallStatus,
    AgentStatus, AgentWorkStep, FirstMessageDelivery, SandboxAgentLogActivity,
};
use layer2_core::seconds_from_rfc3339;
use mongodb::bson::{doc, Document};
use serde::{Deserialize, Serialize};

use super::sandbox_mongo::record_bson;

const SCHEDULED: &str = "quiet_connection_recovery_schedule_v1";
pub(super) const COMPLETED: &str = "quiet_connection_recovery_activity_v1";

#[derive(PartialEq, Eq, Serialize, Deserialize)]
struct Schedule {
    conversation_id: String,
    generation: u64,
    previous_seq: u64,
    fence: u64,
    due_at: String,
}

#[derive(Serialize, Deserialize)]
struct Completion {
    conversation_id: String,
    generation: u64,
    seq: u64,
    appended_at: String,
    activity_at: String,
}

pub(super) fn record_schedule(
    next: &Agent,
    fields: &mut Document,
) -> Result<(), SandboxStoreError> {
    if let Some(schedule) = schedule(next, next.work.next_fence) {
        fields.insert(
            SCHEDULED,
            record_bson(&schedule).map_err(SandboxStoreError::Corrupt)?,
        );
    }
    Ok(())
}

fn schedule(agent: &Agent, fence: u64) -> Option<Schedule> {
    if agent.work.step != AgentWorkStep::PublishConnection || !quiet(agent) {
        return None;
    }
    Some(Schedule {
        conversation_id: agent.conversation.conversation_id.clone()?,
        generation: agent.conversation.runtime_generation?,
        previous_seq: agent.conversation.last_seq,
        fence,
        due_at: agent.work.ready_at.clone(),
    })
}

fn quiet(agent: &Agent) -> bool {
    agent.status == AgentStatus::Ready
        && agent.requested_stop_reason.is_none()
        && agent.conversation.active_turn.is_none()
        && agent.conversation.queued_messages.is_empty()
        && agent
            .conversation
            .first_message
            .as_ref()
            .is_none_or(|message| {
                matches!(
                    message.state,
                    FirstMessageDelivery::Completed | FirstMessageDelivery::Failed
                )
            })
        && agent
            .conversation
            .operation
            .as_ref()
            .is_none_or(|operation| {
                !matches!(
                    operation.state,
                    AgentOperationDelivery::DispatchFenced
                        | AgentOperationDelivery::WriteConfirmed
                        | AgentOperationDelivery::RecoveryRequired
                )
            })
        && !agent
            .work
            .live_skills
            .iter()
            .any(|skill| skill.status == AgentSkillInstallStatus::Pending)
}

pub(super) fn is_connection_completion(entry: &AgentLogEntry) -> bool {
    matches!(
        entry.kind,
        AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::PublishConnection
        }
    )
}

/// A prior completion carries the clock only while it is still the verified head.
pub(super) fn activity_time(row: &Document, agent: &Agent, head: &AgentLogEntry) -> Option<String> {
    if !is_connection_completion(head)
        || head.agent_id != agent.id
        || head.seq != agent.conversation.last_seq
    {
        return None;
    }
    let proof: Completion =
        mongodb::bson::deserialize_from_bson(row.get(COMPLETED)?.clone()).ok()?;
    if proof.seq == head.seq
        && proof.appended_at == head.appended_at
        && Some(&proof.conversation_id) == agent.conversation.conversation_id.as_ref()
        && Some(proof.generation) == agent.conversation.runtime_generation
        && seconds_from_rfc3339(&proof.activity_at)? <= seconds_from_rfc3339(&head.appended_at)?
    {
        return Some(proof.activity_at);
    }
    None
}

pub(super) fn completed_update(
    row: &Document,
    current: &Agent,
    next: &Agent,
    previous: Option<&AgentLogEntry>,
    logs: &[AgentLogEntry],
) -> Result<Option<Document>, SandboxStoreError> {
    let (Some(previous), [entry]) = (previous, logs) else {
        return Ok(None);
    };
    let Some(scheduled) = current
        .work
        .lease
        .as_ref()
        .and_then(|lease| schedule(current, lease.fence))
    else {
        return Ok(None);
    };
    let persisted = row
        .get(SCHEDULED)
        .cloned()
        .and_then(|value| mongodb::bson::deserialize_from_bson::<Schedule>(value).ok());
    if persisted.as_ref() != Some(&scheduled)
        || next.work.step != AgentWorkStep::None
        || !quiet(next)
        || Some(&scheduled.conversation_id) != next.conversation.conversation_id.as_ref()
        || Some(scheduled.generation) != next.conversation.runtime_generation
        || previous.agent_id != current.id
        || previous.seq != scheduled.previous_seq
        || entry.seq != next.conversation.last_seq
        || !is_connection_completion(entry)
    {
        return Ok(None);
    }
    if seconds_from_rfc3339(&previous.appended_at)
        .zip(seconds_from_rfc3339(&entry.appended_at))
        .is_none_or(|(previous, completion)| previous > completion)
    {
        return Ok(None);
    }
    let activity_at =
        activity_time(row, current, previous).unwrap_or_else(|| previous.appended_at.clone());
    let proof = Completion {
        conversation_id: scheduled.conversation_id,
        generation: scheduled.generation,
        seq: entry.seq,
        appended_at: entry.appended_at.clone(),
        activity_at,
    };
    Ok(Some(doc! {
        "$set": { COMPLETED: record_bson(&proof).map_err(SandboxStoreError::Corrupt)? },
        "$unset": { SCHEDULED: "" },
    }))
}

pub(super) fn project_head(
    entry: &AgentLogEntry,
    activity_at: Option<String>,
) -> SandboxAgentLogActivity {
    SandboxAgentLogActivity {
        agent_id: entry.agent_id.clone(),
        seq: entry.seq,
        appended_at: entry.appended_at.clone(),
        activity_at,
    }
}
