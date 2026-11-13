use super::super::sandbox_mongo::{read_record, record_bson, DOCUMENT_SCHEMA_VERSION};
use layer2_core::sandbox::control::{SandboxControlError, SandboxControlRecord};
use mongodb::bson::{doc, Document};
use mongodb::options::IndexOptions;
use mongodb::{Collection, IndexModel};

pub(super) fn document(record: &SandboxControlRecord) -> Result<Document, SandboxControlError> {
    record.validate()?;
    Ok(doc! {
        "_id": record.envelope.command_id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "sandbox_id": record.envelope.sandbox_id.as_str(),
        "scope": record.envelope.scope(),
        "key_sha256": &record.key_sha256,
        "queued": record.queued(),
        "revision": record.revision as i64,
        "queue_deadline": &record.queue_deadline,
        "record": record_bson(record).map_err(|_| SandboxControlError::Corrupt)?,
    })
}

pub(super) fn read(value: &Document) -> Result<SandboxControlRecord, SandboxControlError> {
    let record: SandboxControlRecord =
        read_record(value).map_err(|_| SandboxControlError::Corrupt)?;
    let expected = document(&record)?;
    if expected
        .iter()
        .any(|(key, field)| key != "record" && value.get(key) != Some(field))
    {
        return Err(SandboxControlError::Corrupt);
    }
    Ok(record)
}

pub(in crate::store) async fn ensure_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    let mut indexes = Vec::new();
    for (name, keys, unique, partial) in [
        (
            "sandbox_control_observations",
            doc! { "queued": 1, "record.admitted_at": 1, "queue_deadline": 1 },
            false,
            None,
        ),
        (
            "sandbox_control_idempotency",
            doc! { "scope": 1, "key_sha256": 1 },
            true,
            None,
        ),
        (
            "sandbox_control_discovery",
            doc! { "queued": 1, "_id": 1 },
            false,
            None,
        ),
        (
            "sandbox_control_pending",
            doc! { "sandbox_id": 1 },
            true,
            Some(doc! { "queued": true }),
        ),
    ] {
        indexes.push(
            IndexModel::builder()
                .keys(keys)
                .options(
                    IndexOptions::builder()
                        .name(name.to_owned())
                        .unique(unique)
                        .partial_filter_expression(partial)
                        .build(),
                )
                .build(),
        );
    }
    collection
        .create_indexes(indexes)
        .await
        .map(|_| ())
        .map_err(|_| layer2_core::StoreError::Unavailable("create Sandbox control indexes".into()))
}
