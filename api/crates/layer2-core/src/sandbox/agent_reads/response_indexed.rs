//! An indexed observation cannot escape its original authority or IO deadline.
use super::*;
use crate::sandbox::agent_output_index::{AgentOutputChange, AgentToolOutputState};
use crate::sandbox::store::indexed::{
    AgentOutputMetadata, AgentOutputReader, OUTPUT_PAGE_CHANGES, OUTPUT_PAGE_POINTS,
};
use std::{future::Future, pin::Pin};

pub struct AgentOutputObservation<'a> {
    reader: Box<dyn AgentOutputReader + 'a>,
    personal: bool,
}

impl AgentOutputObservation<'_> {
    pub fn metadata(&self) -> &AgentOutputMetadata {
        self.reader.metadata()
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
        self.reader.tool(call).await.map_err(log_store_error)
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
        Ok(rows
            .into_iter()
            .map(|row| AgentVisibleLogEntry::from_entry(row, self.personal))
            .collect())
    }
}

pub type OutputReadFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, AgentReadError>> + Send + 'a>>;

impl AgentReadService {
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
    pub async fn with_output_observation<T: Send, F: Send>(
        &self,
        authorization: &AgentEventStreamAuthorization,
        turn: Option<u64>,
        operation: F,
    ) -> Result<Option<T>, AgentReadError>
    where
        F: for<'a, 'b> FnOnce(&'a mut AgentOutputObservation<'b>) -> OutputReadFuture<'a, T>,
    {
        let result = authorization
            .bounded(async {
                let current = self.current_event_stream_snapshot(authorization).await?;
                let agent = current
                    .lineage
                    .agent
                    .ok_or_else(|| corrupt("authorized output has no Agent"))?;
                let Some(store) = &self.response_logs else {
                    return Ok(None);
                };
                let Some(reader) = store
                    .agent_output_reader(&agent, turn)
                    .await
                    .map_err(log_store_error)?
                else {
                    return Ok(None);
                };
                let metadata = reader.metadata();
                if metadata.agent_id != agent.id
                    || metadata.through != agent.conversation.last_seq
                    || metadata.turn.as_ref().map(|t| t.facts.turn_sequence) != turn
                    || metadata.index.as_ref().map(|i| i.turn_sequence) != turn
                {
                    return Err(corrupt(
                        "indexed output does not match the authorized capture",
                    ));
                }
                let mut observation = AgentOutputObservation {
                    reader,
                    personal: authorization.resource().2,
                };
                operation(&mut observation).await.map(Some)
            })
            .await;
        // Fence failures too: a private dependency outcome never bypasses a
        // revocation or renews authority acquired by the original request.
        self.validate_event_stream(authorization).await?;
        result
    }
}
