//! Shared query machinery selects full records or a bounded lineage projection.
use super::*;
use layer2_core::sandbox::{Agent, SandboxAgentLineage};

pub(super) trait AgentReader {
    type Record;
    fn projection() -> Option<Document>;
    fn read(document: &Document) -> Result<Self::Record, SandboxStoreError>;
    fn id(record: &Self::Record) -> &str;
}

pub(super) struct Complete;
impl AgentReader for Complete {
    type Record = Agent;
    fn projection() -> Option<Document> {
        None
    }
    fn read(document: &Document) -> Result<Agent, SandboxStoreError> {
        read_agent(document)
    }
    fn id(record: &Agent) -> &str {
        record.id.as_str()
    }
}

pub(super) struct Projected;
impl AgentReader for Projected {
    type Record = SandboxAgentLineage;
    fn projection() -> Option<Document> {
        // Both copies are needed: a corrupt indexed query field must still be
        // rejected against the authoritative aggregate's lineage field.
        Some(doc! {
            "_id":1, "document_schema_version":1,
            "agent_log_format":1, "agent_log_incarnation":1, "agent_log_head_revision":1,
            "creation_key":1, "record.creation_key":1, "record.created_at":1,
            "owner":1, "sandbox_id":1, "workspace_id":1, "worktree_id":1, "revision":1,
            "record.id":1, "record.owner":1, "record.sandbox_id":1,
            "record.workspace_id":1, "record.worktree_id":1, "record.revision":1,
        })
    }
    fn read(document: &Document) -> Result<SandboxAgentLineage, SandboxStoreError> {
        use crate::store::{
            agent_log_segments::{incarnation_fields, read_format},
            sandbox_mongo::{i64_field_matches, signed, string_field_matches},
        };
        // Agent envelopes support both retained and segmented schemas. Keep
        // their existing discriminator and identity checks without reading logs.
        let format = read_format(document)?;
        let mut record = document
            .get_document("record")
            .map_err(|_| invalid())?
            .clone();
        let Some(mongodb::bson::Bson::String(creation_key)) = record.remove("creation_key") else {
            return Err(invalid());
        };
        let Some(mongodb::bson::Bson::String(created_at)) = record.remove("created_at") else {
            return Err(invalid());
        };
        let agent: SandboxAgentLineage =
            mongodb::bson::deserialize_from_document(record).map_err(|_| invalid())?;
        let revision = signed(agent.revision, "agent revision").map_err(|_| invalid())?;
        if !string_field_matches(document, "_id", agent.id.as_str())
            || !string_field_matches(document, "owner", agent.owner.as_str())
            || !string_field_matches(document, "sandbox_id", agent.sandbox_id.as_str())
            || !string_field_matches(document, "workspace_id", agent.workspace_id.as_str())
            || !string_field_matches(document, "worktree_id", agent.worktree_id.as_str())
            || !i64_field_matches(document, "revision", revision)
            || !string_field_matches(document, "creation_key", &creation_key)
            || format.is_some_and(|(identity, _)| {
                identity != incarnation_fields(agent.id.as_str(), &creation_key, &created_at)
            })
        {
            return Err(invalid());
        }
        Ok(agent)
    }
    fn id(record: &SandboxAgentLineage) -> &str {
        record.id.as_str()
    }
}

fn invalid() -> SandboxStoreError {
    SandboxStoreError::Corrupt("Agent lineage projection disagrees with its aggregate".into())
}
