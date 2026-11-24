use crate::store::transaction_terminal::GuardedTerminal as _;
use futures_util::TryStreamExt;
use layer2_core::resource_access::{ResourceAccessError, ResourceAccessStoreError};
use layer2_core::sandbox::agent_artifacts::*;
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection, IndexModel,
};

use super::super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, read_record, record_bson,
    start_transaction, CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
    TRANSACTION_ATTEMPTS,
};
use super::{fence_lineage, snapshot};

enum ArtifactStoreFailure {
    Access(ResourceAccessError),
    Driver(mongodb::error::Error),
}
impl From<ResourceAccessError> for ArtifactStoreFailure {
    fn from(value: ResourceAccessError) -> Self {
        Self::Access(value)
    }
}
impl From<ResourceAccessStoreError> for ArtifactStoreFailure {
    fn from(value: ResourceAccessStoreError) -> Self {
        Self::Access(value.into())
    }
}
impl From<mongodb::error::Error> for ArtifactStoreFailure {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Driver(value)
    }
}
impl From<super::ReadError> for ArtifactStoreFailure {
    fn from(value: super::ReadError) -> Self {
        match value {
            super::ReadError::Domain(value) => value.into(),
            super::ReadError::Driver(value) => value.into(),
        }
    }
}

#[async_trait::async_trait]
impl AgentArtifactStore for MongoSandboxStore {
    async fn agent_artifact_metadata(
        &self,
        binding: &ArtifactBinding,
        id: &str,
    ) -> Result<Option<StoredAgentArtifactMetadata>, ResourceAccessStoreError> {
        let mut filter = binding_filter(binding)?;
        filter.insert("_id", id);
        let stored = self
            .agent_artifacts
            .find_one(filter)
            .projection(metadata_projection())
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable)?;
        stored.as_ref().map(read_metadata).transpose()
    }

    async fn publish_artifact(
        &self,
        request: &ArtifactPublication,
    ) -> Result<StoredAgentArtifact, ResourceAccessError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            request.require_current()?;
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?;
            let record = match publish(self, request, &mut session).await {
                Ok(record) => record,
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    match error {
                        ArtifactStoreFailure::Access(error) => return Err(error),
                        ArtifactStoreFailure::Driver(error)
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            continue
                        }
                        ArtifactStoreFailure::Driver(_) => {
                            return Err(ResourceAccessStoreError::Unavailable.into())
                        }
                    }
                }
            };
            if let Err(error) = request.require_current() {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(record),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(ResourceAccessStoreError::Unavailable.into())
                }
            }
        }
        Err(ResourceAccessStoreError::Conflict.into())
    }
    async fn agent_artifacts(
        &self,
        binding: &ArtifactBinding,
        page: &ArtifactPageRequest,
    ) -> Result<Vec<StoredAgentArtifactMetadata>, ResourceAccessStoreError> {
        page.validate()?;
        let mut filter = binding_filter(binding)?;
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! {"$gt":cursor});
        }
        let documents: Vec<Document> = self
            .agent_artifacts
            .find(filter)
            .projection(metadata_projection())
            .sort(doc! {"_id":1})
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable)?
            .try_collect()
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable)?;
        documents.iter().map(read_metadata).collect()
    }
    async fn agent_artifact(
        &self,
        binding: &ArtifactBinding,
        id: &str,
    ) -> Result<Option<StoredAgentArtifact>, ResourceAccessStoreError> {
        let mut filter = binding_filter(binding)?;
        filter.insert("_id", id);
        self.agent_artifacts
            .find_one(filter)
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable)?
            .as_ref()
            .map(read)
            .transpose()
    }
}

async fn publish(
    store: &MongoSandboxStore,
    request: &ArtifactPublication,
    session: &mut ClientSession,
) -> Result<StoredAgentArtifact, ArtifactStoreFailure> {
    let snapshot = snapshot::read(store, &request.target(), session).await?;
    request.validate_snapshot(&snapshot)?;
    if let Some(previous) = store
        .agent_artifacts
        .find_one(doc! {"_id":&request.record().metadata.id})
        .session(&mut *session)
        .await?
    {
        return Ok(request.replay(read(&previous)?)?);
    }
    let count = store
        .agent_artifacts
        .count_documents(doc! {"agent_id":request.record().metadata.agent_id.as_str()})
        .session(&mut *session)
        .await?;
    if count >= MAX_AGENT_ARTIFACTS as u64 {
        return Err(ResourceAccessStoreError::Capacity.into());
    }
    // Every share mutation and ancestry write conflicts with this guard; even
    // first publication under a legacy personal root cannot race a policy edit.
    fence_lineage(store, &snapshot, session).await?;
    let changed = store.agents.update_one(
        doc! {"_id": request.record().metadata.agent_id.as_str(), "channel_history_seal": {"$exists": false}},
        doc! {"$inc": {"artifact_history_fence": 1_i64}},
    ).session(&mut *session).await?;
    if changed.matched_count != 1 {
        return Err(ResourceAccessStoreError::Conflict.into());
    }
    request.require_current()?;
    store
        .agent_artifacts
        .insert_one(document(request.record())?)
        .session(&mut *session)
        .await?;
    Ok(request.record().clone())
}

fn metadata_projection() -> Document {
    doc! {
        "_id":1, "agent_id":1, "binding":1, "document_schema_version":1,
        "record.metadata":1, "record.binding":1, "record.authorization":1,
    }
}

fn read_metadata(
    stored: &Document,
) -> Result<StoredAgentArtifactMetadata, ResourceAccessStoreError> {
    let record: StoredAgentArtifactMetadata =
        read_record(stored).map_err(|_| ResourceAccessStoreError::Corrupt)?;
    record.validate()?;
    let mut expected = binding_filter(&record.binding)?;
    expected.insert("_id", &record.metadata.id);
    expected.insert("document_schema_version", DOCUMENT_SCHEMA_VERSION);
    if expected
        .iter()
        .any(|(key, value)| stored.get(key) != Some(value))
    {
        return Err(ResourceAccessStoreError::Corrupt);
    }
    Ok(record)
}

fn binding_filter(binding: &ArtifactBinding) -> Result<Document, ResourceAccessStoreError> {
    Ok(doc! {"agent_id":binding.agent_id.as_str(),
    "binding":record_bson(binding).map_err(|_| ResourceAccessStoreError::Corrupt)?})
}
fn document(record: &StoredAgentArtifact) -> Result<Document, ResourceAccessStoreError> {
    record.content()?;
    let mut document = binding_filter(&record.binding)?;
    document.insert("_id", &record.metadata.id);
    document.insert("document_schema_version", DOCUMENT_SCHEMA_VERSION);
    document.insert(
        "record",
        record_bson(record).map_err(|_| ResourceAccessStoreError::Corrupt)?,
    );
    Ok(document)
}
fn read(stored: &Document) -> Result<StoredAgentArtifact, ResourceAccessStoreError> {
    let record = read_record(stored).map_err(|_| ResourceAccessStoreError::Corrupt)?;
    let expected = document(&record)?;
    if expected
        .iter()
        .any(|(key, value)| key != "record" && stored.get(key) != Some(value))
    {
        return Err(ResourceAccessStoreError::Corrupt);
    }
    Ok(record)
}

pub(in crate::store) async fn ensure_artifact_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    collection
        .create_index(
            IndexModel::builder()
                .keys(doc! {"agent_id":1,"_id":1})
                .build(),
        )
        .await
        .map_err(|_| layer2_core::StoreError::Unavailable("create Agent artifact index".into()))?;
    Ok(())
}
