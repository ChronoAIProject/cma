//! One captured snapshot per bounded output observation; never a history fold.
use super::*;
use crate::store::agent_log_integrity::TransactionLogError;
use crate::store::agent_log_segments::{capture, projection, read_format};
use layer2_core::sandbox::{
    agent_output_index::{AgentOutputChange, AgentToolOutputState},
    store::indexed::{
        AgentOutputMetadata, AgentOutputReader, OUTPUT_PAGE_CHANGES, OUTPUT_PAGE_POINTS,
    },
    AgentLogEntry,
};
use mongodb::{
    bson::{doc, Bson},
    options::ReadConcern,
    ClientSession,
};

mod facts;
mod output;
mod points;
mod turn;

struct Reader<'a> {
    store: &'a MongoSandboxStore,
    session: ClientSession,
    captured: capture::CapturedLog,
    metadata: AgentOutputMetadata,
    message_checkpoint: Option<Bson>,
}

pub(super) async fn open<'a>(
    store: &'a MongoSandboxStore,
    agent: &Agent,
    turn: Option<u64>,
) -> Result<Option<Box<dyn AgentOutputReader + 'a>>, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(driver)?;
    let mut row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    let snapshot_agent = crate::store::agents::read_agent(&row)?;
    if read_format(&row)?.is_none() {
        // The legacy reader independently checks lineage and reads the
        // authorized prefix. A newer legacy revision does not affect the
        // format decision; indexed reads require an exact captured Agent.
        return Ok(None);
    }
    let advanced = snapshot_agent != *agent;
    if advanced && !advance::may_have_advanced(agent, &snapshot_agent, turn) {
        return Err(SandboxStoreError::SequenceConflict);
    }
    let captured = capture::capture(
        store,
        &snapshot_agent,
        &mut session,
        capture::CaptureMode::Snapshot,
    )
    .await
    .map_err(transaction)?;
    let mut reader = Reader {
        store,
        session,
        captured,
        message_checkpoint: row.remove(crate::store::agent_conversation_store::checkpoint::FIELD),
        metadata: AgentOutputMetadata {
            agent_id: snapshot_agent.id.clone(),
            through: snapshot_agent.conversation.last_seq,
            created_at: snapshot_agent.created_at.clone(),
            runtime_generation: snapshot_agent.conversation.runtime_generation,
            active_turn: snapshot_agent.conversation.active_turn,
            initial_model: snapshot_agent.profile.content.model.clone(),
            turn: None,
            index: None,
        },
    };
    if let Some(turn) = turn {
        reader.load_turn(turn).await?;
    }
    if advanced {
        let sequences: Vec<_> =
            (agent.conversation.last_seq + 1..=snapshot_agent.conversation.last_seq).collect();
        let rows = reader.source_points(&sequences).await?;
        if let Some(open_bytes) = reader
            .metadata
            .index
            .as_ref()
            .map(|index| &index.totals.text)
            .filter(|text| text.is_open())
            .map(|text| text.open_bytes as usize)
        {
            if reader.open_message().await?.len() != open_bytes {
                return Err(corrupt());
            }
        }
        return Err(advance::classify(agent, &snapshot_agent, turn, &rows));
    }
    Ok(Some(Box::new(reader)))
}

#[async_trait::async_trait]
impl AgentOutputReader for Reader<'_> {
    fn metadata(&self) -> &AgentOutputMetadata {
        &self.metadata
    }
    async fn changes(
        &mut self,
        from: u64,
        limit: u16,
    ) -> Result<Vec<AgentOutputChange>, SandboxStoreError> {
        self.output_changes(from, limit).await
    }
    async fn tool(&mut self, call: &str) -> Result<AgentToolOutputState, SandboxStoreError> {
        let turn = self
            .metadata
            .index
            .as_ref()
            .ok_or_else(corrupt)?
            .turn_sequence;
        let key = format!("{turn}:{}:{call}", call.len());
        let record = projection::evidence(
            self.store,
            self.captured.active_head(),
            "tool_output",
            &key,
            &mut self.session,
        )
        .await
        .map_err(transaction)?
        .ok_or_else(corrupt)?;
        let projection::EvidenceRecord::ToolOutput { tool } = record else {
            return Err(corrupt());
        };
        if tool.call_id != call || tool.turn_sequence != turn {
            return Err(corrupt());
        }
        Ok(tool)
    }
    async fn points(&mut self, sequences: &[u64]) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        self.source_points(sequences).await
    }
    async fn open_message(&mut self) -> Result<String, SandboxStoreError> {
        let checkpoint: layer2_core::sandbox::store::AgentMessageCheckpoint =
            mongodb::bson::deserialize_from_bson(
                self.message_checkpoint.clone().ok_or_else(corrupt)?,
            )
            .map_err(|_| corrupt())?;
        checkpoint.validate()?;
        if checkpoint.after_seq != self.metadata.through
            || checkpoint.active_turn != self.metadata.active_turn
            || Some(checkpoint.runtime_generation) != self.metadata.runtime_generation
            || self
                .metadata
                .turn
                .as_ref()
                .is_none_or(|turn| turn.facts.conversation_id != checkpoint.native_conversation_id)
        {
            return Err(corrupt());
        }
        checkpoint
            .assembly
            .published_text()
            .map(str::to_owned)
            .ok_or_else(corrupt)
    }
    async fn source_page(
        &mut self,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        let index = self.metadata.index.as_ref().ok_or_else(corrupt)?;
        if limit == 0
            || usize::from(limit) > OUTPUT_PAGE_POINTS
            || after < index.turn_sequence.saturating_sub(1)
            || after > index.through_sequence
        {
            return Err(SandboxStoreError::InvalidPage);
        }
        let end = index
            .through_sequence
            .min(after.saturating_add(u64::from(limit)));
        if end == after {
            return Ok(vec![]);
        }
        let sequences: Vec<_> = (after + 1..=end).collect();
        self.source_points(&sequences).await
    }
}

fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("indexed Agent output evidence is inconsistent".into())
}
fn driver(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
fn transaction(error: TransactionLogError) -> SandboxStoreError {
    match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => driver(error),
    }
}
