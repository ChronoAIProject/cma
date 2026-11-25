//! Fence a Document and its reclaim obligation in one majority transaction.
use super::{mongo::MongoDocumentStore, quota};
use crate::store::transaction_terminal::GuardedTerminal as _;
use crate::store::{commit_transaction, is_confirmed_body_retry, start_transaction, CommitOutcome};
use layer2_core::document_artifacts::{digest, DocumentError, DocumentTombstone, StoredDocument};
use mongodb::{
    bson::{doc, Document, Timestamp},
    options::ReadConcern,
    ClientSession,
};

const TOMBSTONE_VERSION: i64 = 2;

pub(crate) fn encode(
    tombstone: &DocumentTombstone,
    audit: &StoredDocument,
) -> Result<Document, DocumentError> {
    tombstone.validate()?;
    audit.validate()?;
    if audit.metadata.id != tombstone.receipt.id
        || digest(audit.metadata.owner_subject.as_bytes()) != tombstone.owner_hash
        || audit.metadata.revision != tombstone.expected_revision
    {
        return Err(DocumentError::Corrupt);
    }
    let data = mongodb::bson::serialize_to_bson(tombstone).map_err(|_| DocumentError::Corrupt)?;
    let retained = mongodb::bson::serialize_to_bson(audit).map_err(|_| DocumentError::Corrupt)?;
    Ok(doc! {
        "_id": &tombstone.receipt.id,
        "document_schema_version": TOMBSTONE_VERSION,
        "deletion": data,
        "audit_owner_subject": &audit.metadata.owner_subject,
        "audit": retained,
    })
}

pub(crate) fn decode_audit(
    row: &Document,
) -> Result<Option<(DocumentTombstone, StoredDocument)>, DocumentError> {
    if row.get_i64("document_schema_version").ok() != Some(TOMBSTONE_VERSION) {
        return Ok(None);
    }
    let tombstone: DocumentTombstone = mongodb::bson::deserialize_from_bson(
        row.get("deletion").ok_or(DocumentError::Corrupt)?.clone(),
    )
    .map_err(|_| DocumentError::Corrupt)?;
    let audit: StoredDocument = mongodb::bson::deserialize_from_bson(
        row.get("audit").ok_or(DocumentError::Corrupt)?.clone(),
    )
    .map_err(|_| DocumentError::Corrupt)?;
    if row != &encode(&tombstone, &audit)? {
        return Err(DocumentError::Corrupt);
    }
    Ok(Some((tombstone, audit)))
}

pub(super) fn decode(row: &Document) -> Result<Option<DocumentTombstone>, DocumentError> {
    Ok(decode_audit(row)?.map(|(tombstone, _)| tombstone))
}

pub(super) fn obligation_id(owner_hash: &str, document_id: &str) -> String {
    format!("deletion:{owner_hash}:{document_id}")
}

pub(super) fn obligation(
    tombstone: &DocumentTombstone,
    activation: &Document,
) -> Result<Document, DocumentError> {
    if !quota::ready(activation) {
        return Err(DocumentError::Unavailable);
    }
    Ok(doc! {
        "_id": obligation_id(&tombstone.owner_hash, &tombstone.receipt.id),
        "document_schema_version": 1_i64,
        "kind": "deletion",
        "owner_hash": &tombstone.owner_hash,
        "document_id": &tombstone.receipt.id,
        "request_hash": &tombstone.request_hash,
        "revision": tombstone.receipt.revision as i64,
        "deleted_at": &tombstone.receipt.deleted_at,
        "activation_id": activation.get_str("activation_id").map_err(|_| DocumentError::Corrupt)?,
        "inventory_sha256": activation.get_str("inventory_sha256").map_err(|_| DocumentError::Corrupt)?,
    })
}

pub(super) async fn tombstone(
    store: &MongoDocumentStore,
    id: &str,
) -> Result<Option<DocumentTombstone>, DocumentError> {
    match store
        .rows
        .find_one(doc! {"_id": id})
        .max_time(quota::IO)
        .await
        .map_err(quota::unavailable)?
    {
        Some(row) => decode(&row),
        None => Ok(None),
    }
}

pub(super) async fn delete(
    store: &MongoDocumentStore,
    owner: &str,
    prior: &DocumentTombstone,
) -> Result<bool, DocumentError> {
    if prior.owner_hash != digest(owner.as_bytes()) {
        return Err(DocumentError::Corrupt);
    }
    prior.validate()?;
    if i64::try_from(prior.receipt.revision).is_err() {
        return Err(DocumentError::Capacity);
    }
    tokio::time::timeout(quota::IO, delete_bounded(store, owner, prior))
        .await
        .map_err(|_| DocumentError::Unavailable)?
}

