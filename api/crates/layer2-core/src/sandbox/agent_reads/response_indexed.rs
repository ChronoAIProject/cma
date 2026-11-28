//! An indexed observation cannot escape its original authority or IO deadline.
use super::*;
use crate::sandbox::agent_output_index::{AgentOutputChange, AgentToolOutputState};
use crate::sandbox::store::indexed::{
    AgentOutputMetadata, AgentOutputReader, OUTPUT_PAGE_CHANGES, OUTPUT_PAGE_POINTS,
};
use std::{future::Future, pin::Pin};

#[path = "response_indexed_validation.rs"]
mod validation;

pub struct AgentOutputObservation<'a> {
    reader: Box<dyn AgentOutputReader + 'a>,
    personal: bool,
}

impl AgentOutputObservation<'_> {
    pub fn metadata(&self) -> &AgentOutputMetadata {
        self.reader.metadata()
    }
    pub async fn open_message(&mut self) -> Result<String, AgentReadError> {
        let index = self
            .metadata()
            .index
            .as_ref()
            .ok_or_else(|| corrupt("missing turn"))?;
        if self.metadata().active_turn != Some(index.turn_sequence) || !index.totals.text.is_open()
        {
            return Err(corrupt("output turn has no open message"));
        }
        let bytes = index.totals.text.open_bytes as usize;
        let content = self.reader.open_message().await.map_err(log_store_error)?;
        if content.len() != bytes {
            return Err(corrupt("open message does not match its certified extent"));
        }
        Ok(content)
    }
    pub async fn changes(
        &mut self,
        from: u64,
        limit: u16,
    ) -> Result<Vec<AgentOutputChange>, AgentReadError> {
        if limit == 0 || limit > OUTPUT_PAGE_CHANGES {
            return Err(corrupt("invalid output page bound"));
        }
        let result = self
            .reader
            .changes(from, limit)
            .await
            .map_err(log_store_error)?;
        let index = self
            .metadata()
            .index
            .as_ref()
            .ok_or_else(|| corrupt("output page has no turn"))?;
        let expected = index.changes.saturating_sub(from).min(u64::from(limit));
        if from > index.changes || result.len() as u64 != expected {
            return Err(corrupt("output page has incomplete coverage"));
        }
        for (offset, change) in result.iter().enumerate() {
            validation::change(change, index)?;
            if change.ordinal != from + offset as u64
                || change.turn_sequence != index.turn_sequence
                || change.source_sequence > index.through_sequence
            {
                return Err(corrupt(
                    "output change does not match its certified interval",
                ));
            }
        }
        if result
            .windows(2)
            .any(|pair| pair[0].after != pair[1].before)
        {
            return Err(corrupt("output page counters are not consecutive"));
        }
        Ok(result)
    }
    pub async fn tool(&mut self, call: &str) -> Result<AgentToolOutputState, AgentReadError> {
        let value = self.reader.tool(call).await.map_err(log_store_error)?;
        validation::tool(
            &value,
            call,
            self.metadata()
                .index
                .as_ref()
                .ok_or_else(|| corrupt("tool has no turn"))?,
        )?;
        Ok(value)
    }
    pub async fn points(
        &mut self,
        sequences: &[u64],
    ) -> Result<Vec<AgentVisibleLogEntry>, AgentReadError> {
        if sequences.is_empty()
            || sequences.len() > OUTPUT_PAGE_POINTS
            || sequences.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(corrupt("invalid original output references"));
        }
        let rows = self
            .reader
            .points(sequences)
            .await
            .map_err(log_store_error)?;
        if rows.len() != sequences.len()
            || rows.iter().zip(sequences).any(|(row, seq)| {
                row.seq != *seq
                    || row.agent_id != self.metadata().agent_id
                    || row.seq > self.metadata().through
            })
        {
            return Err(corrupt(
                "original output points do not match their references",
            ));
        }
        for row in &rows {
            validation::point(self.metadata(), row)?;
        }
        Ok(rows
            .into_iter()
            .map(|row| AgentVisibleLogEntry::from_entry(row, self.personal))
            .collect())
    }
    pub async fn source_page(
        &mut self,
        after: u64,
        limit: u16,
    ) -> Result<Vec<AgentVisibleLogEntry>, AgentReadError> {
        if limit == 0 || usize::from(limit) > OUTPUT_PAGE_POINTS {
            return Err(corrupt("invalid original source page bound"));
        }
        let rows = self
            .reader
            .source_page(after, limit)
            .await
            .map_err(log_store_error)?;
        let index = self
            .metadata()
            .index
            .as_ref()
            .ok_or_else(|| corrupt("source page has no turn"))?;
        if after < index.turn_sequence.saturating_sub(1)
            || after > index.through_sequence
            || rows.len() as u64 != (index.through_sequence - after).min(u64::from(limit))
            || rows.iter().enumerate().any(|(offset, row)| {
                row.agent_id != self.metadata().agent_id || row.seq != after + offset as u64 + 1
            })
        {
            return Err(corrupt("original source page is not consecutive"));
        }
        for row in &rows {
            validation::point(self.metadata(), row)?;
        }
        Ok(rows
            .into_iter()
            .map(|row| AgentVisibleLogEntry::from_entry(row, self.personal))
            .collect())
    }
}

