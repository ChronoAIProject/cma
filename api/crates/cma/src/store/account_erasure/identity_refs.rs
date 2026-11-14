//! Precise attribution redaction on resources and audits retained by other owners.
use super::{references::Redaction, Error};
use crate::store::{durability::majority_collection, resource_access::records};
use layer2_core::{
    resource_access::{ResourceActor, ResourceAuthorizationAudit, ResourceAuthorizationBasis},
    sandbox::{
        agent_io::CommandAuthorizationAudit, binding::ProviderBinding, AgentCreationOrigin,
        AgentCreationProvenance,
    },
    SubjectId,
};
use mongodb::{
    bson::{self, doc, Bson, Document},
    Database,
};
use std::collections::BTreeSet;

#[path = "identity_audits.rs"]
mod audits;
#[path = "identity_channels.rs"]
mod channels;
#[path = "identity_shares.rs"]
mod shares;

pub(super) async fn redact(
    db: &Database,
    collection: &str,
    row: &Document,
    subjects: &BTreeSet<String>,
) -> Result<Redaction, Error> {
    let replacement = match collection {
        "resource_policies" => policy(row, subjects)?,
        "provider_bindings" => binding(row, subjects)?,
        "agent_creation_provenance" => provenance(row, subjects)?,
        "agent_commands" => command(row, subjects)?,
        "command_authorization_audits" => audit(row, subjects)?,
        "resource_shares" | "resource_share_commands" => {
            shares::replacement(collection, row, subjects)?
        }
        "sandbox_controls" | "agent_artifacts" | "bots" => {
            audits::replacement(collection, row, subjects)?
        }
        "activity" => super::identity_activity::replacement(db, row, subjects).await?,
        "agent_lifecycle_operations" => lifecycle(db, row, subjects).await?,
        "agent_channel_usage" | "channel_usage_events" => {
            channels::replacement(collection, row, subjects)?
        }
        _ => None,
    };
    let Some(mut replacement) = replacement else {
        return Ok(Redaction::Unchanged);
    };
    let name = name(collection)?;
    let (filter, version) = cas(row, name)?;
    replacement.insert("mutation_version", version);
    let result = majority_collection::<Document>(db, collection)
        .replace_one(filter, replacement)
        .await?;
    Ok(if result.matched_count == 1 {
        Redaction::Updated
    } else {
        Redaction::Conflict
    })
}

fn name(collection: &str) -> Result<&'static str, Error> {
    match collection {
        "resource_policies" => Ok("resource_policies"),
        "provider_bindings" => Ok("provider_bindings"),
        "agent_creation_provenance" => Ok("agent_creation_provenance"),
        "agent_commands" => Ok("agent_commands"),
        "command_authorization_audits" => Ok("command_authorization_audits"),
        "resource_shares" => Ok("resource_shares"),
        "resource_share_commands" => Ok("resource_share_commands"),
        "agent_lifecycle_operations" => Ok("agent_lifecycle_operations"),
        "agent_channel_usage" => Ok("agent_channel_usage"),
        "channel_usage_events" => Ok("channel_usage_events"),
        "sandbox_controls" => Ok("sandbox_controls"),
        "agent_artifacts" => Ok("agent_artifacts"),
        "bots" => Ok("bots"),
        "activity" => Ok("activity"),
        _ => Err(Error::InvalidRecord("identity references")),
    }
}

fn cas(row: &Document, collection: &'static str) -> Result<(Document, i64), Error> {
    if !row.contains_key("_id") {
        return Err(Error::InvalidRecord(collection));
    }
    let filter = doc! {"_id":row.get("_id").cloned().ok_or(Error::InvalidRecord(collection))?,
    "$expr":{"$eq":["$$ROOT",{"$literal":row.clone()}]}};
    let current = match row.get("mutation_version") {
        None => 0,
        Some(Bson::Int64(n)) if *n > 0 => *n,
        _ => return Err(Error::InvalidRecord(collection)),
    };
    Ok((
        filter,
        current
            .checked_add(1)
            .ok_or(Error::InvalidRecord(collection))?,
    ))
}

fn has_subject(row: &Document, paths: &[&str], subjects: &BTreeSet<String>) -> bool {
    paths.iter().any(|path| {
        super::selectors::strings(row, path)
            .iter()
            .any(|subject| subjects.contains(*subject))
    })
}
pub(super) fn mentions_activity(row: &Document, subjects: &BTreeSet<String>) -> bool {
    has_subject(row, &["record.actor", "record.detail.subject"], subjects)
}

