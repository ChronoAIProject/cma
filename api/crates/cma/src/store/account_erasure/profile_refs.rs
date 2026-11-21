//! Retire retained immutable pins and physically remove unused authoring data.
use super::{references::Redaction, Error};
use crate::store::durability::majority_collection;
use mongodb::{
    bson::{doc, Bson, Document},
    Database,
};
use std::collections::BTreeSet;
mod discovery;
pub(super) use discovery::State as Discovery;
const AGGREGATES: [&str; 3] = ["sandbox_profiles", "workspace_profiles", "agent_profiles"];
const REVISIONS: [&str; 3] = [
    "sandbox_profile_revisions",
    "workspace_profile_revisions",
    "agent_profile_revisions",
];

#[cfg(test)]
pub(super) async fn retained(
    db: &Database,
    subjects: &BTreeSet<String>,
) -> Result<BTreeSet<String>, Error> {
    super::indexes::ensure(db).await?;
    let mut state = Discovery::default();
    loop {
        if let Some(retained) = discovery::page(db, subjects, &BTreeSet::new(), &mut state).await? {
            return Ok(retained.pins);
        }
    }
}
pub(super) async fn page(
    db: &Database,
    contexts: &[super::context::Context],
    cursor: &mut super::Cursor,
) -> Result<bool, Error> {
    let before = super::progress::contributions(cursor, contexts)?;
    let organizations = contexts
        .iter()
        .flat_map(|context| context.organization_sandboxes.iter().cloned())
        .collect();
    let state = cursor
        .profile_discovery
        .get_or_insert_with(Discovery::default);
    let retained = discovery::page(db, &cursor.subjects, &organizations, state).await?;
    let complete = if let Some(retained) = retained {
        cursor.retained_profiles = Some(retained.pins);
        cursor.retained_profile_owners = retained.owners;
        cursor.profile_discovery = None;
        true
    } else {
        false
    };
    // Shared pages can contain several authors. Charge the actual largest
    // serialized growth, including graph-to-retained transitions, rather than
    // whichever small author happened to appear at the end of the page.
    cursor.capacity_subject = super::progress::contributions(cursor, contexts)?
        .into_iter()
        .filter_map(|(subject, bytes)| {
            let growth = bytes.saturating_sub(before.get(&subject).copied().unwrap_or(0));
            (growth > 0).then_some((subject, growth))
        })
        .max_by_key(|(_, growth)| *growth)
        .map(|(subject, _)| subject);
    Ok(complete)
}
pub(super) async fn apply(
    db: &Database,
    collection: &str,
    row: &Document,
    subjects: &BTreeSet<String>,
    retained: &mut BTreeSet<String>,
    now: &str,
) -> Result<Redaction, Error> {
    let revision = REVISIONS.iter().position(|name| *name == collection);
    let aggregate = revision.map(|i| AGGREGATES[i]).unwrap_or(collection);
    let id = row
        .get_str(if revision.is_some() {
            "profile_id"
        } else {
            "_id"
        })
        .map_err(|_| Error::InvalidRecord("Profile identity"))?;
    let profiles = majority_collection::<Document>(db, aggregate);
    let Some(parent) = profiles.find_one(doc! {"_id":id}).await? else {
        return Ok(Redaction::Unchanged);
    };
    if !parent
        .get_str("owner")
        .ok()
        .is_some_and(|owner| subjects.contains(owner))
    {
        return Ok(Redaction::Unchanged);
    }
    if parent.get_bool("account_erasure_fenced").ok() != Some(true)
        || parent.get_datetime("account_erasure_fenced_at").is_err()
    {
        return Ok(Redaction::DrainRequired);
    }
    let keep = if revision.is_some() {
        row.get_str("_id").is_ok_and(|key| retained.contains(key))
    } else {
        retained.contains(id)
    };
    if keep {
        if revision.is_some() {
            return Ok(Redaction::Unchanged);
        }
        let next = parent
            .get_i64("draft_version")
            .ok()
            .and_then(|n| n.checked_add(1))
            .ok_or(Error::InvalidRecord("Profile draft version"))?;
        let mut update = doc! {"$set":{
            "owner":layer2_core::DELETED_ACCOUNT_SUBJECT,"record.owner":layer2_core::DELETED_ACCOUNT_SUBJECT,
            "visibility":"private","record.visibility":"private","retired_at":now,"record.retired_at":now,
            "name":"","record.name":"","description":"","record.description":"",
            "draft_version":next,"record.draft_version":next,"account_erasure_retained":true,
        },"$unset":{"record.draft":"","record.forked_from":""}};
        if aggregate == "sandbox_profiles" {
            update
                .get_document_mut("$set")
                .unwrap()
                .insert("record.availability.warmup_enabled", false);
        }
        let changed = profiles.update_one(exact_parent(&parent), update).await?;
        return Ok(if changed.modified_count == 1 {
            Redaction::Updated
        } else {
            Redaction::Conflict
        });
    }
    let records = majority_collection::<Document>(db, collection);
    let filter = if revision.is_some() {
        doc! {"_id":row.get("_id").cloned().ok_or(Error::InvalidRecord("Profile revision identity"))?,
        "record":row.get("record").cloned().ok_or(Error::InvalidRecord("Profile revision content"))?}
    } else {
        exact_parent(&parent)
    };
    Ok(if records.delete_one(filter).await?.deleted_count == 1 {
        Redaction::Deleted
    } else {
        Redaction::Conflict
    })
}
fn exact_parent(row: &Document) -> Document {
    let mut filter = doc! {};
    for field in [
        "_id",
        "owner",
        "draft_version",
        "last_revision",
        "trigger_publication_fence",
        "account_erasure_fenced",
        "account_erasure_fenced_at",
    ] {
        filter.insert(field, row.get(field).cloned().unwrap_or(Bson::Null));
    }
    filter
}
