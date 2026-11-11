//! Trusted transcript projection reads for the legacy Agent-log format.

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, AgentLogEntry, AgentLogKind};
use mongodb::bson::doc;
use mongodb::options::ReadConcern;

use super::agent_log::read_entry;
use super::agent_log_integrity::read::read_proven_agent;
use super::sandbox_mongo::{signed, MongoSandboxStore};

const ROLLOUT_FLAG: &str = "CHRONO_AGENT_TRANSCRIPT_BOUNDED_READS_ENABLED";

impl MongoSandboxStore {
    #[cfg(test)]
    pub(crate) fn with_bounded_transcript_reads(mut self, enabled: bool) -> Self {
        self.bounded_transcript_reads = enabled;
        self
    }

    pub(super) async fn read_transcript_log(
        &self,
        expected: &Agent,
    ) -> Result<Option<Vec<AgentLogEntry>>, SandboxStoreError> {
        // S2 owns process-config registration. Until that contract is wired,
        // an absent flag is deliberately closed and preserves complete reads.
        if !self.bounded_transcript_reads && std::env::var(ROLLOUT_FLAG).as_deref() != Ok("true") {
            return Ok(None);
        }
        let mut session = self
            .client
            .start_session()
            .snapshot(true)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let Some((agent, trusted)) = read_proven_agent(self, &expected.id, &mut session).await?
        else {
            return Err(SandboxStoreError::NotFound);
        };
        if !trusted || agent != *expected {
            return Ok(None);
        }

        let through = signed(agent.conversation.last_seq, "transcript high-water mark")
            .map_err(SandboxStoreError::Corrupt)?;
        let mut read = super::mongo_read_telemetry::Read::find(self.agent_log.name());
        let mut cursor = self
            .agent_log
            .find(doc! {
                "agent_id": agent.id.as_str(),
                "seq": { "$lte": through },
                "$or": [
                    { "record.kind.type": { "$in": transcript_log_types() } },
                    {
                        "record.kind.type": "operation_delivery_changed",
                        "record.kind.operation": "steer",
                        "record.kind.state": "dispatch_fenced",
                    },
                ],
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
                || !is_transcript_entry(&entry.kind)
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
            || (agent.conversation.conversation_id.is_some()
                && !entries
                    .iter()
                    .any(|entry| matches!(entry.kind, AgentLogKind::ConversationOpened { .. })))
        {
            return Ok(None);
        }
        Ok(Some(entries))
    }
}

fn transcript_log_types() -> Vec<&'static str> {
    vec![
        "created",
        "conversation_opened",
        "runtime_generation_retired",
        "turn_started",
        "turn_metadata_set",
        "owner_message",
        "agent_message",
        "turn_completed",
        "turn_failed",
        "turn_interrupted",
        "conversation_gap",
    ]
}

fn is_transcript_entry(kind: &AgentLogKind) -> bool {
    matches!(
        kind,
        AgentLogKind::Created { .. }
            | AgentLogKind::ConversationOpened { .. }
            | AgentLogKind::RuntimeGenerationRetired { .. }
            | AgentLogKind::TurnStarted { .. }
            | AgentLogKind::TurnMetadataSet { .. }
            | AgentLogKind::OwnerMessage { .. }
            | AgentLogKind::AgentMessage { .. }
            | AgentLogKind::TurnCompleted { .. }
            | AgentLogKind::TurnFailed { .. }
            | AgentLogKind::TurnInterrupted { .. }
            | AgentLogKind::ConversationGap { .. }
            | AgentLogKind::OperationDeliveryChanged {
                operation: layer2_core::sandbox::AgentOperationKind::Steer,
                state: layer2_core::sandbox::AgentOperationDelivery::DispatchFenced,
                ..
            }
    )
}
