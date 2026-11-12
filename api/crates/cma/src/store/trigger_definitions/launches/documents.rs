//! Schema-v1 BSON shapes for Launch runtime records.

use layer2_core::ids::LaunchId;
use layer2_core::trigger_domain::{
    Launch, LaunchEvent, LaunchStoreError, ServiceConnectionHandoff,
};
use mongodb::bson::{doc, Bson, Document};

use crate::store::sandbox_mongo::{
    i64_field_matches, read_record, record_bson, require_version, signed, string_field_matches,
    unsigned,
};
use crate::store::schema::DOCUMENT_SCHEMA_VERSION;

use super::corrupt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StoredLaunch {
    pub launch: Launch,
    pub mutation_version: u64,
    pub home_capacity_admission: bool,
}

pub(super) fn launch_document(
    launch: &Launch,
    mutation_version: u64,
    home_capacity_admission: bool,
) -> Result<Document, LaunchStoreError> {
    launch.validate()?;
    let lease = launch.work.lease.as_ref();
    let mut document = doc! {
        "_id": launch.id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "owner": launch.owner.as_str(),
        "trigger_id": launch.trigger_id.as_str(),
        "public_id": launch.public_id.as_str(),
        "command_key_hash": &launch.command_key_hash,
        "command_fingerprint": &launch.command_fingerprint,
        "state": encode(&launch.state)?,
        "created_at": &launch.created_at,
        "blocked_reason": encode(&launch.blocked_reason)?,
        "work_step": encode(&launch.work.step)?,
        "work_ready_at": &launch.work.ready_at,
        "work_lease_holder": optional_string(lease.map(|row| row.holder.as_str())),
        "work_lease_fence": optional_integer(
            lease.map(|row| row.fence),
            "Launch work lease fence",
        )?,
        "work_lease_expires_at": optional_string(lease.map(|row| row.expires_at.as_str())),
        "ready_at": optional_string(launch.ready_at.as_deref()),
        "event_head": storage_integer(launch.event_head, "Launch event head")?,
        "revision": storage_integer(launch.revision, "Launch revision")?,
        "mutation_version": storage_integer(mutation_version, "Launch mutation version")?,
        "record": encode(launch)?,
    };
    if home_capacity_admission {
        document.insert("home_capacity_admission", true);
    }
    if let Some(initiation) = &launch.initiation {
        document.insert("launch_initiation", encode(initiation)?);
    }
    if let Some(protocol) = &launch.input_protocol {
        document.insert("trigger_input_protocol", encode(protocol)?);
    }
    if let Some(binding) = &launch.input_binding_sha256 {
        document.insert("trigger_input_binding_sha256", binding);
    }
    Ok(document)
}

