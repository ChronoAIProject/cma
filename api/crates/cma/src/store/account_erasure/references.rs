//! Revision-fenced redaction of references held by other accounts.
use super::Error;
use crate::store::{durability::majority_collection, resource_access::records};
use layer2_core::{
    document_artifacts::{DocumentError, DocumentStore},
    resource_access::{AgentShareAudience, ResourceShare},
    SandboxId,
};
use mongodb::{
    bson::{self, doc, Bson, Document},
    Database,
};
use std::collections::BTreeSet;
#[path = "reference_gates.rs"]
mod gates;
#[path = "share_policy.rs"]
mod share_policy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Redaction {
    Unchanged,
    Deleted,
    Updated,
    Conflict,
    Waiting,
    DrainRequired,
}

#[cfg(test)]
pub(super) async fn redact(
    db: &Database,
    collection: &str,
    row: &Document,
    subjects: &BTreeSet<String>,
) -> Result<Redaction, Error> {
    redact_with_agents(
        db,
        collection,
        row,
        subjects,
        &BTreeSet::new(),
        &layer2_core::now_rfc3339(),
    )
    .await
}

pub(super) async fn redact_with_agents(
    db: &Database,
    collection: &str,
    row: &Document,
    subjects: &BTreeSet<String>,
    agents: &BTreeSet<String>,
    now: &str,
) -> Result<Redaction, Error> {
    let identity = super::identity_refs::redact(db, collection, row, subjects).await?;
    if matches!(identity, Redaction::Conflict | Redaction::Waiting) {
        return Ok(identity);
    }
    let fresh = if identity == Redaction::Updated {
        majority_collection::<Document>(db, collection).find_one(doc! {"_id":row.get("_id").cloned().ok_or(Error::InvalidRecord(collection_name(collection)))?}).await?
    } else {
        None
    };
    if identity == Redaction::Updated && fresh.is_none() {
        return Ok(identity);
    }
    let outcome = redact_content(
        db,
        collection,
        fresh.as_ref().unwrap_or(row),
        subjects,
        agents,
        now,
    )
    .await?;
    Ok(if outcome == Redaction::Unchanged {
        identity
    } else {
        outcome
    })
}
fn collection_name(collection: &str) -> &'static str {
    super::catalog::ERASURE
        .iter()
        .find(|entry| entry.collection == collection)
        .map_or("reference collection", |entry| entry.collection)
}
pub(super) async fn redact_content(
    db: &Database,
    collection: &str,
    row: &Document,
    subjects: &BTreeSet<String>,
    agents: &BTreeSet<String>,
    now: &str,
) -> Result<Redaction, Error> {
    match collection {
        "agent_log_gates" => gates::redact(db, row, subjects, agents, now).await,
        "cma_document_artifacts" if document_mentions(row, subjects) => {
            document(db, row, subjects).await
        }
        "resource_shares"
            if share_mentions(row.get_document("record").ok(), subjects)
                || array_mentions(row.get_array("accepted_subjects").ok(), subjects) =>
        {
            share(db, row, subjects, now).await
        }
        "resource_share_commands"
            if share_mentions(
                row.get_document("record")
                    .ok()
                    .and_then(|r| r.get_document("share").ok()),
                subjects,
            ) =>
        {
            receipt(db, row, subjects).await
        }
        "trigger_definition_revisions"
            if row
                .get_str("owner")
                .ok()
                .is_some_and(|s| subjects.contains(s)) =>
        {
            trigger_revision(db, row).await
        }
        "platform_settings" if settings_mentions(row, subjects) => {
            settings(db, row, subjects).await
        }
        _ => Ok(Redaction::Unchanged),
    }
}

