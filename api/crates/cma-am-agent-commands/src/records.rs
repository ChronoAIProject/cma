use layer2_core::sandbox::agent_io::{AdmittedAgentCommand, CommandLedgerError};
use mongodb::bson::{doc, Document};
use mongodb::options::IndexOptions;
use mongodb::{Collection, IndexModel};

use cma_am_mongo_primitives::{read_record, record_bson, signed};

use crate::DOCUMENT_SCHEMA_VERSION;

pub fn document(record: &AdmittedAgentCommand) -> Result<Document, CommandLedgerError> {
    record.validate()?;
    let envelope = &record.receipt.envelope;
    let mut compatible = record.clone();
    let budget_failure = compatible.work.failure.as_mut().and_then(|failure| {
        failure.code.budget_error().inspect(|_| {
            failure.code = failure.code.legacy_code();
        })
    });
    let capacity_failure = compatible.work.failure.as_mut().is_some_and(|failure| {
        if failure.code
            == layer2_core::sandbox::agent_io::CommandFailureCode::SessionCapacityUnavailable
        {
            failure.code = failure.code.legacy_code();
            true
        } else {
            false
        }
    });
    let mut document = doc! {
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
        "record": record_bson(&compatible).map_err(|_| CommandLedgerError::Corrupt)?,
    };
    if let Some(reason) = budget_failure {
        document.insert(
            "model_budget_failure",
            record_bson(&reason).map_err(|_| CommandLedgerError::Corrupt)?,
        );
    }
    if capacity_failure {
        document.insert("admission_capacity_failure", "session_capacity_unavailable");
    }
    if let Some(input) = &record.queued_input {
        document.insert(
            "queued_input",
            record_bson(input).map_err(|_| CommandLedgerError::Corrupt)?,
        );
    }
    if let Some(key) = envelope.external_event_key() {
        document.insert("external_event_key", key);
    }
    // Older binaries reject this strict-record marker before claiming the
    // command. Ignoring the additive sidecar must never dispatch original text.
    if guarded_input(record) {
        document
            .get_document_mut("record")
            .map_err(|_| CommandLedgerError::Corrupt)?
            .insert("queued_input_required", 1_i32);
    }
    Ok(document)
}

pub fn read(value: &Document) -> Result<AdmittedAgentCommand, CommandLedgerError> {
    let mut retained = value.clone();
    let marker = retained
        .get_document_mut("record")
        .map_err(|_| CommandLedgerError::Corrupt)?
        .remove("queued_input_required");
    let mut record: AdmittedAgentCommand =
        read_record(&retained, DOCUMENT_SCHEMA_VERSION).map_err(|_| CommandLedgerError::Corrupt)?;
    record.queued_input = value
        .get("queued_input")
        .map(|input| {
            mongodb::bson::deserialize_from_bson(input.clone())
                .map_err(|_| CommandLedgerError::Corrupt)
        })
        .transpose()?;
    if marker != guarded_input(&record).then_some(mongodb::bson::Bson::Int32(1)) {
        return Err(CommandLedgerError::Corrupt);
    }
    if let Some(value) = value.get("model_budget_failure") {
        use layer2_core::sandbox::agent_io::{AgentResponseStatus, CommandFailureCode};
        let reason: layer2_core::budget::BudgetError =
            mongodb::bson::deserialize_from_bson(value.clone())
                .map_err(|_| CommandLedgerError::Corrupt)?;
        let failure = record
            .work
            .failure
            .as_mut()
            .ok_or(CommandLedgerError::Corrupt)?;
        if record.status != AgentResponseStatus::Failed
            || record.work.dispatch.is_some()
            || failure.code != CommandFailureCode::ExecutionFailed
        {
            return Err(CommandLedgerError::Corrupt);
        }
        failure.code = CommandFailureCode::from_budget(reason);
    }
    if let Some(value) = value.get("admission_capacity_failure") {
        use layer2_core::sandbox::agent_io::{AgentResponseStatus, CommandFailureCode};
        let failure = record
            .work
            .failure
            .as_mut()
            .ok_or(CommandLedgerError::Corrupt)?;
        if value.as_str() != Some("session_capacity_unavailable")
            || record.status != AgentResponseStatus::Failed
            || record.work.dispatch.is_some()
            || failure.code != CommandFailureCode::ExecutionFailed
        {
            return Err(CommandLedgerError::Corrupt);
        }
        failure.code = CommandFailureCode::SessionCapacityUnavailable;
    }
    let expected = document(&record)?;
    if value.get("external_event_key") != expected.get("external_event_key")
        || expected
            .iter()
            .any(|(key, field)| key != "record" && value.get(key) != Some(field))
    {
        return Err(CommandLedgerError::Corrupt);
    }
    Ok(record)
}

fn guarded_input(record: &AdmittedAgentCommand) -> bool {
    record.queued_input.is_some() && !record.status.is_terminal() && record.work.dispatch.is_none()
}

pub async fn ensure_indexes(
    collection: &Collection<Document>,
) -> Result<(), layer2_core::StoreError> {
    let definitions = [
        (
            "agent_command_observations",
            doc! { "status": 1, "record.receipt.admitted_at": 1, "queue_deadline": 1 },
            false,
        ),
        (
            "agent_command_answer_admission",
            doc! { "agent_id": 1, "record.receipt.envelope.command.type": 1, "record.receipt.event_cursor": 1 },
            false,
        ),
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
        (
            "agent_command_retained_authority",
            doc! { "record.receipt.envelope.agent_id": 1, "record.status": 1 },
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
    indexes.push(
        IndexModel::builder()
            .keys(doc! { "external_event_key": 1 })
            .options(
                IndexOptions::builder()
                    .name("agent_command_external_event".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "external_event_key": { "$type": "string" } })
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
