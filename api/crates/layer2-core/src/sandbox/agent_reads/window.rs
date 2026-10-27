use super::*;
use crate::sandbox::agent_io::{AgentEvent, AgentEventKind};
use crate::sandbox::AgentLogKind;
use serde::Serialize;

/// Current safe coordinates. Runtime generation is for the server's live guard.
#[derive(Debug, Clone)]
pub struct AgentEventStreamHeader {
    pub id: AgentId,
    pub created_at: String,
    pub runtime_generation: Option<u64>,
    pub active_turn: Option<u64>,
    pub head: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentResponseBinding {
    pub command_id: crate::ids::AgentCommandId,
    pub response_id: crate::ids::AgentResponseId,
    pub turn_sequence: u64,
}

#[derive(Debug, Clone)]
pub struct AgentReplayEntry {
    pub sequence: u64,
    pub appended_at: String,
    pub kind: Option<AgentEventKind>,
    pub response_binding: Option<AgentResponseBinding>,
    pub message_command_id: Option<crate::ids::AgentCommandId>,
    pub(in crate::sandbox) command_candidate: Option<String>,
}

pub struct AgentReplayWindow {
    pub header: AgentEventStreamHeader,
    pub through: u64,
    pub entries: Vec<AgentReplayEntry>,
}

impl AgentReadService {
    /// Only the immediately preceding immutable steering fence can bind this message.
    pub(in crate::sandbox) async fn steering_operation(
        &self,
        authorization: &AgentEventStreamAuthorization,
        entry: &AgentReplayEntry,
    ) -> Result<Option<u64>, AgentReadError> {
        if entry.sequence < 2 {
            return Ok(None);
        }
        let (_, id, _) = authorization.resource();
        let previous = self
            .store
            .agent_log(id, entry.sequence - 2, 1)
            .await
            .map_err(log_store_error)?;
        let previous = previous
            .first()
            .ok_or_else(|| corrupt("message predecessor is missing"))?;
        if &previous.agent_id != id || previous.seq != entry.sequence - 1 {
            return Err(corrupt("message predecessor identity is invalid"));
        }
        match previous.kind {
            AgentLogKind::OperationDeliveryChanged {
                operation_sequence,
                operation: super::super::AgentOperationKind::Steer,
                state: super::super::AgentOperationDelivery::DispatchFenced,
            } => {
                if operation_sequence != previous.seq || previous.appended_at != entry.appended_at {
                    return Err(corrupt("steering message has no matching operation fence"));
                }
                Ok(Some(operation_sequence))
            }
            _ => Ok(None),
        }
    }

    pub async fn event_stream_header(
        &self,
        authorization: &AgentEventStreamAuthorization,
    ) -> Result<AgentEventStreamHeader, AgentReadError> {
        let result = authorization
            .bounded(async {
                let snapshot = self.current_event_stream_snapshot(authorization).await?;
                let agent = snapshot
                    .lineage
                    .agent
                    .ok_or_else(|| corrupt("authorized event snapshot has no Agent"))?;
                header(&agent)
            })
            .await;
        self.validate_event_stream(authorization).await?;
        result
    }

    /// Fetch only the requested consecutive interval, through a frozen head.
    /// A tail poll never folds or copies the preceding immutable log prefix.
    pub async fn event_stream_window(
        &self,
        authorization: &AgentEventStreamAuthorization,
        after: u64,
        through: Option<u64>,
        limit: u16,
    ) -> Result<AgentReplayWindow, AgentReadError> {
        let result = authorization
            .bounded(async {
                if !(1..=200).contains(&limit) {
                    return Err(AgentReadError::InvalidPage);
                }
                let snapshot = self.current_event_stream_snapshot(authorization).await?;
                let agent = snapshot
                    .lineage
                    .agent
                    .ok_or_else(|| corrupt("authorized event snapshot has no Agent"))?;
                let header = header(&agent)?;
                let through = through.unwrap_or(header.head);
                if after > through || through > header.head {
                    return Err(AgentReadError::InvalidPage);
                }
                let count = (through - after).min(u64::from(limit)) as u16;
                let mut projected = Vec::with_capacity(usize::from(count));
                let mut bytes = 2048;
                for offset in 0..u64::from(count) {
                    // Fetch one source row at a time: a row-count bound alone
                    // would still accumulate hundreds of large messages before
                    // the public page could enforce its serialized byte bound.
                    let mut entries = self
                        .store
                        .agent_log(&agent.id, after + offset, 1)
                        .await
                        .map_err(log_store_error)?;
                    if entries.len() != 1 {
                        return Err(corrupt("agent log ended before its high-water mark"));
                    }
                    let entry = entries.remove(0);
                    if entry.agent_id != agent.id || entry.seq != after + offset + 1 {
                        return Err(corrupt("agent log identity or sequence is invalid"));
                    }
                    let command_candidate = match &entry.kind {
                        AgentLogKind::TurnStarted {
                            origin_key: Some(origin),
                            ..
                        } => origin.strip_prefix("command:").map(str::to_owned),
                        _ => None,
                    };
                    let visible =
                        AgentVisibleLogEntry::from_entry(entry, authorization.resource().2);
                    let kind = visible.kind.map(|kind| {
                        AgentEvent::from(AgentLogEntry {
                            agent_id: agent.id.clone(),
                            seq: visible.seq,
                            appended_at: visible.appended_at.clone(),
                            kind,
                        })
                        .kind
                    });
                    let size = serde_json::to_vec(&(visible.seq, &visible.appended_at, &kind))
                        .map_err(|_| corrupt("event projection is not serializable"))?
                        .len()
                        + 512
                        + command_candidate.as_ref().map_or(0, String::len);
                    if bytes + size > 4 * 1024 * 1024 {
                        if projected.is_empty() {
                            return Err(corrupt("event exceeds the replay byte bound"));
                        }
                        break;
                    }
                    bytes += size;
                    projected.push(AgentReplayEntry {
                        sequence: visible.seq,
                        appended_at: visible.appended_at,
                        kind,
                        response_binding: None,
                        message_command_id: None,
                        command_candidate,
                    });
                }
                self.reject_uncommitted_tail(&agent, header.head).await?;
                Ok(AgentReplayWindow {
                    header,
                    through,
                    entries: projected,
                })
            })
            .await;
        self.validate_event_stream(authorization).await?;
        result
    }
}

fn header(agent: &Agent) -> Result<AgentEventStreamHeader, AgentReadError> {
    if agent.conversation.last_seq == 0 {
        return Err(corrupt("agent log has no creation record"));
    }
    Ok(AgentEventStreamHeader {
        id: agent.id.clone(),
        created_at: agent.created_at.clone(),
        runtime_generation: agent.conversation.runtime_generation,
        active_turn: agent.conversation.active_turn,
        head: agent.conversation.last_seq,
    })
}
