//! Encrypted CMA runtime tool capabilities, separate from NyxID relay custody.
use std::{sync::Arc, time::Duration};

use layer2_core::bridge::outbox::{
    ChannelReplySessionAuthorization, ChannelReplySessionObservation, OutboxError,
};
use layer2_core::sandbox::store::AgentProvenanceSnapshot;
use layer2_core::sandbox::{AgentCreationOrigin, RuntimeBinding};
use layer3_adapter::hosted::{HostedChannelToolBinding, HostedChannelToolSource};
use layer3_adapter::{AgentOpenRequest, RuntimeCredential, RuntimeHandle};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::sandbox_mongo::{
    commit_transaction, read_record, record_bson, start_transaction, CommitOutcome,
    DOCUMENT_SCHEMA_VERSION,
};
use super::{
    encryption::{AuthCipher, CipherEnvelope},
    AuthEncryptionKey, MongoSandboxStore,
};

mod binding;
mod open;
mod records;
pub(in crate::store) use open::validate_and_fence_open;
#[cfg(test)]
mod tests;

const COLLECTION: &str = "bridge_channel_capabilities";

pub(crate) struct MongoChannelToolStore {
    resources: Arc<MongoSandboxStore>,
    capabilities: Collection<Document>,
    cipher: AuthCipher,
    public_origin: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityRecord {
    id: String,
    agent_id: layer2_core::AgentId,
    agent_created_at: String,
    creation_fingerprint: String,
    runtime: RuntimeBinding,
    request_sha256: String,
    token_sha256: String,
    secret: CipherEnvelope,
    opened_conversation: Option<String>,
    revoked: bool,
    revision: u64,
}

impl MongoChannelToolStore {
    /// MCP initialization precedes conversation publication. This permission
    /// admits discovery only; a reply additionally needs authenticated session
    /// proof plus the exact current command/turn authorization.
    pub(crate) async fn authorize_connection(
        &self,
        token: &RuntimeCredential,
    ) -> Result<(), OutboxError> {
        if !crate::bridge::channel_tools::valid_credential(token.expose()) {
            return Err(OutboxError::ToolUnavailable);
        }
        let mut session = self
            .resources
            .client
            .start_session()
            .await
            .map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        let result = async {
            let document = self
                .capabilities
                .find_one(doc! {"token_sha256":sha(token.expose().as_bytes()),"revoked":false})
                .session(&mut session)
                .await
                .map_err(unavailable)?
                .ok_or(OutboxError::ToolUnavailable)?;
            let record = records::read(&document)?;
            let snapshot = super::agent_provenance::read_snapshot(
                &self.resources,
                &record.agent_id,
                &mut session,
            )
            .await
            .map_err(|_| OutboxError::ToolUnavailable)?;
            record.require_connection(&snapshot)
        }
        .await;
        let _ = session.abort_transaction().await;
        result
    }
    pub(crate) async fn open(
        resources: Arc<MongoSandboxStore>,
        deployment: &str,
        key: &AuthEncryptionKey,
        public_origin: &str,
    ) -> Result<Self, layer2_core::StoreError> {
        let database = resources
            .client
            .database(&resources.sandboxes.namespace().db);
        super::schema::initialize_collections(&database, &[COLLECTION]).await?;
        let capabilities = super::durability::majority_collection(&database, COLLECTION);
        capabilities
            .create_index(
                mongodb::IndexModel::builder()
                    .keys(doc! {"token_sha256":1_i32})
                    .options(
                        mongodb::options::IndexOptions::builder()
                            .unique(true)
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(|_| {
                layer2_core::StoreError::Unavailable("channel capability index unavailable".into())
            })?;
        Ok(Self {
            resources,
            capabilities,
            cipher: AuthCipher::new(deployment, key)?,
            public_origin: public_origin.into(),
        })
    }

    pub(crate) async fn authenticate(
        &self,
        token: &RuntimeCredential,
    ) -> Result<ChannelReplySessionAuthorization, OutboxError> {
        if !crate::bridge::channel_tools::valid_credential(token.expose()) {
            return Err(OutboxError::ToolUnavailable);
        }
        let mut session = self
            .resources
            .client
            .start_session()
            .await
            .map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        let result = self.authenticate_in(token, &mut session).await;
        let _ = session.abort_transaction().await;
        result
    }

    async fn authenticate_in(
        &self,
        token: &RuntimeCredential,
        session: &mut ClientSession,
    ) -> Result<ChannelReplySessionAuthorization, OutboxError> {
        let document = self
            .capabilities
            .find_one(doc! {"token_sha256":sha(token.expose().as_bytes()),"revoked":false})
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .ok_or(OutboxError::ToolUnavailable)?;
        let record = records::read(&document)?;
        let snapshot =
            super::agent_provenance::read_snapshot(&self.resources, &record.agent_id, session)
                .await
                .map_err(|_| OutboxError::ToolUnavailable)?;
        record.authorize(&snapshot)
    }
}

/// Called inside every reply admission/recovery transaction. Runtime changes
/// rotate the bearer; revocation and concurrent rotation conflict with this write.
pub(in crate::store) async fn validate_and_fence_session(
    resources: &MongoSandboxStore,
    capability: &ChannelReplySessionAuthorization,
    session: &mut ClientSession,
) -> Result<(), OutboxError> {
    capability.require_current()?;
    let collection = super::durability::majority_collection::<Document>(
        &resources
            .client
            .database(&resources.sandboxes.namespace().db),
        COLLECTION,
    );
    let document = collection
        .find_one(doc! {"_id":capability.capability_incarnation(),"revoked":false})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .ok_or(OutboxError::ToolUnavailable)?;
    let record = records::read(&document)?;
    let snapshot = super::agent_provenance::read_snapshot(resources, &record.agent_id, session)
        .await
        .map_err(|_| OutboxError::ToolUnavailable)?;
    let current = record.authorize(&snapshot)?;
    if current.namespace() != capability.namespace()
        || current.runtime_generation() != capability.runtime_generation()
        || current.owner() != capability.owner()
    {
        return Err(OutboxError::SourceMismatch);
    }
    let changed = collection
        .update_one(
            doc! {"_id":&record.id,"revision":record.revision as i64,"revoked":false},
            doc! {"$inc":{"authorization_fence":1_i64}},
        )
        .session(&mut *session)
        .await
        .map_err(unavailable)?;
    if changed.matched_count != 1 {
        return Err(OutboxError::Conflict);
    }
    capability.require_current()
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn unavailable<T>(_: T) -> OutboxError {
    OutboxError::Unavailable
}
fn adapter_error(_: OutboxError) -> layer3_adapter::AdapterError {
    layer3_adapter::AdapterError::Rejected("channel tool capability unavailable".into())
}
