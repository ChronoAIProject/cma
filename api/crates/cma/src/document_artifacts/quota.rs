//! Shared owner accounting; the immutable baseline is installed separately.
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::document_artifacts::*;
use mongodb::{
    bson::{doc, Document},
    options::{ReadConcern, WriteConcern},
    Client, ClientSession, Collection, Database,
};
use std::time::Duration;

pub(crate) const QUOTA_COLLECTION: &str = "cma_document_quotas";
pub(super) const IO: Duration = Duration::from_secs(3);
pub(super) const STATE: &str = "activation";

pub(crate) struct MongoDocumentQuotaStore {
    pub(super) rows: Collection<Document>,
    client: Client,
    enabled: bool,
    require_ready: bool,
    generation_writes: bool,
    storage_namespace: Option<String>,
    limits: [u64; 3],
}
impl MongoDocumentQuotaStore {
    pub(crate) fn new(database: &Database, enabled: bool) -> Self {
        Self {
            rows: database.collection_with_options(
                QUOTA_COLLECTION,
                mongodb::options::CollectionOptions::builder()
                    .read_concern(ReadConcern::majority())
                    .write_concern(WriteConcern::majority())
                    .build(),
            ),
            client: database.client().clone(),
            enabled,
            require_ready: false,
            generation_writes: false,
            storage_namespace: None,
            limits: [
                OWNER_DOCUMENT_LIMIT,
                OWNER_DOCUMENT_BYTES,
                OWNER_DOCUMENT_OBJECT_LIMIT,
            ],
        }
    }

    pub(super) fn with_required_baseline(mut self, required: bool) -> Self {
        self.require_ready = required;
        self
    }

    pub(super) fn with_generation_writes(mut self, enabled: bool) -> Self {
        self.generation_writes = enabled;
        self
    }

    pub(super) fn with_storage_namespace(mut self, namespace: Option<String>) -> Self {
        self.storage_namespace = namespace;
        self
    }

