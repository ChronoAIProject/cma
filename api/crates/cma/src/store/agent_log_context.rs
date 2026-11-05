//! Explicit bounded mutation context; never presents an active tail as a full log.
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, AgentLogEntry, AgentLogKind};
use mongodb::{bson::doc, options::ReadConcern, ClientSession};

use super::agent_log_integrity::{scan_complete_log, LogSummary, TransactionLogError};
use super::agent_log_segments::{self as segments, projection};
use super::sandbox_mongo::MongoSandboxStore;

pub(super) struct LogContext {
    pub summary: LogSummary,
    pub last: Option<AgentLogEntry>,
    pub latest_turn: Option<u64>,
    pub latest_generation: Option<u64>,
    pub has_owner_input: bool,
    head: Option<segments::Head>,
    initial: Option<AgentLogEntry>,
}

impl LogContext {
    pub(super) async fn initial_entry(
        &self,
        store: &MongoSandboxStore,
        session: &mut ClientSession,
        snapshot: bool,
    ) -> Result<Option<AgentLogEntry>, TransactionLogError> {
        let Some(head) = &self.head else {
            return Ok(self.initial.clone());
        };
        if head.last_seq == 0 {
            return Ok(None);
        }
        let mut initial = None;
        segments::visit_range(
            store,
            head,
            segments::LogRange {
                after: 0,
                through: 1,
                limit: 1,
            },
            session,
            snapshot,
            |entry| {
                initial = Some(entry);
                Ok(())
            },
        )
        .await?;
        Ok(initial)
    }
}

pub(super) async fn validate(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<LogContext, TransactionLogError> {
    let find = store.agents.find_one(doc! {"_id":agent.id.as_str()});
    let find = if snapshot {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    let aggregate = find
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    if super::agents::read_agent(&aggregate)? != *agent {
        return Err(SandboxStoreError::Conflict.into());
    }
    let mut last = None;
    if segments::read_format(&aggregate)?.is_some() {
        let head = segments::read_head(store, &aggregate, session, snapshot).await?;
        let state = projection::read(store, agent, &head, session).await?;
        segments::visit_range(
            store,
            &head,
            segments::LogRange {
                after: (head.active_start_seq - 1) as u64,
                through: head.last_seq as u64,
                limit: (head.active_count as u16).max(1),
            },
            session,
            snapshot,
            |entry| {
                last = Some(entry);
                Ok(())
            },
        )
        .await?;
        // A storage-only recovery may leave the successor empty. Fetch the
        // exact previous event when cleanup needs its retained failure marker.
        if last.is_none() && head.last_seq > 0 {
            segments::visit_range(
                store,
                &head,
                segments::LogRange {
                    after: head.last_seq as u64 - 1,
                    through: head.last_seq as u64,
                    limit: 1,
                },
                session,
                snapshot,
                |entry| {
                    last = Some(entry);
                    Ok(())
                },
            )
            .await?;
        }
        return Ok(LogContext {
            summary: LogSummary::segmented(head.active_count, head.active_bytes, head.last_seq)?,
            last,
            latest_turn: state.latest_turn,
            latest_generation: state.latest_conversation.map(|(_, generation)| generation),
            has_owner_input: state.has_owner_input,
            head: Some(head),
            initial: None,
        });
    }
    let mut latest_turn = None;
    let mut latest_generation = None;
    let mut has_owner_input = false;
    let mut initial = None;
    let summary = scan_complete_log(
        store,
        &agent.id,
        agent.conversation.last_seq,
        session,
        snapshot,
        |entry| {
            if initial.is_none() {
                initial = Some(entry.clone());
            }
            has_owner_input |= matches!(entry.kind, AgentLogKind::OwnerMessage { .. });
            match &entry.kind {
                AgentLogKind::TurnStarted { turn_sequence, .. } => {
                    latest_turn = Some(*turn_sequence)
                }
                AgentLogKind::ConversationOpened {
                    runtime_generation, ..
                }
                | AgentLogKind::RuntimeGenerationRetired {
                    runtime_generation, ..
                } => latest_generation = Some(*runtime_generation),
                _ => {}
            }
            last = Some(entry);
            Ok(())
        },
    )
    .await?;
    Ok(LogContext {
        summary,
        last,
        latest_turn,
        latest_generation,
        has_owner_input,
        head: None,
        initial,
    })
}