pub(super) fn actor(actor: &mut ResourceActor, subjects: &BTreeSet<String>) -> bool {
    match actor {
        ResourceActor::Human { subject } | ResourceActor::Trigger { subject, .. }
            if subjects.contains(subject.as_str()) =>
        {
            *subject = SubjectId::new(layer2_core::DELETED_ACCOUNT_SUBJECT);
            true
        }
        ResourceActor::Api {
            subject,
            delegation_id,
        } if subjects.contains(subject.as_str()) => {
            *subject = SubjectId::new(layer2_core::DELETED_ACCOUNT_SUBJECT);
            *delegation_id = None;
            true
        }
        _ => false,
    }
}

pub(super) fn creation(record: &mut AgentCreationProvenance, subjects: &BTreeSet<String>) -> bool {
    let mut changed = record
        .created_by
        .as_mut()
        .is_some_and(|value| actor(value, subjects));
    if let AgentCreationOrigin::Trigger {
        launching_actor: Some(value),
        ..
    } = &mut record.creation_origin
    {
        changed |= actor(value, subjects);
    }
    changed
}

fn encoded<T: serde::Serialize>(
    row: &Document,
    record: &T,
    collection: &'static str,
) -> Result<Document, Error> {
    let mut replacement = row.clone();
    replacement.insert(
        "record",
        bson::serialize_to_bson(record).map_err(|_| Error::InvalidRecord(collection))?,
    );
    Ok(replacement)
}

fn policy(row: &Document, subjects: &BTreeSet<String>) -> Result<Option<Document>, Error> {
    const NAME: &str = "resource_policies";
    if !has_subject(
        row,
        &["record.organization_evidence.admitted_by.subject"],
        subjects,
    ) {
        return Ok(None);
    }
    let mut record = records::read_policy(row).map_err(|_| Error::InvalidRecord(NAME))?;
    let Some(evidence) = &mut record.organization_evidence else {
        return Ok(None);
    };
    if !actor(&mut evidence.admitted_by, subjects) {
        return Ok(None);
    }
    record.revision = record
        .revision
        .checked_add(1)
        .ok_or(Error::InvalidRecord(NAME))?;
    let mut replacement = row.clone();
    replacement.extend(records::policy_document(&record).map_err(|_| Error::InvalidRecord(NAME))?);
    Ok(Some(replacement))
}

fn binding(row: &Document, subjects: &BTreeSet<String>) -> Result<Option<Document>, Error> {
    const NAME: &str = "provider_bindings";
    if !has_subject(row, &["record.manager"], subjects) {
        return Ok(None);
    }
    let mut record: ProviderBinding =
        crate::store::sandbox_mongo::read_record(row).map_err(|_| Error::InvalidRecord(NAME))?;
    record.validate().map_err(|_| Error::InvalidRecord(NAME))?;
    if row.get_str("_id").ok() != Some(record.binding_id.as_str())
        || row.get_i64("revision").ok() != i64::try_from(record.revision).ok()
        || row.get_i64("generation").ok() != i64::try_from(record.generation).ok()
        || row.get_bool("pending").ok()
            != Some(layer2_core::sandbox::binding::store::binding_has_work(
                &record,
            ))
    {
        return Err(Error::InvalidRecord(NAME));
    }
    record.manager = SubjectId::new(layer2_core::DELETED_ACCOUNT_SUBJECT);
    record.revision = record
        .revision
        .checked_add(1)
        .ok_or(Error::InvalidRecord(NAME))?;
    record.validate().map_err(|_| Error::InvalidRecord(NAME))?;
    let mut replacement = encoded(row, &record, NAME)?;
    replacement.insert(
        "revision",
        i64::try_from(record.revision).map_err(|_| Error::InvalidRecord(NAME))?,
    );
    Ok(Some(replacement))
}

fn provenance(row: &Document, subjects: &BTreeSet<String>) -> Result<Option<Document>, Error> {
    const NAME: &str = "agent_creation_provenance";
    if !has_subject(
        row,
        &[
            "record.created_by.subject",
            "record.creation_origin.launching_actor.subject",
        ],
        subjects,
    ) {
        return Ok(None);
    }
    let mut record =
        crate::store::agent_provenance::read(row).map_err(|_| Error::InvalidRecord(NAME))?;
    if !creation(&mut record, subjects) {
        return Ok(None);
    }
    encoded(row, &record, NAME).map(Some)
}