pub type OutputReadFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, AgentReadError>> + Send + 'a>>;

impl AgentReadService {
    /// Personal compatibility reads retain their actual authenticated caller.
    /// They neither synthesize a resource credential nor create a shared grant.
    pub async fn with_owned_output_observation<T: Send, F>(
        &self,
        caller: &Caller,
        id: &AgentId,
        turn: u64,
        operation: F,
    ) -> Result<Option<T>, AgentReadError>
    where
        F: Send + for<'a, 'b> FnOnce(&'a mut AgentOutputObservation<'b>) -> OutputReadFuture<'a, T>,
    {
        require(caller)?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        let original = tokio::time::timeout_at(deadline, self.owned_lineage(caller, id))
            .await
            .map_err(|_| AgentReadError::Unavailable("output authorization timed out".into()))??;
        let result = tokio::time::timeout_at(deadline, async {
            let Some(store) = &self.response_logs else {
                return Ok(None);
            };
            let Some(reader) = store
                .agent_output_reader(&original.agent, Some(turn))
                .await
                .map_err(log_store_error)?
            else {
                return Ok(None);
            };
            validation::metadata(reader.metadata(), &original.agent, Some(turn))?;
            let mut observation = AgentOutputObservation {
                reader,
                personal: true,
            };
            operation(&mut observation).await.map(Some)
        })
        .await
        .map_err(|_| AgentReadError::Unavailable("output read timed out".into()))?;
        let current = tokio::time::timeout_at(deadline, self.owned_lineage(caller, id))
            .await
            .map_err(|_| AgentReadError::Unavailable("output release timed out".into()))??;
        if current.agent.created_at != original.agent.created_at
            || current.sandbox.id != original.sandbox.id
            || current.sandbox.created_at != original.sandbox.created_at
            || current.workspace.id != original.workspace.id
            || current.workspace.created_at != original.workspace.created_at
            || current.worktree.id != original.worktree.id
        {
            return Err(AgentReadError::NotFound);
        }
        result
    }
    pub async fn output_is_indexed(
        &self,
        authorization: &AgentEventStreamAuthorization,
    ) -> Result<bool, AgentReadError> {
        let result = authorization
            .bounded(async {
                let current = self.current_event_stream_snapshot(authorization).await?;
                let agent = current
                    .lineage
                    .agent
                    .ok_or_else(|| corrupt("authorized output has no Agent"))?;
                match &self.response_logs {
                    Some(store) => store
                        .agent_output_available(&agent)
                        .await
                        .map_err(log_store_error),
                    None => Ok(false),
                }
            })
            .await;
        self.validate_event_stream(authorization).await?;
        result
    }
    /// Runs all indexed probes/points for one page or bounded result inside one
    /// IO deadline. The store snapshot is dropped before releasing the result.
    pub async fn with_output_observation<T: Send, F>(
        &self,
        authorization: &AgentEventStreamAuthorization,
        turn: Option<u64>,
        operation: F,
    ) -> Result<Option<T>, AgentReadError>
    where
        F: Send + for<'a, 'b> FnOnce(&'a mut AgentOutputObservation<'b>) -> OutputReadFuture<'a, T>,
    {
        let result = authorization
            .bounded(async {
                let current = self.current_event_stream_snapshot(authorization).await?;
                let agent = current
                    .lineage
                    .agent
                    .ok_or_else(|| corrupt("authorized output has no Agent"))?;
                self.read_output_observation(&agent, turn, authorization.resource().2, operation)
                    .await
            })
            .await;
        // Fence failures too: a private dependency outcome never bypasses a
        // revocation or renews authority acquired by the original request.
        self.validate_event_stream(authorization).await?;
        result
    }

    pub(super) async fn read_output_observation<T: Send, F>(
        &self,
        agent: &Agent,
        turn: Option<u64>,
        personal: bool,
        operation: F,
    ) -> Result<Option<T>, AgentReadError>
    where
        F: Send + for<'a, 'b> FnOnce(&'a mut AgentOutputObservation<'b>) -> OutputReadFuture<'a, T>,
    {
        let Some(store) = &self.response_logs else {
            return Ok(None);
        };
        let flight = match &self.observation {
            Some(service) => service
                .output_flight(agent, turn)
                .await
                .map_err(log_store_error)?,
            None => None,
        };
        if let Some(flight) = &flight {
            let Some(reader) =
                super::super::output_memo::MemoReader::open(store.as_ref(), agent, turn, flight)
                    .await?
            else {
                return Ok(None);
            };
            validation::metadata(reader.metadata(), agent, turn)?;
            let commit = reader.commit();
            let mut observation = AgentOutputObservation {
                reader: Box::new(reader),
                personal,
            };
            let result = operation(&mut observation).await?;
            commit(flight);
            return Ok(Some(result));
        }
        let Some(reader) = store
            .agent_output_reader(agent, turn)
            .await
            .map_err(log_store_error)?
        else {
            return Ok(None);
        };
        validation::metadata(reader.metadata(), agent, turn)?;
        let mut observation = AgentOutputObservation { reader, personal };
        operation(&mut observation).await.map(Some)
    }
}