pub(super) fn read_launch(document: &Document) -> Result<StoredLaunch, LaunchStoreError> {
    require_version(document).map_err(corrupt)?;
    let mut launch: Launch = read_record(document).map_err(corrupt)?;
    launch.initiation = document
        .get("launch_initiation")
        .map(|value| {
            mongodb::bson::deserialize_from_bson(value.clone())
                .map_err(|error| corrupt(error.to_string()))
        })
        .transpose()?;
    launch.input_protocol = document
        .get("trigger_input_protocol")
        .map(|value| {
            mongodb::bson::deserialize_from_bson(value.clone())
                .map_err(|error| corrupt(error.to_string()))
        })
        .transpose()?;
    launch.input_binding_sha256 = document
        .get("trigger_input_binding_sha256")
        .map(|value| {
            mongodb::bson::deserialize_from_bson(value.clone())
                .map_err(|error| corrupt(error.to_string()))
        })
        .transpose()?;
    launch
        .validate()
        .map_err(|error| corrupt(error.to_string()))?;
    let mutation_version = integer_field(document, "mutation_version")?;
    let lease = launch.work.lease.as_ref();
    if mutation_version == 0
        || !string_field_matches(document, "_id", launch.id.as_str())
        || !string_field_matches(document, "owner", launch.owner.as_str())
        || !string_field_matches(document, "trigger_id", launch.trigger_id.as_str())
        || !string_field_matches(document, "public_id", launch.public_id.as_str())
        || !string_field_matches(document, "command_key_hash", &launch.command_key_hash)
        || !string_field_matches(document, "command_fingerprint", &launch.command_fingerprint)
        || document.get("state") != Some(&encode(&launch.state)?)
        || !string_field_matches(document, "created_at", &launch.created_at)
        || document.get("blocked_reason") != Some(&encode(&launch.blocked_reason)?)
        || document.get("work_step") != Some(&encode(&launch.work.step)?)
        || !string_field_matches(document, "work_ready_at", &launch.work.ready_at)
        || document.get("work_lease_holder")
            != Some(&optional_string(lease.map(|row| row.holder.as_str())))
        || document.get("work_lease_fence")
            != Some(&optional_integer(
                lease.map(|row| row.fence),
                "Launch work lease fence",
            )?)
        || document.get("work_lease_expires_at")
            != Some(&optional_string(lease.map(|row| row.expires_at.as_str())))
        || document.get("ready_at") != Some(&optional_string(launch.ready_at.as_deref()))
        || !integer_matches(document, "event_head", launch.event_head)?
        || !integer_matches(document, "revision", launch.revision)?
    {
        return Err(corrupt(
            "Launch query fields disagree with its aggregate record",
        ));
    }
    let home_capacity_admission = match document.get("home_capacity_admission") {
        None => false,
        Some(Bson::Boolean(true)) => true,
        Some(_) => return Err(corrupt("invalid Launch home capacity marker")),
    };
    Ok(StoredLaunch {
        launch,
        mutation_version,
        home_capacity_admission,
    })
}

pub(super) fn event_document(event: &LaunchEvent) -> Result<Document, LaunchStoreError> {
    if event.seq == 0 {
        return Err(corrupt("Launch event sequence must be positive"));
    }
    Ok(doc! {
        "_id": event_key(&event.launch_id, event.seq),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "launch_id": event.launch_id.as_str(),
        "seq": storage_integer(event.seq, "Launch event sequence")?,
        "appended_at": &event.appended_at,
        "record": encode(event)?,
    })
}

pub(super) fn read_event(document: &Document) -> Result<LaunchEvent, LaunchStoreError> {
    require_version(document).map_err(corrupt)?;
    let event: LaunchEvent = read_record(document).map_err(corrupt)?;
    if event.seq == 0
        || !string_field_matches(document, "_id", &event_key(&event.launch_id, event.seq))
        || !string_field_matches(document, "launch_id", event.launch_id.as_str())
        || !integer_matches(document, "seq", event.seq)?
        || !string_field_matches(document, "appended_at", &event.appended_at)
    {
        return Err(corrupt(
            "Launch event query fields disagree with its record",
        ));
    }
    Ok(event)
}

pub(super) fn handoff_document(
    handoff: &ServiceConnectionHandoff,
) -> Result<Document, LaunchStoreError> {
    handoff.validate()?;
    Ok(doc! {
        "_id": handoff.id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "owner": handoff.owner.as_str(),
        "launch_id": handoff.launch_id.as_str(),
        "service_id": handoff.service_id.as_str(),
        "intent_id": &handoff.intent_id,
        "external_id": &handoff.external_id,
        "state": encode(&handoff.state)?,
        "expires_at": &handoff.expires_at,
        "created_at": &handoff.created_at,
        "updated_at": &handoff.updated_at,
        "revision": storage_integer(handoff.revision, "service handoff revision")?,
        "record": encode(handoff)?,
    })
}

