//! Read the durable admission timestamp and immutable desktop intent.
use super::*;
use mongodb::bson::Bson;

pub(super) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
) -> Result<Option<layer2_core::sandbox::store::AgentCreateAdmission>, SandboxStoreError> {
    let layer2_core::sandbox::AgentOrigin::Lifecycle { operation_id } = &agent.origin else {
        return Ok(None);
    };
    let Some(record) = store
        .lifecycle_operations
        .find_one(doc! { "_id": operation_id })
        .projection(
            doc! { "created_at": 1, "owner": 1, "input.request.desktop": 1,
            "input.request.machine_included": 1 },
        )
        .max_time(Duration::from_millis(200))
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    else {
        return Ok(None);
    };
    if record.get_str("owner").ok() != Some(agent.owner.as_str()) {
        return Ok(None);
    }
    let admitted_at_ms = record
        .get_datetime("created_at")
        .ok()
        .map(|value| value.timestamp_millis());
    let request = record
        .get_document("input")
        .ok()
        .and_then(|input| input.get_document("request").ok());
    let valid_bool = |key: &str| {
        request
            .and_then(|request| request.get(key))
            .is_none_or(|value| matches!(value, Bson::Boolean(_)))
    };
    let (desktop, machine_included) = match request {
        Some(request) if valid_bool("desktop") && valid_bool("machine_included") => (
            Some(request.get_bool("desktop").unwrap_or(false)),
            Some(request.get_bool("machine_included").unwrap_or(false)),
        ),
        _ => (None, None),
    };
    Ok(admitted_at_ms.map(
        |admitted_at_ms| layer2_core::sandbox::store::AgentCreateAdmission {
            admitted_at_ms,
            desktop,
            machine_included,
        },
    ))
}
