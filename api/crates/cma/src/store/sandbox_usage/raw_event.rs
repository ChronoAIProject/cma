//! Validate an allocation event directly from the driver's borrowed BSON bytes.
use super::super::sandbox_mongo::DOCUMENT_SCHEMA_VERSION;
use layer2_core::{sandbox::store::SandboxStoreError, sandbox_usage::UsageEvent};
use mongodb::bson::{self, RawBsonRef, RawDocument};

fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("usage event envelope or record is invalid".into())
}

pub(in crate::store) fn read(document: &RawDocument) -> Result<UsageEvent, SandboxStoreError> {
    // One envelope pass preserves exact BSON query-field types. Ambiguous
    // duplicate authority fields cannot provide a trusted lookup envelope.
    let mut fields = [None; 8];
    for element in document {
        let (key, value) = element.map_err(|_| corrupt())?;
        let position = match key.as_str() {
            "document_schema_version" => 0,
            "record" => 1,
            "_id" => 2,
            "owner" => 3,
            "sandbox_id" => 4,
            "revision" => 5,
            "started_at" => 6,
            "ended_at" => 7,
            _ => {
                // Match the ordinary Document reader's validation of additional
                // envelope metadata, including malformed nested BSON.
                let _ = bson::Bson::try_from(value).map_err(|_| corrupt())?;
                continue;
            }
        };
        if fields[position].replace(value).is_some() {
            return Err(corrupt());
        }
    }
    let version = match fields[0] {
        Some(RawBsonRef::Int32(version)) => i64::from(version),
        Some(RawBsonRef::Int64(version)) => version,
        _ => return Err(corrupt()),
    };
    if version != DOCUMENT_SCHEMA_VERSION {
        return Err(corrupt());
    }
    let Some(RawBsonRef::Document(record)) = fields[1] else {
        return Err(corrupt());
    };
    let event: UsageEvent =
        bson::deserialize_from_slice(record.as_bytes()).map_err(|_| corrupt())?;
    let revision = i64::try_from(event.revision).map_err(|_| corrupt())?;
    let started_at = i64::try_from(event.started_at).map_err(|_| corrupt())?;
    let ended_at = i64::try_from(event.ended_at).map_err(|_| corrupt())?;
    let id = event.transaction_id();
    let expected = [
        RawBsonRef::String(&id),
        RawBsonRef::String(event.owner.as_str()),
        RawBsonRef::String(event.sandbox_id.as_str()),
        RawBsonRef::Int64(revision),
        RawBsonRef::Int64(started_at),
        RawBsonRef::Int64(ended_at),
    ];
    if !fields[2..]
        .iter()
        .zip(expected)
        .all(|(actual, expected)| *actual == Some(expected))
    {
        return Err(corrupt());
    }
    Ok(event)
}

#[cfg(test)]
mod tests;
