//! Read-only visibility of acknowledged text retained outside the event log.
use super::*;
use crate::sandbox::store::{AgentMessageCheckpoint, AGENT_LOG_CAPACITY_FAILURE};
use crate::sandbox::{AgentLogKind, AgentStatus};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const RETAINED_OUTPUT_CHUNK_BYTES: usize = 512 * 1024;

/// Derived from the existing atomic private-prefix settlement, never authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRetainedOutputAvailability {
    pub available: bool,
}
impl AgentRetainedOutputAvailability {
    /// Only this exact server-owned notice proves a private prefix. The capacity
    /// class also covers failures whose entire prefix is already public.
    pub fn from_failure(failure: &crate::sandbox::TypedFailure) -> Option<Self> {
        const NOTICE: &str = "This Agent cannot continue because its conversation history is full. Some partial output could not be saved in the conversation history. It is retained privately; contact support to recover it through an operator export.";
        (failure.class == AGENT_LOG_CAPACITY_FAILURE && failure.detail == NOTICE)
            .then_some(Self { available: true })
    }

    pub fn from_outcome(outcome: &crate::sandbox::AgentTurnOutcome) -> Option<Self> {
        match outcome {
            crate::sandbox::AgentTurnOutcome::Failed { failure, .. } => Self::from_failure(failure),
            _ => None,
        }
    }
}

/// An immutable partial message, never a new event or a completed answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRetainedOutput {
    pub id: String,
    pub agent_id: AgentId,
    pub turn_sequence: u64,
    pub checkpoint_sequence: u64,
    pub failure_sequence: u64,
    pub failed_at: String,
    pub byte_length: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRetainedOutputPage {
    pub output: Option<AgentRetainedOutput>,
    pub offset: usize,
    pub content: String,
    pub next: Option<String>,
}

