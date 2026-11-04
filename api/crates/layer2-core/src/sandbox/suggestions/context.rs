use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::*;
use crate::sandbox::{
    Agent, AgentLogEntry, AgentLogKind, AgentTranscriptMessage, AgentTranscriptRole,
    AgentTranscriptTurn, AgentTurnOutcome, Sandbox,
};

pub const MAX_CONTEXT_EVENTS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestionContext {
    pub goal: String,
    pub answer: String,
    pub confirmed_resources: Vec<String>,
}

pub fn authority_digest(agent: &Agent, sandbox: &Sandbox) -> String {
    let identity = serde_json::json!([
        agent.owner,
        agent.sandbox_id,
        agent.workspace_id,
        agent.worktree_id,
        agent.profile,
        agent.conversation.preferences,
        agent.conversation.conversation_id,
        agent.conversation.runtime_generation,
        sandbox.owner,
        sandbox.runtime,
        sandbox.credential.current,
        sandbox.credential.reaches,
        sandbox.credential.service_grants,
        sandbox.profile.content.model_access,
    ]);
    format!("{:x}", Sha256::digest(identity.to_string().as_bytes()))
}

pub fn work_from_turn(
    agent: &Agent,
    sandbox: &Sandbox,
    entries: &[AgentLogEntry],
) -> Option<SuggestionWork> {
    if entries.len() > MAX_CONTEXT_EVENTS || agent.owner != sandbox.owner {
        return None;
    }
    let first = entries.first()?;
    let AgentLogKind::TurnStarted { turn_sequence, .. } = first.kind else {
        return None;
    };
    if turn_sequence != first.seq {
        return None;
    }
    let completed = entries.last()?;
    if !matches!(completed.kind, AgentLogKind::TurnCompleted { turn_sequence: sequence } if sequence == turn_sequence)
    {
        return None;
    }
    let mut turn = AgentTranscriptTurn {
        turn_sequence,
        started_at: first.appended_at.clone(),
        origin_key: None,
        owner_message: String::new(),
        agent_messages: Vec::new(),
        messages: Vec::new(),
        metadata: Default::default(),
        outcome: AgentTurnOutcome::Completed {
            at: completed.appended_at.clone(),
        },
    };
    for (offset, entry) in entries.iter().enumerate() {
        if entry.agent_id != agent.id || entry.seq != turn_sequence + offset as u64 {
            return None;
        }
        let (role, content) = match &entry.kind {
            AgentLogKind::OwnerMessage { content } => (AgentTranscriptRole::Owner, content),
            AgentLogKind::AgentMessage { content } => (AgentTranscriptRole::Agent, content),
            AgentLogKind::ConversationGap { .. } | AgentLogKind::TurnFailed { .. } => return None,
            AgentLogKind::TurnStarted { .. } if offset != 0 => return None,
            _ => continue,
        };
        if role == AgentTranscriptRole::Owner {
            if !turn.owner_message.is_empty() {
                turn.owner_message.push('\n');
            }
            turn.owner_message.push_str(content);
        }
        turn.messages.push(AgentTranscriptMessage {
            sequence: entry.seq,
            role,
            content: content.clone(),
            appended_at: entry.appended_at.clone(),
        });
    }
    let answer = turn.final_answer()?;
    if turn.owner_message.trim().is_empty()
        || turn.owner_message.len() > 4096
        || answer.content.len() > 8192
    {
        return None;
    }
    let scope = AnswerContext {
        turn_sequence,
        answer_sequence: answer.sequence,
    };
    let id = format!(
        "suggestion:{:x}",
        Sha256::digest(
            serde_json::to_vec(&(&agent.owner, &agent.id, scope, STRATEGY_VERSION,)).ok()?
        )
    );
    Some(SuggestionWork {
        id,
        owner: agent.owner.clone(),
        agent_id: agent.id.clone(),
        scope,
        completed_sequence: completed.seq,
        strategy_version: STRATEGY_VERSION.into(),
        authority: authority_digest(agent, sandbox),
        status: SuggestionStatus::Pending,
        context: SuggestionContext {
            goal: turn.owner_message.clone(),
            answer: answer.content.clone(),
            confirmed_resources: Vec::new(),
        },
        set: None,
        created_at_ms: unix_millis(),
        started_at_ms: None,
        deadline_ms: None,
        observation: None,
    })
}

pub fn work_is_current(
    work: &SuggestionWork,
    agent: &Agent,
    sandbox: &Sandbox,
    tail: &[AgentLogEntry],
    newer_admission: bool,
) -> bool {
    work.agent_id == agent.id
        && work.owner == agent.owner
        && agent.sandbox_id == sandbox.id
        && agent.status == crate::sandbox::AgentStatus::Ready
        && sandbox.status == crate::sandbox::SandboxStatus::Active
        && sandbox.lifecycle.operation.is_none()
        && agent.conversation.active_turn.is_none()
        && agent
            .conversation
            .operation
            .as_ref()
            .is_none_or(|operation| {
                operation.state == crate::sandbox::AgentOperationDelivery::ExecutionObserved
                    && Some(operation.runtime_generation) == agent.conversation.runtime_generation
                    && operation.turn_sequence == work.scope.turn_sequence
                    && operation.sequence < work.completed_sequence
            })
        && agent
            .conversation
            .queued_messages
            .iter()
            .all(|message| message.state == crate::sandbox::QueuedMessageState::Delivered)
        && work.authority == authority_digest(agent, sandbox)
        && !newer_admission
        && agent.conversation.last_seq >= work.completed_sequence
        && agent.conversation.last_seq - work.completed_sequence == tail.len() as u64
        && tail.len() <= MAX_CONTEXT_EVENTS
        && tail.iter().enumerate().all(|(offset, entry)| {
            entry.agent_id == agent.id
                && entry.seq == work.completed_sequence + offset as u64 + 1
                && !matches!(
                    entry.kind,
                    AgentLogKind::TurnStarted { .. }
                        | AgentLogKind::OwnerMessage { .. }
                        | AgentLogKind::ConversationGap { .. }
                        | AgentLogKind::ConversationOpened { .. }
                )
        })
}