fn array_mentions(values: Option<&Vec<Bson>>, subjects: &BTreeSet<String>) -> bool {
    values.is_some_and(|values| {
        values
            .iter()
            .any(|v| v.as_str().is_some_and(|s| subjects.contains(s)))
    })
}
fn field_mentions(row: Option<&Document>, field: &str, subjects: &BTreeSet<String>) -> bool {
    row.and_then(|r| r.get_str(field).ok())
        .is_some_and(|s| subjects.contains(s))
}
fn share_mentions(row: Option<&Document>, subjects: &BTreeSet<String>) -> bool {
    let link = row.and_then(|r| r.get_document("link").ok());
    array_mentions(
        link.and_then(|r| r.get_array("accepted_subjects").ok()),
        subjects,
    ) || array_mentions(
        link.and_then(|r| r.get_document("audience").ok())
            .and_then(|r| r.get_array("subjects").ok()),
        subjects,
    )
}
fn document_mentions(row: &Document, subjects: &BTreeSet<String>) -> bool {
    if array_mentions(row.get_array("recipient_subjects").ok(), subjects) {
        return true;
    }
    let Some(record) = row
        .get_document("record")
        .or_else(|_| row.get_document("audit"))
        .ok()
    else {
        return false;
    };
    for (field, subject_field) in [("shares", "subject"), ("comments", "author_subject")] {
        if record.get_array(field).ok().is_some_and(|values| {
            values
                .iter()
                .any(|v| field_mentions(v.as_document(), subject_field, subjects))
        }) {
            return true;
        }
    }
    record
        .get_document("receipts")
        .ok()
        .is_some_and(|receipts| {
            receipts.values().any(|value| {
                field_mentions(
                    value
                        .as_document()
                        .and_then(|r| r.get_document("outcome").ok())
                        .and_then(|r| r.get_document("comment").ok()),
                    "author_subject",
                    subjects,
                )
            })
        })
}

async fn document(
    db: &Database,
    row: &Document,
    subjects: &BTreeSet<String>,
) -> Result<Redaction, Error> {
    let deleted = crate::document_artifacts::deletion::decode_audit(row)
        .map_err(|_| Error::InvalidRecord("cma_document_artifacts"))?;
    let (tombstone, mut record) = match deleted {
        Some((tombstone, audit)) => (Some(tombstone), audit),
        None => (
            None,
            crate::document_artifacts::decode_record(row)
                .map_err(|_| Error::InvalidRecord("cma_document_artifacts"))?,
        ),
    };
    let expected = record.metadata.revision;
    record.shares.retain(|s| !subjects.contains(&s.subject));
    record
        .comments
        .retain(|c| !subjects.contains(&c.author_subject));
    for receipt in record.receipts.values_mut() {
        if let Some(comment) = receipt
            .outcome
            .comment
            .as_mut()
            .filter(|c| subjects.contains(&c.author_subject))
        {
            comment.author_subject = layer2_core::DELETED_ACCOUNT_SUBJECT.into();
            comment.body.clear();
        }
    }
    if let Some(tombstone) = tombstone {
        let replacement = crate::document_artifacts::deletion::encode(&tombstone, &record)
            .map_err(|_| Error::InvalidRecord("cma_document_artifacts"))?;
        let result = majority_collection::<Document>(db, "cma_document_artifacts")
            .replace_one(doc! {"_id": &row["_id"], "document_schema_version": &row["document_schema_version"], "deletion": &row["deletion"], "audit": &row["audit"]}, replacement)
            .await?;
        return Ok(outcome(result.matched_count == 1));
    }
    record.metadata.revision = expected
        .checked_add(1)
        .ok_or(Error::InvalidRecord("cma_document_artifacts"))?;
    let store = crate::document_artifacts::MongoDocumentStore::new(db.clone());
    match store.compare_exchange(expected, &record).await {
        Ok(changed) => Ok(outcome(changed)),
        Err(DocumentError::Unavailable) => {
            Err(Error::Store("document redaction unavailable".into()))
        }
        Err(_) => Err(Error::InvalidRecord("cma_document_artifacts")),
    }
}