fn command(row: &Document, subjects: &BTreeSet<String>) -> Result<Option<Document>, Error> {
    const NAME: &str = "agent_commands";
    if !has_subject(row, &["record.receipt.envelope.actor.subject"], subjects) {
        return Ok(None);
    }
    let mut record =
        crate::store::agent_commands::records::read(row).map_err(|_| Error::InvalidRecord(NAME))?;
    if !actor(&mut record.receipt.envelope.actor, subjects) {
        return Ok(None);
    }
    record.revision = record
        .revision
        .checked_add(1)
        .ok_or(Error::InvalidRecord(NAME))?;
    let mut replacement = row.clone();
    replacement.extend(
        crate::store::agent_commands::records::document(&record)
            .map_err(|_| Error::InvalidRecord(NAME))?,
    );
    Ok(Some(replacement))
}

fn audit(row: &Document, subjects: &BTreeSet<String>) -> Result<Option<Document>, Error> {
    const NAME: &str = "command_authorization_audits";
    if !has_subject(
        row,
        &[
            "record.actor.subject",
            "record.resources.actor.subject",
            "record.resources.observations.basis.subject",
        ],
        subjects,
    ) {
        return Ok(None);
    }
    let mut record: CommandAuthorizationAudit =
        crate::store::sandbox_mongo::read_record(row).map_err(|_| Error::InvalidRecord(NAME))?;
    record.validate().map_err(|_| Error::InvalidRecord(NAME))?;
    let mut changed = actor(&mut record.actor, subjects);
    for resource in &mut record.resources {
        changed |= resource_audit(resource, subjects);
    }
    if !changed {
        return Ok(None);
    }
    record.validate().map_err(|_| Error::InvalidRecord(NAME))?;
    encoded(row, &record, NAME).map(Some)
}

fn resource_audit(record: &mut ResourceAuthorizationAudit, subjects: &BTreeSet<String>) -> bool {
    let mut changed = actor(&mut record.actor, subjects);
    for observation in &mut record.observations {
        if let ResourceAuthorizationBasis::ShareRecipient { subject, .. } = &mut observation.basis {
            if subjects.contains(subject.as_str()) {
                *subject = SubjectId::new(layer2_core::DELETED_ACCOUNT_SUBJECT);
                changed = true;
            }
        }
    }
    changed
}

fn redact_actor_field(
    record: &mut Document,
    field: &str,
    subjects: &BTreeSet<String>,
    collection: &'static str,
) -> Result<bool, Error> {
    let Some(value) = record.get(field) else {
        return Ok(false);
    };
    let mut value: ResourceActor =
        bson::deserialize_from_bson(value.clone()).map_err(|_| Error::InvalidRecord(collection))?;
    if !actor(&mut value, subjects) {
        return Ok(false);
    }
    record.insert(
        field,
        bson::serialize_to_bson(&value).map_err(|_| Error::InvalidRecord(collection))?,
    );
    Ok(true)
}

async fn lifecycle(
    db: &Database,
    row: &Document,
    subjects: &BTreeSet<String>,
) -> Result<Option<Document>, Error> {
    const NAME: &str = "agent_lifecycle_operations";
    if !has_subject(row, &["input.actor.subject", "owner"], subjects) {
        return Ok(None);
    }
    let mut replacement = row.clone();
    let mut changed = match replacement.get_document_mut("input") {
        Ok(input) if matches!(input.get("actor"), None | Some(Bson::Null)) => false,
        Ok(input) => redact_actor_field(input, "actor", subjects, NAME)?,
        Err(_) => false,
    };
    if row
        .get_str("owner")
        .ok()
        .is_some_and(|owner| subjects.contains(owner))
    {
        if let Ok(id) = row.get_str("sandbox_id") {
            if majority_collection::<Document>(db, "resource_policies")
                .find_one(doc! {"_id":id,"owner_kind":"organization"})
                .await?
                .is_some()
            {
                replacement.insert("owner", layer2_core::DELETED_ACCOUNT_SUBJECT);
                changed = true;
            }
        }
    }
    Ok(changed.then_some(replacement))
}