async fn delete_bounded(
    store: &MongoDocumentStore,
    owner: &str,
    prior: &DocumentTombstone,
) -> Result<bool, DocumentError> {
    let ledger = store.quota(true).rows;
    for _ in 0..5 {
        let mut session = store
            .rows
            .client()
            .start_session()
            .await
            .map_err(quota::unavailable)?;
        match start_transaction(&mut session).await {
            Ok(()) => {}
            Err(error) if is_confirmed_body_retry(&error) => continue,
            Err(_) => return Err(DocumentError::Unavailable),
        }
        match write(store, &ledger, owner, prior, &mut session).await {
            Ok(true) => {}
            result => {
                let _ = session.abort_guarded().await;
                match result {
                    Ok(applied) => return Ok(applied),
                    Err(WriteError::Driver(error)) if is_confirmed_body_retry(&error) => continue,
                    Err(WriteError::Driver(_)) => return Err(DocumentError::Unavailable),
                    Err(WriteError::Domain(error)) => return Err(error),
                }
            }
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(true),
            CommitOutcome::Retry(_) => continue,
            CommitOutcome::Unknown(_) => {
                let time = session.operation_time().ok_or(DocumentError::Unavailable)?;
                return reconcile(store, &ledger, prior, time).await;
            }
            CommitOutcome::Failed(_) => return Err(DocumentError::Unavailable),
        }
    }
    Err(DocumentError::Unavailable)
}

async fn reconcile(
    store: &MongoDocumentStore,
    ledger: &mongodb::Collection<Document>,
    prior: &DocumentTombstone,
    time: Timestamp,
) -> Result<bool, DocumentError> {
    let mut snapshot = store
        .rows
        .client()
        .start_session()
        .snapshot(true)
        .snapshot_time(time)
        .await
        .map_err(quota::unavailable)?;
    let current = store
        .rows
        .find_one(doc! {"_id": &prior.receipt.id})
        .read_concern(ReadConcern::snapshot())
        .session(&mut snapshot)
        .await
        .map_err(quota::unavailable)?
        .ok_or(DocumentError::Unavailable)?;
    let activation = ledger
        .find_one(doc! {"_id": quota::STATE})
        .read_concern(ReadConcern::snapshot())
        .session(&mut snapshot)
        .await
        .map_err(quota::unavailable)?
        .ok_or(DocumentError::Unavailable)?;
    let marker = obligation(prior, &activation)?;
    if decode(&current)?.as_ref() != Some(prior)
        || ledger
            .find_one(doc! {"_id": &marker["_id"]})
            .read_concern(ReadConcern::snapshot())
            .session(&mut snapshot)
            .await
            .map_err(quota::unavailable)?
            .as_ref()
            != Some(&marker)
    {
        return Err(DocumentError::Unavailable);
    }
    Ok(true)
}

async fn write(
    store: &MongoDocumentStore,
    ledger: &mongodb::Collection<Document>,
    owner: &str,
    prior: &DocumentTombstone,
    session: &mut ClientSession,
) -> Result<bool, WriteError> {
    let activation = ledger
        .find_one(doc! {"_id": quota::STATE})
        .session(&mut *session)
        .await?
        .ok_or(DocumentError::Unavailable)?;
    if !quota::ready(&activation) {
        return Err(DocumentError::Unavailable.into());
    }
    let owner_id = quota::owner_id(&prior.owner_hash);
    let owner_row = ledger
        .find_one(doc! {"_id": &owner_id})
        .session(&mut *session)
        .await?
        .ok_or(DocumentError::Corrupt)?;
    quota::validate_row(&owner_row, "owner")?;
    if quota::number(&owner_row, "documents")? == 0 {
        return Err(DocumentError::Corrupt.into());
    }
    let document = ledger
        .find_one(doc! {"_id": format!("document:{}:{}", prior.owner_hash, prior.receipt.id)})
        .session(&mut *session)
        .await?
        .ok_or(DocumentError::Corrupt)?;
    quota::validate_row(&document, "document")?;
    let fenced = ledger.update_one(doc! {"_id": &owner_id, "kind": "owner", "fence": owner_row.get_i64("fence").map_err(|_| DocumentError::Corrupt)?}, doc! {"$inc": {"fence": 1_i64}}).session(&mut *session).await?;
    if fenced.matched_count != 1 {
        return Err(DocumentError::Conflict.into());
    }
    let filter = doc! {"_id": &prior.receipt.id, "owner_subject": owner, "revision": prior.expected_revision as i64};
    let Some(live) = store
        .rows
        .find_one(filter.clone())
        .session(&mut *session)
        .await?
    else {
        return Ok(false);
    };
    let audit = super::mongo::decode(&live)?;
    let replacement = encode(prior, &audit)?;
    let result = store
        .rows
        .replace_one(filter, replacement)
        .session(&mut *session)
        .await?;
    if result.matched_count == 0 {
        return Ok(false);
    }
    ledger
        .insert_one(obligation(prior, &activation)?)
        .session(&mut *session)
        .await?;
    Ok(true)
}

enum WriteError {
    Driver(mongodb::error::Error),
    Domain(DocumentError),
}
impl From<mongodb::error::Error> for WriteError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}
impl From<DocumentError> for WriteError {
    fn from(error: DocumentError) -> Self {
        Self::Domain(error)
    }
}