pub(super) fn remove_subjects(
    share: &mut ResourceShare,
    subjects: &BTreeSet<String>,
    at: &str,
) -> bool {
    let Some(link) = &mut share.link else {
        return false;
    };
    let old = link.accepted_subjects.len();
    link.accepted_subjects
        .retain(|s| !subjects.contains(s.as_str()));
    let mut changed = old != link.accepted_subjects.len();
    if let AgentShareAudience::Users { subjects: audience } = &mut link.audience {
        let old = audience.len();
        audience.retain(|s| !subjects.contains(s.as_str()));
        changed |= old != audience.len();
        if changed && audience.is_empty() && share.revoked_at.is_none() {
            // An empty explicit audience is only a readable, revoked tombstone.
            let effective_at = if matches!(
                (layer2_core::seconds_from_rfc3339(at), layer2_core::seconds_from_rfc3339(&share.created_at)),
                (Some(at), Some(created)) if at >= created
            ) {
                at
            } else {
                &share.created_at
            };
            share.revoked_at = Some(effective_at.into());
        }
    }
    changed
}
async fn share(
    db: &Database,
    row: &Document,
    subjects: &BTreeSet<String>,
    now: &str,
) -> Result<Redaction, Error> {
    let sandbox = SandboxId(
        row.get_str("sandbox_id")
            .map_err(|_| Error::InvalidRecord("resource_shares"))?
            .into(),
    );
    let mut share =
        records::read_share(&sandbox, row).map_err(|_| Error::InvalidRecord("resource_shares"))?;
    if !remove_subjects(&mut share, subjects, now) {
        return Ok(Redaction::Unchanged);
    }
    let revision = share.revision;
    share.revision = revision
        .checked_add(1)
        .ok_or(Error::InvalidRecord("resource_shares"))?;
    let encoded = records::share_document(&sandbox, &share)
        .map_err(|_| Error::InvalidRecord("resource_shares"))?;
    let mut previous = share.clone();
    previous.revision = revision;
    if !share_policy::reserve(db, &sandbox, &previous).await? {
        return Ok(Redaction::Conflict);
    }
    let mut replacement = row.clone();
    replacement.extend(encoded);
    let result = majority_collection::<Document>(db, "resource_shares")
        .replace_one(
            doc! {"_id":&share.id,"$expr":{"$eq":["$$ROOT",{"$literal":row.clone()}]}},
            replacement,
        )
        .await?;
    Ok(outcome(result.matched_count == 1))
}
async fn receipt(
    db: &Database,
    row: &Document,
    subjects: &BTreeSet<String>,
) -> Result<Redaction, Error> {
    let mut receipt =
        records::read_receipt(row).map_err(|_| Error::InvalidRecord("resource_share_commands"))?;
    if !remove_subjects(&mut receipt.share, subjects, &receipt.at.clone()) {
        return Ok(Redaction::Unchanged);
    }
    let encoded = records::receipt_document(&receipt)
        .map_err(|_| Error::InvalidRecord("resource_share_commands"))?;
    let mut replacement = row.clone();
    replacement.extend(encoded);
    let (filter, next) = immutable_cas(row, "resource_share_commands")?;
    replacement.insert("mutation_version", next);
    let result = majority_collection::<Document>(db, "resource_share_commands")
        .replace_one(filter, replacement)
        .await?;
    Ok(outcome(result.matched_count == 1))
}
async fn trigger_revision(db: &Database, row: &Document) -> Result<Redaction, Error> {
    let stored = crate::store::trigger_definitions::documents::read_revision(row)
        .map_err(|_| Error::InvalidRecord("trigger_definition_revisions"))?;
    let mut revision = stored.revision;
    revision.erase_publisher();
    revision
        .validate()
        .map_err(|_| Error::InvalidRecord("trigger_definition_revisions"))?;
    let (filter, next) = immutable_cas(row, "trigger_definition_revisions")?;
    let mut replacement = row.clone();
    replacement.insert(
        "record",
        bson::serialize_to_bson(&revision)
            .map_err(|_| Error::InvalidRecord("trigger_definition_revisions"))?,
    );
    replacement.insert("owner", layer2_core::DELETED_ACCOUNT_SUBJECT);
    for field in [
        "sandbox_profile_id",
        "workspace_profile_id",
        "agent_profile_id",
    ] {
        replacement.insert(field, "");
    }
    replacement.insert("mutation_version", next);
    let result = majority_collection::<Document>(db, "trigger_definition_revisions")
        .replace_one(filter, replacement)
        .await?;
    Ok(outcome(result.matched_count == 1))
}
fn immutable_cas(row: &Document, collection: &'static str) -> Result<(Document, i64), Error> {
    let mut filter = row.clone();
    let current = if row.contains_key("mutation_version") {
        row.get_i64("mutation_version")
            .ok()
            .filter(|n| *n > 0)
            .ok_or(Error::InvalidRecord(collection))?
    } else {
        filter.insert("mutation_version", doc! {"$exists":false});
        0
    };
    if let Some(record) = row.get("record") {
        filter.insert("record", doc! {"$eq":record.clone()});
    }
    Ok((
        filter,
        current
            .checked_add(1)
            .ok_or(Error::InvalidRecord(collection))?,
    ))
}
pub(super) fn settings_mentions(row: &Document, subjects: &BTreeSet<String>) -> bool {
    row.get_document("settings")
        .ok()
        .and_then(|settings| settings.get_document("model_budget").ok())
        .and_then(|budget| budget.get_array("owners").ok())
        .is_some_and(|owners| {
            owners
                .iter()
                .any(|row| field_mentions(row.as_document(), "owner", subjects))
        })
        || row
            .get_document("setting_changes")
            .ok()
            .and_then(|metadata| metadata.get_document("entries").ok())
            .is_some_and(|entries| {
                entries
                    .values()
                    .any(|row| field_mentions(row.as_document(), "actor", subjects))
            })
}

