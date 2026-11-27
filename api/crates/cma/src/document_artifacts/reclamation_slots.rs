//! Evidence required to refund the last charged object and its Document slot.
use super::{
    deletion,
    quota::{self, QuotaError},
};
use layer2_core::document_artifacts::{DocumentError, DocumentSourceReservation};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};

fn key(source: &DocumentSourceReservation) -> String {
    format!(
        "document_publication:{}:{}",
        source.owner_hash, source.document_id
    )
}

fn identity(source: &DocumentSourceReservation) -> Document {
    doc! {
        "_id": key(source), "document_schema_version": 1_i64, "kind": "document_publication",
        "owner_hash": &source.owner_hash, "document_id": &source.document_id,
    }
}

pub(super) async fn open(
    ledger: &Collection<Document>,
    source: &DocumentSourceReservation,
    session: &mut ClientSession,
) -> Result<(), QuotaError> {
    let mut row = identity(source);
    row.insert("published", false);
    ledger
        .update_one(doc! { "_id": key(source) }, doc! { "$setOnInsert": row })
        .upsert(true)
        .session(session)
        .await?;
    Ok(())
}

pub(super) async fn publish(
    ledger: &Collection<Document>,
    source: &DocumentSourceReservation,
    session: &mut ClientSession,
) -> Result<(), QuotaError> {
    // A predecessor slot has no durable proof of its full publication history.
    ledger
        .update_one(identity(source), doc! { "$set": { "published": true } })
        .session(session)
        .await?;
    Ok(())
}

pub(super) async fn can_release(
    ledger: &Collection<Document>,
    source: &DocumentSourceReservation,
    session: &mut ClientSession,
) -> Result<bool, QuotaError> {
    if terminal(ledger, source, session).await? {
        return Ok(true);
    }
    let Some(proof) = ledger.find_one(identity(source)).session(session).await? else {
        return Ok(false);
    };
    Ok(!proof
        .get_bool("published")
        .map_err(|_| DocumentError::Corrupt)?)
}

/// The deletion obligation survives ordinary source reclamation and account
/// erasure of the private audit. Bind it to the current baseline before use.
pub(super) async fn terminal(
    ledger: &Collection<Document>,
    source: &DocumentSourceReservation,
    session: &mut ClientSession,
) -> Result<bool, QuotaError> {
    let Some(row) = ledger
        .find_one(doc! {"_id": deletion::obligation_id(&source.owner_hash, &source.document_id)})
        .session(&mut *session)
        .await?
    else {
        return Ok(false);
    };
    quota::validate_row(&row, "deletion")?;
    let state = ledger
        .find_one(doc! {"_id":quota::STATE})
        .session(session)
        .await?
        .ok_or(DocumentError::Unavailable)?;
    if !quota::ready(&state)
        || row.get_str("owner_hash").ok() != Some(source.owner_hash.as_str())
        || row.get_str("document_id").ok() != Some(source.document_id.as_str())
        || row.get_str("activation_id").ok() != state.get_str("activation_id").ok()
        || row.get_str("inventory_sha256").ok() != state.get_str("inventory_sha256").ok()
        || !row
            .get_str("request_hash")
            .is_ok_and(layer2_core::document_artifacts::valid_digest)
        || !row
            .get_str("deleted_at")
            .is_ok_and(|v| layer2_core::seconds_from_rfc3339(v).is_some())
        || quota::number(&row, "revision")? < 2
    {
        return Err(DocumentError::Corrupt.into());
    }
    Ok(true)
}
