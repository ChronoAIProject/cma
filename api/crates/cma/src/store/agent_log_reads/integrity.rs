//! Reader contract for #1591's prepared metadata proof; no metadata parser here.
use layer2_core::{ids::AgentId, sandbox::store::SandboxStoreError};
use mongodb::{bson::doc, options::ReadConcern, ClientSession};

use crate::store::sandbox_mongo::{require_version, unsigned, MongoSandboxStore};

pub(in crate::store) struct ReadHead {
    pub(in crate::store) last_seq: u64,
    /// True only after a prepared-cohort proof of revision/history, head/count/
    /// encoded bytes, raw tail hash and absence of extra rows in this snapshot.
    pub(in crate::store) trusted: bool,
}

#[async_trait::async_trait]
pub(in crate::store) trait LogReadIntegrity: Send + Sync {
    /// Use the supplied snapshot session for every proof query. Return None
    /// only for a missing Agent. Missing/stale/disabled trust returns an
    /// untrusted head; corruption, quarantine and dependency errors propagate.
    /// Never start/abort a transaction, repair metadata, or write quarantine.
    async fn read_head(
        &self,
        store: &MongoSandboxStore,
        agent_id: &AgentId,
        session: &mut ClientSession,
    ) -> Result<Option<ReadHead>, SandboxStoreError>;
}

pub(in crate::store) struct LegacyIntegrity;

#[async_trait::async_trait]
impl LogReadIntegrity for LegacyIntegrity {
    async fn read_head(
        &self,
        store: &MongoSandboxStore,
        agent_id: &AgentId,
        session: &mut ClientSession,
    ) -> Result<Option<ReadHead>, SandboxStoreError> {
        let Some(row) = store
            .agents
            .find_one(doc! { "_id": agent_id.as_str() })
            .projection(doc! { "document_schema_version": 1, "last_seq": 1 })
            .read_concern(ReadConcern::snapshot())
            .session(session)
            .await
            .map_err(super::unavailable)?
        else {
            return Ok(None);
        };
        require_version(&row).map_err(SandboxStoreError::Corrupt)?;
        let last_seq = row
            .get_i64("last_seq")
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
            .and_then(|value| {
                unsigned(value, "agent log sequence").map_err(SandboxStoreError::Corrupt)
            })?;
        Ok(Some(ReadHead {
            last_seq,
            trusted: false,
        }))
    }
}
