//! Schema-v1 BSON shapes for versioned Trigger definitions.

use layer2_core::trigger_domain::{PublicTriggerId, Trigger, TriggerRevision, TriggerStoreError};
use mongodb::bson::{doc, Bson, Document};

use crate::DOCUMENT_SCHEMA_VERSION;
use cma_am_mongo_primitives::{
    i64_field_matches, read_record, record_bson, require_version, signed, string_field_matches,
    unsigned,
};

pub struct StoredTrigger {
    pub trigger: Trigger,
    pub mutation_version: u64,
}

pub struct StoredRevision {
    pub trigger_id: String,
    pub public_id: PublicTriggerId,
    pub owner: String,
    pub revision: TriggerRevision,
}

pub fn trigger_document(
    trigger: &Trigger,
    mutation_version: u64,
) -> Result<Document, TriggerStoreError> {
    trigger.validate()?;
    let latest = trigger.latest_revision();
    Ok(doc! {
        "_id": trigger.id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "owner": trigger.owner.as_str(),
        "draft_version": storage_integer(trigger.draft_version, "trigger draft version")?,
        "mutation_version": storage_integer(mutation_version, "trigger mutation version")?,
        "availability": encode(&trigger.availability)?,
        "public_id": optional_id(trigger.public_id.as_ref().map(PublicTriggerId::as_str)),
        "updated_at": &trigger.updated_at,
        "retired_at": optional_id(trigger.retired_at.as_deref()),
        "archived_at": optional_id(trigger.archived_at.as_deref()),
        "latest_revision": storage_integer(trigger.revisions.len() as u64, "latest Trigger revision")?,
        "latest_sandbox_profile_id": optional_id(latest.map(|row| {
            row.content.target.sandbox_profile.id.as_str()
        })),
        "latest_workspace_profile_id": optional_id(latest.map(|row| {
            row.content.target.workspace_profile.id.as_str()
        })),
        "latest_agent_profile_id": optional_id(latest.map(|row| {
            row.content.target.agent_profile.id.as_str()
        })),
        "launches_total": storage_integer(trigger.stats.launches_total, "Trigger launch count")?,
        "agents_ready_total": storage_integer(
            trigger.stats.agents_ready_total,
            "Trigger ready-Agent count",
        )?,
        "record": encode(trigger)?,
    })
}

pub fn read_trigger(document: &Document) -> Result<StoredTrigger, TriggerStoreError> {
    require_version(document, DOCUMENT_SCHEMA_VERSION).map_err(TriggerStoreError::Corrupt)?;
    let trigger: Trigger =
        read_record(document, DOCUMENT_SCHEMA_VERSION).map_err(TriggerStoreError::Corrupt)?;
    trigger
        .validate()
        .map_err(|error| TriggerStoreError::Corrupt(error.to_string()))?;
    let mutation_version = integer_field(document, "mutation_version")?;
    let latest = trigger.latest_revision();
    let expected_availability = encode(&trigger.availability)?;
    if mutation_version == 0
        || !string_field_matches(document, "_id", trigger.id.as_str())
        || !string_field_matches(document, "owner", trigger.owner.as_str())
        || !i64_field_matches(
            document,
            "draft_version",
            storage_integer(trigger.draft_version, "trigger draft version")?,
        )
        || document.get("availability") != Some(&expected_availability)
        || document.get("public_id")
            != Some(&optional_id(
                trigger.public_id.as_ref().map(PublicTriggerId::as_str),
            ))
        || !string_field_matches(document, "updated_at", &trigger.updated_at)
        || document.get("retired_at") != Some(&optional_id(trigger.retired_at.as_deref()))
        || document.get("archived_at").unwrap_or(&Bson::Null)
            != &optional_id(trigger.archived_at.as_deref())
        || !i64_field_matches(
            document,
            "latest_revision",
            storage_integer(trigger.revisions.len() as u64, "latest Trigger revision")?,
        )
        || document.get("latest_sandbox_profile_id")
            != Some(&optional_id(
                latest.map(|row| row.content.target.sandbox_profile.id.as_str()),
            ))
        || document.get("latest_workspace_profile_id")
            != Some(&optional_id(
                latest.map(|row| row.content.target.workspace_profile.id.as_str()),
            ))
        || document.get("latest_agent_profile_id")
            != Some(&optional_id(
                latest.map(|row| row.content.target.agent_profile.id.as_str()),
            ))
        || !counter_matches(document, "launches_total", trigger.stats.launches_total)?
        || !counter_matches(
            document,
            "agents_ready_total",
            trigger.stats.agents_ready_total,
        )?
    {
        return Err(TriggerStoreError::Corrupt(
            "Trigger query fields disagree with its aggregate".into(),
        ));
    }
    Ok(StoredTrigger {
        trigger,
        mutation_version,
    })
}

