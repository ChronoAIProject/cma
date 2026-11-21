//! Trusted usage projection reads for the legacy Agent-log format.

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, AgentLogEntry, AgentLogKind};
use mongodb::bson::doc;
use mongodb::options::ReadConcern;

use super::agent_log::read_entry;
use super::agent_log_integrity::read::read_proven_agent;
use super::agent_log_segment_reads;
use super::sandbox_mongo::{signed, MongoSandboxStore};

impl MongoSandboxStore {
    pub(crate) fn with_bounded_usage_reads(mut self, enabled: bool) -> Self {
        self.bounded_usage_reads = enabled;
        self
    }

    pub(super) async fn read_usage_log(
        &self,
        expected: &Agent,
    ) -> Result<Option<Vec<AgentLogEntry>>, SandboxStoreError> {
        if !self.bounded_usage_reads {
            return Ok(None);
        }
        let mut session = self
            .client
            .start_session()
            .snapshot(true)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;

        // Segment proofs make the selected-row fold safe for histories beyond
        // the legacy physical scan budget. Keep creation and conversation
        // boundaries because UsageFold uses them to validate generations.
        let segmented = agent_log_segment_reads::is_segmented(self, expected, &mut session).await?;
        match agent_log_segment_reads::selected(self, expected, &mut session, is_usage_entry).await
        {
            Ok(Some(entries)) => {
                if entries.first().is_none_or(|entry| entry.seq != 1)
                    || entries
                        .first()
                        .is_none_or(|entry| !matches!(entry.kind, AgentLogKind::Created { .. }))
                {
                    return Ok(None);
                }
                return Ok(Some(entries));
            }
            Ok(None) if segmented => return Ok(None),
            Ok(None) => {}
            Err(SandboxStoreError::Corrupt(_))
            | Err(SandboxStoreError::SequenceConflict)
            | Err(SandboxStoreError::AgentLogCapacityExceeded)
                if segmented =>
            {
                return Ok(None)
            }
            Err(SandboxStoreError::Corrupt(_))
            | Err(SandboxStoreError::SequenceConflict)
            | Err(SandboxStoreError::AgentLogCapacityExceeded) => return Ok(None),
            Err(error) => return Err(error),
        }

        let Some((agent, trusted)) = read_proven_agent(self, &expected.id, &mut session).await?
        else {
            return Err(SandboxStoreError::NotFound);
        };
        if !trusted || agent != *expected {
            return Ok(None);
        }

        let through = signed(agent.conversation.last_seq, "usage high-water mark")
            .map_err(SandboxStoreError::Corrupt)?;
        let mut read = super::mongo_read_telemetry::Read::find(self.agent_log.name());
        let mut cursor = self
            .agent_log
            .find(doc! {
                "agent_id": agent.id.as_str(),
                "seq": { "$lte": through },
                "record.kind.type": { "$in": usage_log_types() },
            })
            .sort(doc! { "seq": 1 })
            .batch_size(200)
            .read_concern(ReadConcern::snapshot())
            .session(&mut session)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut entries = Vec::new();
        while let Some(document) = read
            .next_in_session(&mut cursor, &mut session)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            let entry = match read_entry(&document) {
                Ok(entry) => entry,
                Err(SandboxStoreError::Corrupt(_)) | Err(SandboxStoreError::SequenceConflict) => {
                    read.complete();
                    return Ok(None);
                }
                Err(error) => return Err(error),
            };
            if entry.agent_id != agent.id
                || entry.seq == 0
                || entry.seq > agent.conversation.last_seq
                || entries
                    .last()
                    .is_some_and(|previous: &AgentLogEntry| previous.seq >= entry.seq)
                || !is_usage_entry(&entry.kind)
            {
                read.complete();
                return Ok(None);
            }
            entries.push(entry);
        }
        read.complete();
        if entries.first().is_none_or(|entry| entry.seq != 1)
            || entries
                .first()
                .is_none_or(|entry| !matches!(entry.kind, AgentLogKind::Created { .. }))
        {
            return Ok(None);
        }
        Ok(Some(entries))
    }
}

fn usage_log_types() -> Vec<&'static str> {
    vec![
        "created",
        "conversation_opened",
        "runtime_generation_retired",
        "usage",
        "usage_updated",
    ]
}

fn is_usage_entry(kind: &AgentLogKind) -> bool {
    matches!(
        kind,
        AgentLogKind::Created { .. }
            | AgentLogKind::ConversationOpened { .. }
            | AgentLogKind::RuntimeGenerationRetired { .. }
            | AgentLogKind::Usage { .. }
            | AgentLogKind::UsageUpdated { .. }
    )
}
