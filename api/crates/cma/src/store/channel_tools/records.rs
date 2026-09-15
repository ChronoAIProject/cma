use super::*;

pub(super) fn read(document: &Document) -> Result<CapabilityRecord, OutboxError> {
    let record: CapabilityRecord = read_record(document).map_err(|_| OutboxError::Corrupt)?;
    if document.get_str("_id").ok() != Some(&record.id)
        || document.get_str("token_sha256").ok() != Some(&record.token_sha256)
        || document.get_bool("revoked").ok() != Some(record.revoked)
        || document.get_i64("revision").ok() != i64::try_from(record.revision).ok()
        || record.revision == 0
        || record.runtime.provision_generation.is_none_or(|v| v == 0)
        || record.token_sha256.len() != 64
        || record.id
            != identity(
                &record.agent_id,
                &record.agent_created_at,
                &record.creation_fingerprint,
            )?
    {
        return Err(OutboxError::Corrupt);
    }
    Ok(record)
}

pub(super) fn document(record: &CapabilityRecord) -> Result<Document, OutboxError> {
    Ok(
        doc! {"_id":&record.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "token_sha256":&record.token_sha256,"revoked":record.revoked,
        "revision":i64::try_from(record.revision).map_err(|_|OutboxError::Corrupt)?,
        "record":record_bson(record).map_err(|_|OutboxError::Corrupt)?},
    )
}

pub(super) fn identity(
    agent: &layer2_core::AgentId,
    created_at: &str,
    fingerprint: &str,
) -> Result<String, OutboxError> {
    Ok(format!(
        "brcap_{}",
        sha(
            &serde_json::to_vec(&("cma.channel.capability.v1", agent, created_at, fingerprint))
                .map_err(|_| OutboxError::Corrupt)?
        )
    ))
}

impl CapabilityRecord {
    pub(super) fn require_connection(
        &self,
        snapshot: &AgentProvenanceSnapshot,
    ) -> Result<(), OutboxError> {
        let agent = snapshot
            .resource
            .lineage
            .agent
            .as_ref()
            .ok_or(OutboxError::ToolUnavailable)?;
        let provenance = snapshot
            .creation_provenance(&self.agent_id)
            .map_err(|_| OutboxError::ToolUnavailable)?;
        if self.revoked
            || agent.id != self.agent_id
            || agent.created_at != self.agent_created_at
            || agent.creation_fingerprint != self.creation_fingerprint
            || snapshot.resource.lineage.sandbox.runtime.as_ref() != Some(&self.runtime)
            || !matches!(
                provenance.creation_origin,
                AgentCreationOrigin::ChannelBridge { .. }
            )
            || !agent.status.is_live()
            || agent.requested_stop_reason.is_some()
        {
            return Err(OutboxError::ToolUnavailable);
        }
        Ok(())
    }
    pub(super) fn authorize(
        &self,
        snapshot: &AgentProvenanceSnapshot,
    ) -> Result<ChannelReplySessionAuthorization, OutboxError> {
        self.require_connection(snapshot)?;
        let agent = snapshot
            .resource
            .lineage
            .agent
            .as_ref()
            .ok_or(OutboxError::ToolUnavailable)?;
        let sandbox = &snapshot.resource.lineage.sandbox;
        let provenance = snapshot
            .creation_provenance(&self.agent_id)
            .map_err(|_| OutboxError::ToolUnavailable)?;
        if self.revoked
            || agent.id != self.agent_id
            || agent.created_at != self.agent_created_at
            || agent.creation_fingerprint != self.creation_fingerprint
            || sandbox.runtime.as_ref() != Some(&self.runtime)
            || self.opened_conversation.is_none()
            || self.opened_conversation != agent.conversation.conversation_id
            || !agent.status.is_live()
            || agent.requested_stop_reason.is_some()
        {
            return Err(OutboxError::ToolUnavailable);
        }
        ChannelReplySessionAuthorization::from_current(ChannelReplySessionObservation {
            agent,
            sandbox,
            provenance: &provenance,
            capability_incarnation: &self.id,
            runtime_generation: self
                .runtime
                .provision_generation
                .ok_or(OutboxError::ToolUnavailable)?,
            deadline: tokio::time::Instant::now() + Duration::from_secs(15),
        })
    }
}