impl AgentReadService {
    pub async fn retained_output(
        &self,
        caller: &Caller,
        id: &AgentId,
        cursor: Option<&str>,
    ) -> Result<AgentRetainedOutputPage, AgentReadError> {
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let (lineage, held) = self
                .owner_projection_authority(caller, id, crate::RuntimeCapability::Conversation)
                .await?;
            let result = self
                .read_retained_output(&lineage.agent, None, cursor)
                .await;
            self.finish_owner_projection(
                caller,
                id,
                &lineage,
                held,
                crate::RuntimeCapability::Conversation,
            )
            .await?;
            result
        })
        .await
        .map_err(|_| AgentReadError::Unavailable("retained output read timed out".into()))?
    }

    /// Each chunk uses current history authority and a release fence, including
    /// errors. The optional turn restricts a caller's already-authorized binding.
    pub async fn retained_output_for_stream(
        &self,
        authority: &AgentEventStreamAuthorization,
        turn: Option<u64>,
        cursor: Option<&str>,
    ) -> Result<AgentRetainedOutputPage, AgentReadError> {
        let result = authority
            .bounded(async {
                let agent = self
                    .current_event_stream_snapshot(authority)
                    .await?
                    .lineage
                    .agent
                    .ok_or(AgentReadError::NotFound)?;
                require_granted(&agent, crate::RuntimeCapability::Conversation)?;
                self.read_retained_output(&agent, turn, cursor).await
            })
            .await;
        self.validate_event_stream(authority).await?;
        result
    }

    async fn read_retained_output(
        &self,
        agent: &Agent,
        turn: Option<u64>,
        cursor: Option<&str>,
    ) -> Result<AgentRetainedOutputPage, AgentReadError> {
        let Some(prefix) = self
            .store
            .retained_agent_output(agent)
            .await
            .map_err(store_error)?
        else {
            return absent(cursor);
        };
        prefix.validate().map_err(store_error)?;
        let original_turn = prefix
            .active_turn
            .ok_or_else(|| corrupt("retained output has no turn"))?;
        if prefix.assembly.content.is_empty()
            || original_turn > prefix.after_seq
            || prefix.after_seq >= agent.conversation.last_seq
            || prefix.outcome.turn_closed
            || prefix
                .outcome
                .appended
                .iter()
                .any(|entry| entry.agent_id != agent.id)
        {
            return Err(corrupt(
                "retained output checkpoint does not match its Agent",
            ));
        }
        if turn.is_some_and(|turn| turn != original_turn) {
            return absent(cursor);
        }
        let mut budget = 8 * 1024 * 1024;
        let start = self
            .retained_entry(agent, original_turn, &mut budget)
            .await?;
        if !matches!(start.kind, AgentLogKind::TurnStarted { turn_sequence, .. } if turn_sequence == original_turn)
        {
            return Err(corrupt("retained output turn evidence is missing"));
        }
        // Capacity settlement appends only bounded usage/first-input facts,
        // then the original failure and its status transition. Do not use the
        // current failure: subsequent recovery may have changed it.
        let mut failure = None;
        for sequence in prefix.after_seq + 1
            ..=agent
                .conversation
                .last_seq
                .min(prefix.after_seq.saturating_add(64))
        {
            let entry = self.retained_entry(agent, sequence, &mut budget).await?;
            match entry.kind {
                AgentLogKind::Usage { .. }
                | AgentLogKind::UsageUpdated { .. }
                | AgentLogKind::FirstMessageChanged { .. }
                    if failure.is_none() => {}
                AgentLogKind::TurnFailed {
                    turn_sequence,
                    failure: ref reason,
                } if failure.is_none()
                    && turn_sequence == original_turn
                    && reason.class == AGENT_LOG_CAPACITY_FAILURE =>
                {
                    failure = Some((sequence, entry.appended_at));
                }
                AgentLogKind::StatusChanged {
                    from: AgentStatus::Working | AgentStatus::WaitingApproval,
                    to: AgentStatus::Failed,
                } if failure
                    .as_ref()
                    .is_some_and(|(seq, at)| *seq + 1 == sequence && at == &entry.appended_at) =>
                {
                    let (failure_sequence, failed_at) = failure.expect("checked failure");
                    let output = AgentRetainedOutput {
                        id: identity(agent, &prefix)?,
                        agent_id: agent.id.clone(),
                        turn_sequence: original_turn,
                        checkpoint_sequence: prefix.after_seq,
                        failure_sequence,
                        failed_at,
                        byte_length: prefix.assembly.content.len(),
                    };
                    return chunk(output, &prefix.assembly.content, cursor);
                }
                _ => {
                    return Err(corrupt(
                        "retained output capacity failure evidence is invalid",
                    ))
                }
            }
        }
        Err(corrupt(
            "retained output capacity failure evidence is missing",
        ))
    }

    async fn retained_entry(
        &self,
        agent: &Agent,
        sequence: u64,
        budget: &mut usize,
    ) -> Result<AgentLogEntry, AgentReadError> {
        let entry = self
            .store
            .agent_log_entry(&agent.id, sequence)
            .await
            .map_err(log_store_error)?
            .ok_or_else(|| corrupt("retained output evidence contains a gap"))?;
        if entry.agent_id != agent.id
            || entry.seq != sequence
            || crate::seconds_from_rfc3339(&entry.appended_at).is_none()
        {
            return Err(corrupt("retained output evidence identity is invalid"));
        }
        let size = serde_json::to_vec(&entry)
            .map_err(|_| corrupt("invalid retained output evidence"))?
            .len();
        *budget = budget
            .checked_sub(size)
            .ok_or_else(|| corrupt("retained output evidence exceeds its byte bound"))?;
        Ok(entry)
    }
}

fn absent(cursor: Option<&str>) -> Result<AgentRetainedOutputPage, AgentReadError> {
    if cursor.is_some() {
        Err(AgentReadError::InvalidPage)
    } else {
        Ok(AgentRetainedOutputPage::default())
    }
}

fn identity(agent: &Agent, prefix: &AgentMessageCheckpoint) -> Result<String, AgentReadError> {
    // Hash all checkpoint fields without exposing private runtime identities or
    // receipt metadata. This is an immutable coordinate, never authorization.
    let bytes = serde_json::to_vec(&(&agent.id, &agent.created_at, prefix))
        .map_err(|_| corrupt("invalid retained output checkpoint"))?;
    Ok(format!("retained_{:x}", Sha256::digest(bytes)))
}

fn chunk(
    output: AgentRetainedOutput,
    content: &str,
    cursor: Option<&str>,
) -> Result<AgentRetainedOutputPage, AgentReadError> {
    let offset = match cursor {
        None => 0,
        Some(cursor) => {
            let (id, offset) = cursor.split_once(':').ok_or(AgentReadError::InvalidPage)?;
            if id != output.id {
                return Err(AgentReadError::InvalidPage);
            }
            offset
                .parse::<usize>()
                .map_err(|_| AgentReadError::InvalidPage)?
        }
    };
    if offset >= content.len() || !content.is_char_boundary(offset) {
        return Err(AgentReadError::InvalidPage);
    }
    let mut end = content
        .len()
        .min(offset.saturating_add(RETAINED_OUTPUT_CHUNK_BYTES));
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    Ok(AgentRetainedOutputPage {
        next: (end < content.len()).then(|| format!("{}:{end}", output.id)),
        output: Some(output),
        offset,
        content: content[offset..end].into(),
    })
}