pub(super) fn read_handoff(
    document: &Document,
) -> Result<ServiceConnectionHandoff, LaunchStoreError> {
    require_version(document).map_err(corrupt)?;
    let handoff: ServiceConnectionHandoff = read_record(document).map_err(corrupt)?;
    handoff
        .validate()
        .map_err(|error| corrupt(error.to_string()))?;
    if !string_field_matches(document, "_id", handoff.id.as_str())
        || !string_field_matches(document, "owner", handoff.owner.as_str())
        || !string_field_matches(document, "launch_id", handoff.launch_id.as_str())
        || !string_field_matches(document, "service_id", handoff.service_id.as_str())
        || !string_field_matches(document, "intent_id", &handoff.intent_id)
        || !string_field_matches(document, "external_id", &handoff.external_id)
        || document.get("state") != Some(&encode(&handoff.state)?)
        || !string_field_matches(document, "expires_at", &handoff.expires_at)
        || !string_field_matches(document, "created_at", &handoff.created_at)
        || !string_field_matches(document, "updated_at", &handoff.updated_at)
        || !integer_matches(document, "revision", handoff.revision)?
    {
        return Err(corrupt(
            "service handoff query fields disagree with its record",
        ));
    }
    Ok(handoff)
}

pub(super) fn event_key(id: &LaunchId, seq: u64) -> String {
    format!("{id}:{seq:020}")
}

fn encode<T: serde::Serialize>(value: &T) -> Result<Bson, LaunchStoreError> {
    record_bson(value).map_err(corrupt)
}

fn storage_integer(value: u64, field: &str) -> Result<i64, LaunchStoreError> {
    signed(value, field).map_err(corrupt)
}

fn integer_field(document: &Document, field: &str) -> Result<u64, LaunchStoreError> {
    document
        .get_i64(field)
        .map_err(|error| corrupt(error.to_string()))
        .and_then(|value| unsigned(value, field).map_err(corrupt))
}

fn integer_matches(
    document: &Document,
    field: &str,
    expected: u64,
) -> Result<bool, LaunchStoreError> {
    Ok(i64_field_matches(
        document,
        field,
        storage_integer(expected, field)?,
    ))
}

fn optional_string(value: Option<&str>) -> Bson {
    value
        .map(str::to_owned)
        .map(Bson::String)
        .unwrap_or(Bson::Null)
}

fn optional_integer(value: Option<u64>, field: &str) -> Result<Bson, LaunchStoreError> {
    value
        .map(|value| storage_integer(value, field).map(Bson::Int64))
        .transpose()
        .map(|value| value.unwrap_or(Bson::Null))
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::ids::{ServiceHandoffId, ServiceId, SubjectId};
    use layer2_core::trigger_domain::{BrokerHandoffState, ServiceConnectionHandoff};

    #[test]
    fn event_keys_sort_by_sequence() {
        let id = LaunchId("lch_one".into());
        assert!(event_key(&id, 9) < event_key(&id, 10));
        assert!(event_key(&id, 10) < event_key(&id, 1_000));
    }

    #[test]
    fn handoff_query_fields_and_schema_are_strict() {
        let handoff = ServiceConnectionHandoff {
            id: ServiceHandoffId("hnd_one".into()),
            launch_id: LaunchId("lch_one".into()),
            owner: SubjectId::new("owner"),
            service_id: ServiceId::new("github"),
            intent_id: "intent-one".into(),
            hosted_url: "https://connection.example/one".into(),
            expires_at: "2026-09-05T01:00:00Z".into(),
            state: BrokerHandoffState::Pending,
            created_at: "2026-09-05T00:00:00Z".into(),
            updated_at: "2026-09-05T00:00:00Z".into(),
            external_id: "external-one".into(),
            revision: 1,
        };
        let document = handoff_document(&handoff).unwrap();
        assert_eq!(read_handoff(&document).unwrap(), handoff);

        let mut corrupt_owner = document.clone();
        corrupt_owner.insert("owner", "someone-else");
        assert!(matches!(
            read_handoff(&corrupt_owner),
            Err(LaunchStoreError::Corrupt(_))
        ));
        let mut future = document;
        future.insert("document_schema_version", 2_i64);
        assert!(matches!(
            read_handoff(&future),
            Err(LaunchStoreError::Corrupt(_))
        ));
    }
}
