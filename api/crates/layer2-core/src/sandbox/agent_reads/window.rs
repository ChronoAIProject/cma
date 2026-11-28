use super::*;
use crate::sandbox::agent_io::{AgentEvent, AgentEventKind};
use crate::sandbox::AgentLogKind;
use serde::Serialize;

/// Current safe coordinates. Runtime generation is for the server's live guard.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub retained_output: Option<super::AgentRetainedOutputAvailability>,
    pub sequence: u64,
    pub appended_at: String,
    pub kind: Option<AgentEventKind>,
    pub response_binding: Option<AgentResponseBinding>,
    pub message_command_id: Option<crate::ids::AgentCommandId>,
    pub(in crate::sandbox) command_candidate: Option<String>,
    pub(in crate::sandbox) steering_operation: Option<u64>,
}

pub struct AgentReplayWindow {
    pub header: AgentEventStreamHeader,
    pub through: u64,
    pub entries: Vec<AgentReplayEntry>,
}

pub(super) struct WindowRead {
    pub after: u64,
    pub through: Option<u64>,
    pub limit: u16,
    pub suffix: bool,
    pub shared_tail_proof: bool,
}

impl AgentReadService {
    /// One bounded current authorization snapshot, with no log IO. Callers may
    /// use it to fence a live batch or detect the need for proved replay.
    pub async fn current_event_stream_header(
        &self,
        authorization: &AgentEventStreamAuthorization,
    ) -> Result<AgentEventStreamHeader, AgentReadError> {
        authorization
            .bounded(async {
                let snapshot = self.current_event_stream_snapshot(authorization).await?;
                let agent = snapshot
                    .lineage
                    .agent
                    .ok_or_else(|| corrupt("authorized event snapshot has no Agent"))?;
                header(&agent)
            })
            .await
    }

    /// The predecessor was verified in the same page snapshot; no extra source
    /// scan or new authority decision is needed to correlate this projection.
    pub(in crate::sandbox) async fn steering_operation(
        &self,
        _authorization: &AgentEventStreamAuthorization,
        entry: &AgentReplayEntry,
    ) -> Result<Option<u64>, AgentReadError> {
        Ok(entry.steering_operation)
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
        self.read_event_window(authorization, after, through, limit, false)
            .await
    }

    /// Keep only a bounded suffix ending at the requested immutable cursor.
    /// Byte trimming moves the lower boundary, never the high-water mark.
    pub async fn event_history_window(
        &self,
        authorization: &AgentEventStreamAuthorization,
        through: u64,
        limit: u16,
    ) -> Result<AgentReplayWindow, AgentReadError> {
        self.read_event_window(
            authorization,
            through.saturating_sub(u64::from(limit)),
            Some(through),
            limit,
            true,
        )
        .await
    }

    async fn read_event_window(
        &self,
        authorization: &AgentEventStreamAuthorization,
        after: u64,
        through: Option<u64>,
        limit: u16,
        suffix: bool,
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
                self.project_event_window(
                    authorization,
                    &agent,
                    WindowRead {
                        after,
                        through,
                        limit,
                        suffix,
                        shared_tail_proof: false,
                    },
                )
                .await
            })
            .await;
        self.validate_event_stream(authorization).await?;
        result
    }

    pub(super) async fn project_event_window(
        &self,
        authorization: &AgentEventStreamAuthorization,
        agent: &Agent,
        request: WindowRead,
    ) -> Result<AgentReplayWindow, AgentReadError> {
        let WindowRead {
            after,
            through,
            limit,
            suffix,
            shared_tail_proof,
        } = request;
        let header = header(agent)?;
        let through = through.unwrap_or(header.head);
        if after > through || through > header.head {
            return Err(AgentReadError::InvalidPage);
        }
        let count = (through - after).min(u64::from(limit)) as u16;
        let mut projected = std::collections::VecDeque::with_capacity(usize::from(count));
        let mut bytes = if suffix { 16 * 1024 } else { 2048 };
        let mut seen = 0_u64;
        let source_after = after.saturating_sub(1);
        let mut previous_fence: Option<(u64, String)> = None;
        let mut full = false;
        let mut visit = |entry: AgentLogEntry| {
            seen += 1;
            if entry.agent_id != agent.id || entry.seq != source_after + seen {
                return Err(SandboxStoreError::SequenceConflict);
            }
            let steering_operation = if matches!(entry.kind, AgentLogKind::OwnerMessage { .. }) {
                match previous_fence.take() {
                    Some((sequence, ref at))
                        if sequence.checked_add(1) == Some(entry.seq)
                            && at == &entry.appended_at =>
                    {
                        Some(sequence)
                    }
                    Some(_) => {
                        return Err(SandboxStoreError::Corrupt(
                            "steering message has no matching operation fence".into(),
                        ))
                    }
                    None => None,
                }
            } else {
                None
            };
            previous_fence = match entry.kind {
                AgentLogKind::OperationDeliveryChanged {
                    operation_sequence,
                    operation: super::super::AgentOperationKind::Steer,
                    state: super::super::AgentOperationDelivery::DispatchFenced,
                } => Some((operation_sequence, entry.appended_at.clone())),
                _ => None,
            };
            if entry.seq <= after || full {
                return Ok(());
            }
            let command_candidate = match &entry.kind {
                AgentLogKind::TurnStarted {
                    origin_key: Some(origin),
                    ..
                } => origin.strip_prefix("command:").map(str::to_owned),
                _ => None,
            };
            let visible = AgentVisibleLogEntry::from_entry(entry, authorization.resource().2);
            let kind = visible.kind.and_then(|kind| {
                AgentEvent::try_from(AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq: visible.seq,
                    appended_at: visible.appended_at.clone(),
                    kind,
                })
                .ok()
                .map(|event| event.kind)
            });
            let size = serde_json::to_vec(&(visible.seq, &visible.appended_at, &kind))
                .map_err(|_| {
                    SandboxStoreError::Corrupt("event projection is not serializable".into())
                })?
                .len()
                + 512
                + command_candidate.as_ref().map_or(0, String::len);
            if suffix {
                while bytes + size > 4 * 1024 * 1024 {
                    let Some((removed, _)) = projected.pop_front() else {
                        break;
                    };
                    bytes -= removed;
                }
            }
            if bytes + size > 4 * 1024 * 1024 {
                if projected.is_empty() {
                    return Err(SandboxStoreError::Corrupt(
                        "event exceeds the replay byte bound".into(),
                    ));
                }
                full = true;
                return Ok(());
            }
            bytes += size;
            projected.push_back((
                size,
                AgentReplayEntry {
                    retained_output: visible.retained_output,
                    sequence: visible.seq,
                    appended_at: visible.appended_at,
                    kind,
                    response_binding: None,
                    message_command_id: None,
                    command_candidate,
                    steering_operation,
                },
            ));
            Ok(())
        };
        self.visit_observed_page(
            agent,
            after,
            through,
            count.max(1),
            shared_tail_proof,
            &mut visit,
        )
        .await
        .map_err(log_store_error)?;
        if seen != u64::from(count) + u64::from(after > 0) {
            return Err(corrupt("agent log ended before its high-water mark"));
        }
        if !shared_tail_proof {
            self.reject_uncommitted_tail(agent, header.head).await?;
        }
        Ok(AgentReplayWindow {
            header,
            through,
            entries: projected.into_iter().map(|(_, entry)| entry).collect(),
        })
    }
}

pub(super) fn header(agent: &Agent) -> Result<AgentEventStreamHeader, AgentReadError> {
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