    async fn reserve_bounded(
        &self,
        source: &DocumentSourceReservation,
    ) -> Result<Option<DocumentSourceFence>, DocumentError> {
        let state = self
            .rows
            .find_one(doc! {"_id": STATE})
            .max_time(IO)
            .await
            .map_err(unavailable)?;
        match state {
            None if !self.enabled && !self.require_ready && !self.generation_writes => {
                return Ok(None)
            }
            Some(state) if self.enabled && ready(&state) => {}
            _ => return Err(DocumentError::Unavailable),
        }
        for attempt in 0..32 {
            let mut session = self.client.start_session().await.map_err(unavailable)?;
            session
                .start_transaction()
                .read_concern(ReadConcern::snapshot())
                .write_concern(WriteConcern::majority())
                .max_commit_time(IO)
                .await
                .map_err(unavailable)?;
            let outcome = match crate::store::transaction_barrier::open(&mut session).await {
                Ok(()) => self.reserve_transaction(source, &mut session).await,
                Err(error) => Err(QuotaError::Driver(error)),
            };
            match outcome {
                Ok(fence) => match session.commit_guarded().await {
                    Ok(()) => return Ok(fence),
                    Err(error)
                        if retryable(&error)
                            || error.contains_label("UnknownTransactionCommitResult") => {}
                    Err(_) => return Err(DocumentError::Unavailable),
                },
                Err(QuotaError::Domain(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
                Err(QuotaError::Driver(error)) if retryable(&error) => {}
                Err(_) => {
                    let _ = session.abort_guarded().await;
                    return Err(DocumentError::Unavailable);
                }
            }
            let _ = session.abort_guarded().await;
            tokio::time::sleep(Duration::from_millis((attempt + 1).min(16))).await;
        }
        Err(DocumentError::Unavailable)
    }

    async fn reserve_transaction(
        &self,
        source: &DocumentSourceReservation,
        session: &mut ClientSession,
    ) -> Result<Option<DocumentSourceFence>, QuotaError> {
        let state = self
            .rows
            .find_one(doc! { "_id": STATE })
            .session(&mut *session)
            .await?
            .ok_or(DocumentError::Unavailable)?;
        if !ready(&state) {
            return Err(DocumentError::Unavailable.into());
        }
        let enabled = self.generation_writes || generation_enabled(&state)?;
        if enabled || state.contains_key("storage_namespace_sha256") {
            super::reclamation_binding::Binding::read(
                &state,
                self.storage_namespace
                    .as_deref()
                    .ok_or(DocumentError::Unavailable)?,
            )?;
        }
        if enabled && !generation_enabled(&state)? {
            self.rows
                .update_one(
                    doc! { "_id": STATE, "phase": "ready" },
                    doc! { "$set": { "source_generations": true } },
                )
                .session(&mut *session)
                .await?;
        }
        let owner_id = owner_id(&source.owner_hash);
        self.rows.update_one(doc! {"_id": &owner_id}, doc! {
            "$inc": {"fence": 1_i64},
            "$setOnInsert": {"document_schema_version": 1_i64, "kind": "owner", "documents": 0_i64, "bytes": 0_i64, "objects": 0_i64}
        }).upsert(true).session(&mut *session).await?;
        let owner = self
            .rows
            .find_one(doc! {"_id": &owner_id})
            .session(&mut *session)
            .await?
            .ok_or(DocumentError::Corrupt)?;
        validate_row(&owner, "owner")?;
        if self.rows.find_one(doc! {"_id": super::deletion::obligation_id(&source.owner_hash, &source.document_id)})
            .session(&mut *session).await?.is_some() {
            return Err(DocumentError::NotFound.into());
        }
        let usage = [
            number(&owner, "documents")?,
            number(&owner, "bytes")?,
            number(&owner, "objects")?,
        ];
        let object = object_row(source)?;
        if let Some(existing) = self
            .rows
            .find_one(doc! {"_id": object.get_str("_id").map_err(|_| DocumentError::Corrupt)?})
            .session(&mut *session)
            .await?
        {
            if existing != object {
                return Err(DocumentError::Corrupt.into());
            }
            return self.reserve_authority(source, true, enabled, session).await;
        }
        let document = document_row(source);
        let existing = self
            .rows
            .find_one(doc! {"_id": document.get_str("_id").map_err(|_| DocumentError::Corrupt)?})
            .session(&mut *session)
            .await?;
        let new_document = match existing {
            None => true,
            Some(existing) if existing == document => false,
            _ => return Err(DocumentError::Corrupt.into()),
        };
        let additions = [u64::from(new_document), source.bytes, 1];
        if usage
            .iter()
            .zip(additions)
            .zip(self.limits)
            .any(|((&used, added), limit)| used.checked_add(added).is_none_or(|n| n > limit))
        {
            return Err(DocumentError::Capacity.into());
        }
        if new_document {
            self.rows
                .insert_one(document)
                .session(&mut *session)
                .await?;
            if enabled {
                super::reclamation_slots::open(&self.rows, source, session).await?;
            }
        }
        self.rows.insert_one(object).session(&mut *session).await?;
        self.rows.update_one(doc! {"_id": &owner_id}, doc! {"$inc": {
            "documents": i64::from(new_document), "bytes": source.bytes as i64, "objects": 1_i64
        }}).session(&mut *session).await?;
        self.reserve_authority(source, false, enabled, session)
            .await
    }

    async fn reserve_authority(
        &self,
        source: &DocumentSourceReservation,
        charged: bool,
        enabled: bool,
        session: &mut ClientSession,
    ) -> Result<Option<DocumentSourceFence>, QuotaError> {
        super::object_reservations::reserve(&self.rows, source, charged, enabled, session)
            .await?
            .map(|authority| DocumentSourceFence::new(authority.generation, authority.reservation))
            .transpose()
            .map_err(QuotaError::from)
    }
}

#[async_trait::async_trait]
impl DocumentQuotaStore for MongoDocumentQuotaStore {
    async fn reserve(&self, source: &DocumentSourceReservation) -> Result<(), DocumentError> {
        self.reserve_source(source).await.map(|_| ())
    }
    async fn reserve_source(
        &self,
        source: &DocumentSourceReservation,
    ) -> Result<Option<DocumentSourceFence>, DocumentError> {
        source.validate()?;
        tokio::time::timeout(IO, self.reserve_bounded(source))
            .await
            .map_err(|_| DocumentError::Unavailable)?
    }
}

pub(super) fn generation_enabled(state: &Document) -> Result<bool, DocumentError> {
    if !state.contains_key("source_generations") {
        return Ok(false);
    }
    state
        .get_bool("source_generations")
        .map_err(|_| DocumentError::Corrupt)
}

pub(super) fn ready(state: &Document) -> bool {
    validate_row(state, "activation").is_ok()
        && state.get_str("phase").ok() == Some("ready")
        && state.get_str("inventory_sha256").is_ok_and(valid_digest)
        && state
            .get_str("activation_id")
            .is_ok_and(super::quota_baseline::valid_activation)
        && number(state, "baseline_rows").is_ok_and(|rows| number(state, "next_row") == Ok(rows))
}
pub(super) fn validate_row(row: &Document, kind: &str) -> Result<(), DocumentError> {
    if row.get_i64("document_schema_version").ok() != Some(1)
        || row.get_str("kind").ok() != Some(kind)
    {
        return Err(DocumentError::Corrupt);
    }
    Ok(())
}
pub(super) fn number(row: &Document, key: &str) -> Result<u64, DocumentError> {
    row.get_i64(key)
        .ok()
        .and_then(|n| u64::try_from(n).ok())
        .ok_or(DocumentError::Corrupt)
}
pub(super) fn owner_id(hash: &str) -> String {
    format!("owner:{hash}")
}
pub(super) fn document_row(source: &DocumentSourceReservation) -> Document {
    doc! {"_id": format!("document:{}:{}", source.owner_hash, source.document_id), "document_schema_version": 1_i64, "kind": "document"}
}
pub(super) fn object_row(source: &DocumentSourceReservation) -> Result<Document, DocumentError> {
    source.validate()?;
    Ok(
        doc! {"_id": format!("object:{}:{}:{}", source.owner_hash, source.document_id, source.sha256),
        "document_schema_version": 1_i64, "kind": "object", "bytes": source.bytes as i64},
    )
}
pub(super) fn object_range(source: &DocumentSourceReservation) -> Document {
    let prefix = format!("object:{}:{}:", source.owner_hash, source.document_id);
    doc! { "_id": { "$gte": &prefix, "$lt": format!("{prefix}g") }, "kind": "object" }
}

pub(super) fn source_from_charge(
    row: &Document,
) -> Result<DocumentSourceReservation, DocumentError> {
    let key = row.get_str("_id").map_err(|_| DocumentError::Corrupt)?;
    let mut parts = key.split(':');
    let (Some("object"), Some(owner), Some(document), Some(digest), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return Err(DocumentError::Corrupt);
    };
    let source = DocumentSourceReservation {
        owner_hash: owner.to_owned(),
        document_id: document.to_owned(),
        sha256: digest.to_owned(),
        bytes: number(row, "bytes")?,
    };
    if object_row(&source)? != *row {
        return Err(DocumentError::Corrupt);
    }
    Ok(source)
}

pub(super) fn unavailable(_: mongodb::error::Error) -> DocumentError {
    DocumentError::Unavailable
}
fn retryable(error: &mongodb::error::Error) -> bool {
    error.contains_label("TransientTransactionError")
        || matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(e)) if e.code == 11000)
}
pub(super) enum QuotaError {
    Domain(DocumentError),
    Driver(mongodb::error::Error),
}
impl From<DocumentError> for QuotaError {
    fn from(error: DocumentError) -> Self {
        Self::Domain(error)
    }
}
impl From<mongodb::error::Error> for QuotaError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}

#[cfg(test)]
#[path = "quota_mongo_tests.rs"]
mod tests;
