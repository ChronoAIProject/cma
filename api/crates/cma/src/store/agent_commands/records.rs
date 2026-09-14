use layer2_core::sandbox::agent_io::{AdmittedAgentCommand, CommandLedgerError};
use mongodb::bson::{doc, Document};
use mongodb::options::IndexOptions;
use mongodb::{Collection, IndexModel};

use super::super::sandbox_mongo::{read_record, record_bson, signed, DOCUMENT_SCHEMA_VERSION};

pub(super) fn document(record: &AdmittedAgentCommand) -> Result<Document, CommandLedgerError> {
    record.validate()?;
    let envelope = &record.receipt.envelope;
    Ok(doc! {
        "_id": envelope.command_id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "agent_id": envelope.agent_id.as_str(),
        "scope": record.scope(),
        "key_sha256": &record.key_sha256,
        "agent_order": signed(record.receipt.agent_order, "command order").map_err(|_| CommandLedgerError::Corrupt)?,
        "response_id": record.receipt.response_id.as_ref().map(|id| id.as_str()),
        "status": record_bson(&record.status).map_err(|_| CommandLedgerError::Corrupt)?,
        "queue_deadline": &record.queue_deadline,
        "revision": signed(record.revision, "command revision").map_err(|_| CommandLedgerError::Corrupt)?,
        "record": record_bson(record).map_err(|_| CommandLedgerError::Corrupt)?,
    })
}

pub(super) fn read(value: &Document) -> Result<AdmittedAgentCommand, CommandLedgerError> {
    let record: AdmittedAgentCommand =
        read_record(value).map_err(|_| CommandLedgerError::Corrupt)?;
    let expected = document(&record)?;
    if expected
        .iter()
        .any(|(key, field)| key != "record" && value.get(key) != Some(field))
    {
        return Err(CommandLedgerError::Corrupt);
    }
    Ok(record)
}

pub(in crate::store) async fn ensure_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    let definitions = [
        (
            "agent_command_work_scan",
            doc! { "status": 1, "_id": 1 },
            false,
        ),
        (
            "agent_command_idempotency",
            doc! { "scope": 1, "key_sha256": 1 },
            true,
        ),
        (
            "agent_command_order",
            doc! { "agent_id": 1, "agent_order": 1 },
            true,
        ),
        (
            "agent_command_queue",
            doc! { "agent_id": 1, "status": 1, "agent_order": 1 },
            false,
        ),
    ];
    let mut indexes: Vec<_> = definitions
        .into_iter()
        .map(|(name, keys, unique)| {
            IndexModel::builder()
                .keys(keys)
                .options(
                    IndexOptions::builder()
                        .name(name.to_string())
                        .unique(unique)
                        .build(),
                )
                .build()
        })
        .collect();
    indexes.push(
        IndexModel::builder()
            .keys(doc! { "response_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("agent_command_response".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "response_id": { "$type": "string" } })
                    .build(),
            )
            .build(),
    );
    collection
        .create_indexes(indexes)
        .await
        .map(|_| ())
        .map_err(|_| layer2_core::StoreError::Unavailable("create Agent command indexes".into()))
}