async fn settings(
    db: &Database,
    row: &Document,
    subjects: &BTreeSet<String>,
) -> Result<Redaction, Error> {
    const NAME: &str = "platform_settings";
    let mut settings: layer2_core::PlatformSettings = bson::deserialize_from_document(
        row.get_document("settings")
            .map_err(|_| Error::InvalidRecord(NAME))?
            .clone(),
    )
    .map_err(|_| Error::InvalidRecord(NAME))?;
    settings
        .validate()
        .map_err(|_| Error::InvalidRecord(NAME))?;
    let owner_count = settings.model_budget.owners.len();
    let was_enabled = settings.model_budget.enabled;
    settings
        .model_budget
        .owners
        .retain(|policy| !subjects.contains(policy.owner.as_str()));
    let owners_changed = settings.model_budget.owners.len() != owner_count;
    if owners_changed
        && settings.model_budget.default_policy.is_none()
        && settings.model_budget.owners.is_empty()
    {
        settings.model_budget.enabled = false;
    }
    settings
        .validate()
        .map_err(|_| Error::InvalidRecord(NAME))?;
    let current = row
        .get_i64("revision")
        .ok()
        .filter(|n| *n > 0)
        .ok_or(Error::InvalidRecord(NAME))?;
    if row.get_str("_id").ok() != Some("platform")
        || row.get_i64("document_schema_version").ok() != Some(1)
    {
        return Err(Error::InvalidRecord(NAME));
    }
    let mut replacement = row.clone();
    let next = current.checked_add(1).ok_or(Error::InvalidRecord(NAME))?;
    replacement.insert("revision", next);
    if let Ok(metadata) = replacement.get_document_mut("setting_changes") {
        if let Ok(entries) = metadata.get_document_mut("entries") {
            let erased: Vec<_> = entries
                .iter()
                .filter_map(|(key, value)| {
                    (field_mentions(value.as_document(), "actor", subjects)
                        || (key == "owner_overrides" && owners_changed)
                        || (key == "spend_controls"
                            && was_enabled != settings.model_budget.enabled))
                        .then(|| key.clone())
                })
                .collect();
            for key in erased {
                entries.remove(key);
            }
        }
        if metadata.get_i64("coverage_revision").ok() == Some(current) {
            metadata.insert("coverage_revision", next);
        }
    }
    replacement.insert(
        "settings",
        bson::serialize_to_bson(&settings).map_err(|_| Error::InvalidRecord(NAME))?,
    );
    let result = majority_collection::<Document>(db, NAME)
        .replace_one(
            doc! {"_id":"platform","document_schema_version":1_i64,"revision":current},
            replacement,
        )
        .await?;
    Ok(outcome(result.matched_count == 1))
}
fn outcome(changed: bool) -> Redaction {
    if changed {
        Redaction::Updated
    } else {
        Redaction::Conflict
    }
}
