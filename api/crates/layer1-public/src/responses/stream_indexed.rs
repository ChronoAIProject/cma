use super::*;
use layer2_core::sandbox::{AgentLogEntry, AgentReadError};

impl Open {
    pub(super) async fn read_indexed(
        &mut self,
    ) -> Result<(Agent, ResponseObject, Vec<AgentLogEntry>, bool), AgentReadError> {
        let held = self
            .authorization
            .as_ref()
            .ok_or(AgentReadError::NotFound)?;
        let agent = self.agent.clone();
        let turn = self.turn;
        let after = self.cursor;
        let origin = self.state.agent_page_origin.clone();
        let result = self
            .state
            .reads
            .with_output_observation(held, Some(turn), move |source| {
                Box::pin(async move {
                    let mut agent = agent;
                    agent.conversation.last_seq = source.metadata().through;
                    agent.conversation.runtime_generation = source.metadata().runtime_generation;
                    agent.conversation.active_turn = source.metadata().active_turn;
                    agent.profile.content.model = source.metadata().initial_model.clone();
                    let mut response =
                        retrieve::indexed::metadata(source, &origin, &agent, turn).await?;
                    let rows = source.source_page(after, 32).await?;
                    let cursor = rows.last().map_or(after, |row| row.seq);
                    let through = source
                        .metadata()
                        .index
                        .as_ref()
                        .ok_or(AgentReadError::NotFound)?
                        .through_sequence;
                    let more = cursor < through;
                    let mut pagination = false;
                    if !more && response.status != ResponseStatus::InProgress {
                        match retrieve::indexed::complete(source, &origin, &agent, turn).await? {
                            Some(complete) => response = complete,
                            None => pagination = true,
                        }
                    }
                    let history: Vec<AgentLogEntry> = rows
                        .into_iter()
                        .filter_map(|row| {
                            Some(AgentLogEntry {
                                agent_id: agent.id.clone(),
                                seq: row.seq,
                                appended_at: row.appended_at,
                                kind: row.kind?,
                            })
                        })
                        .collect();
                    Ok((agent, response, history, more, pagination, cursor))
                })
            })
            .await?
            .ok_or_else(|| {
                AgentReadError::Corrupt("segmented output changed source format".into())
            })?;
        self.pagination_required = result.4;
        // Redacted rows still advance the source coordinate. The caller consumes
        // this page before the next read; never rescan those private rows.
        if result.2.is_empty() {
            self.cursor = result.5;
        }
        Ok((result.0, result.1, result.2, result.3))
    }
}