pub fn revision_document(
    trigger: &Trigger,
    revision: &TriggerRevision,
) -> Result<Document, TriggerStoreError> {
    revision.validate()?;
    let public_id = trigger
        .public_id
        .as_ref()
        .ok_or_else(|| TriggerStoreError::Corrupt("published Trigger has no public ID".into()))?;
    Ok(doc! {
        "_id": revision_key(trigger.id.as_str(), revision.revision),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "trigger_id": trigger.id.as_str(),
        "public_id": public_id.as_str(),
        "owner": trigger.owner.as_str(),
        "revision": i64::from(revision.revision),
        "sandbox_profile_id": revision.content.target.sandbox_profile.id.as_str(),
        "workspace_profile_id": revision.content.target.workspace_profile.id.as_str(),
        "agent_profile_id": revision.content.target.agent_profile.id.as_str(),
        "published_at": &revision.published_at,
        "record": encode(revision)?,
    })
}

pub fn revision_fenced(row: &Document) -> Result<bool, TriggerStoreError> {
    match (
        row.get("account_erasure_fenced"),
        row.get("account_erasure_fenced_at"),
    ) {
        (None | Some(Bson::Boolean(false)), None) => Ok(false),
        (Some(Bson::Boolean(true)), Some(Bson::DateTime(_))) => Ok(true),
        _ => Err(TriggerStoreError::Corrupt(
            "invalid publisher erasure fence authority".into(),
        )),
    }
}

pub fn read_revision(document: &Document) -> Result<StoredRevision, TriggerStoreError> {
    require_version(document, DOCUMENT_SCHEMA_VERSION).map_err(TriggerStoreError::Corrupt)?;
    let revision: TriggerRevision =
        read_record(document, DOCUMENT_SCHEMA_VERSION).map_err(TriggerStoreError::Corrupt)?;
    revision
        .validate()
        .map_err(|error| TriggerStoreError::Corrupt(error.to_string()))?;
    revision_fenced(document)?;
    let trigger_id = string_field(document, "trigger_id")?;
    let public_id = PublicTriggerId(string_field(document, "public_id")?);
    let owner = string_field(document, "owner")?;
    if revision.publisher_deleted
        && (owner != layer2_core::DELETED_ACCOUNT_SUBJECT
            || document
                .get_i64("mutation_version")
                .ok()
                .is_none_or(|value| value < 1))
    {
        return Err(TriggerStoreError::Corrupt(
            "invalid publisher tombstone authority".into(),
        ));
    }
    if !string_field_matches(
        document,
        "_id",
        &revision_key(&trigger_id, revision.revision),
    ) || !i64_field_matches(document, "revision", i64::from(revision.revision))
        || !string_field_matches(document, "published_at", &revision.published_at)
        || !string_field_matches(
            document,
            "sandbox_profile_id",
            revision.content.target.sandbox_profile.id.as_str(),
        )
        || !string_field_matches(
            document,
            "workspace_profile_id",
            revision.content.target.workspace_profile.id.as_str(),
        )
        || !string_field_matches(
            document,
            "agent_profile_id",
            revision.content.target.agent_profile.id.as_str(),
        )
    {
        return Err(TriggerStoreError::Corrupt(
            "Trigger revision query fields disagree with its record".into(),
        ));
    }
    Ok(StoredRevision {
        trigger_id,
        public_id,
        owner,
        revision,
    })
}

pub fn revision_key(trigger_id: &str, revision: u32) -> String {
    format!("{trigger_id}:{revision:010}")
}

fn encode<T: serde::Serialize>(value: &T) -> Result<Bson, TriggerStoreError> {
    record_bson(value).map_err(TriggerStoreError::Corrupt)
}

fn optional_id(value: Option<&str>) -> Bson {
    value
        .map(str::to_owned)
        .map(Bson::String)
        .unwrap_or(Bson::Null)
}

fn storage_integer(value: u64, field: &str) -> Result<i64, TriggerStoreError> {
    signed(value, field).map_err(TriggerStoreError::Corrupt)
}

fn integer_field(document: &Document, field: &str) -> Result<u64, TriggerStoreError> {
    document
        .get_i64(field)
        .map_err(|error| TriggerStoreError::Corrupt(error.to_string()))
        .and_then(|value| unsigned(value, field).map_err(TriggerStoreError::Corrupt))
}

fn string_field(document: &Document, field: &str) -> Result<String, TriggerStoreError> {
    document
        .get_str(field)
        .map(str::to_owned)
        .map_err(|error| TriggerStoreError::Corrupt(error.to_string()))
}

fn counter_matches(
    document: &Document,
    field: &str,
    expected: u64,
) -> Result<bool, TriggerStoreError> {
    Ok(i64_field_matches(
        document,
        field,
        storage_integer(expected, field)?,
    ))
}
