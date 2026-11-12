use super::*;
use mongodb::bson::{Bson, DateTime};

pub(super) fn key_hash(key: &str) -> String {
    layer2_core::document_artifacts::digest(key.as_bytes())
}
fn valid_subject_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}
pub(super) fn recover_subject_hash(document: &Document) -> Option<&str> {
    document
        .get_str("subject_hash")
        .ok()
        .filter(|hash| valid_subject_hash(hash))
}
pub(super) fn active_hash(document: &Document) -> Result<&str, AccountDeletionError> {
    let invalid = || AccountDeletionError::Store("invalid active account hash identity".into());
    let subject = document.get_str("subject_hash").map_err(|_| invalid())?;
    let active = document
        .get_str("active_subject_hash")
        .map_err(|_| invalid())?;
    if !valid_subject_hash(subject) || active != subject {
        return Err(invalid());
    }
    Ok(active)
}
pub(super) fn record_document(
    record: &AccountDeletion,
    keys: &[String],
) -> Result<Document, AccountDeletionError> {
    let mut document = mongodb::bson::serialize_to_document(record)
        .map_err(|_| AccountDeletionError::Store("encode account deletion".into()))?;
    document.remove("id");
    document.insert("_id", record.id.as_str());
    document.insert(
        "document_schema_version",
        super::super::schema::DOCUMENT_SCHEMA_VERSION,
    );
    document.insert("request_keys", keys.to_vec());
    if let Some(expiry) = &record.expire_at {
        document.insert("expire_at", date(expiry)?);
    }
    Ok(document)
}
pub(super) fn read_record(mut document: Document) -> Result<AccountDeletion, AccountDeletionError> {
    if document
        .remove("document_schema_version")
        .and_then(|version| version.as_i64())
        != Some(super::super::schema::DOCUMENT_SCHEMA_VERSION)
    {
        return Err(AccountDeletionError::Store(
            "unsupported account deletion schema".into(),
        ));
    }
    let id = document
        .remove("_id")
        .ok_or_else(|| AccountDeletionError::Store("missing account deletion identity".into()))?;
    document.insert("id", id);
    document.remove("request_keys");
    if let Some(Bson::DateTime(expiry)) = document.get("expire_at") {
        let stamp = layer2_core::rfc3339_at(expiry.to_system_time());
        document.insert("expire_at", stamp);
    }
    let record: AccountDeletion = mongodb::bson::deserialize_from_document(document)
        .map_err(|_| AccountDeletionError::Store("malformed account deletion".into()))?;
    if !layer2_core::document_artifacts::valid_id(record.id.as_str(), "acd_")
        || !valid_subject_hash(&record.subject_hash)
        || record
            .active_subject_hash
            .as_ref()
            .is_some_and(|hash| hash != &record.subject_hash)
        || record.is_active() != record.active_subject_hash.is_some()
        || record.is_active() != record.subject.is_some()
        || record
            .subject
            .as_ref()
            .is_some_and(|subject| subject_hash(subject) != record.subject_hash)
    {
        return Err(AccountDeletionError::Store(
            "inconsistent account deletion identity".into(),
        ));
    }
    Ok(record)
}
pub(super) fn request_keys(document: &Document) -> Result<Vec<String>, AccountDeletionError> {
    document
        .get_array("request_keys")
        .map_err(|_| AccountDeletionError::Store("missing request replay identities".into()))?
        .iter()
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                AccountDeletionError::Store("invalid request replay identity".into())
            })
        })
        .collect()
}
pub(super) fn date(stamp: &str) -> Result<DateTime, AccountDeletionError> {
    let seconds = layer2_core::seconds_from_rfc3339(stamp).ok_or(AccountDeletionError::Invalid)?;
    let millis = seconds
        .checked_mul(1000)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or(AccountDeletionError::Invalid)?;
    Ok(DateTime::from_millis(millis))
}
