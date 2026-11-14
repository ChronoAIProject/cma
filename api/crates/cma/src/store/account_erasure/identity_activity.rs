//! Explicit subject details can be erased without claiming their resource target.
use super::Error;
use crate::store::durability::majority_collection;
use layer2_core::{resource_access::ResourceOwner, ActivityRecord, DetailKey, SubjectId};
use mongodb::{
    bson::{self, doc, Document},
    Database,
};
use std::collections::BTreeSet;

pub(super) fn sandbox_target(row: &Document) -> Option<&str> {
    let target = row
        .get_document("record")
        .ok()?
        .get_document("target")
        .ok()?;
    if target.get_str("kind").ok()? != "resource" {
        return None;
    }
    let resource = target.get_str("id").ok()?.strip_prefix("sandbox:")?;
    let id = resource.strip_suffix(":events").unwrap_or(resource);
    (!id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
    .then_some(id)
}

pub(super) async fn replacement(
    db: &Database,
    row: &Document,
    subjects: &BTreeSet<String>,
) -> Result<Option<Document>, Error> {
    const NAME: &str = "activity";
    if !super::identity_refs::mentions_activity(row, subjects) {
        return Ok(None);
    }
    let mut record: ActivityRecord =
        crate::store::sandbox_mongo::read_record(row).map_err(|_| Error::InvalidRecord(NAME))?;
    if row.get_str("_id").ok() != Some(record.id.0.as_str()) {
        return Err(Error::InvalidRecord(NAME));
    }
    let erased_actor = record
        .actor
        .as_ref()
        .is_some_and(|subject| subjects.contains(subject.as_str()));
    let erased_detail = record
        .detail
        .get(DetailKey::Subject)
        .is_some_and(|subject| subjects.contains(subject));
    // Another actor's explicit subject detail is personal attribution, even if
    // the resource has already disappeared. This never establishes ownership.
    if erased_actor || !erased_detail {
        let Some(id) = sandbox_target(row) else {
            return Ok(None);
        };
        if !organization_target(db, id).await? {
            return Ok(None);
        }
    }
    if erased_actor {
        record.actor = Some(SubjectId::new(layer2_core::DELETED_ACCOUNT_SUBJECT));
    }
    if erased_detail {
        record
            .detail
            .record(DetailKey::Subject, layer2_core::DELETED_ACCOUNT_SUBJECT);
    }
    let mut replacement = row.clone();
    replacement.insert(
        "record",
        bson::serialize_to_bson(&record).map_err(|_| Error::InvalidRecord(NAME))?,
    );
    Ok(Some(replacement))
}

async fn organization_target(db: &Database, id: &str) -> Result<bool, Error> {
    let Some(policy) = majority_collection::<Document>(db, "resource_policies")
        .find_one(doc! {"_id":id,"owner_kind":"organization"})
        .await?
    else {
        return Ok(false);
    };
    let policy = crate::store::resource_access::records::read_policy(&policy)
        .map_err(|_| Error::InvalidRecord("resource_policies"))?;
    if !matches!(policy.owner, ResourceOwner::Organization { .. }) {
        return Ok(false);
    }
    if let Some(sandbox) = majority_collection::<Document>(db, "sandboxes")
        .find_one(doc! {"_id":id})
        .await?
    {
        let sandbox = crate::store::sandboxes::read_sandbox(&sandbox)
            .map_err(|_| Error::InvalidRecord("sandboxes"))?;
        if policy
            .personal_owner_of(&sandbox)
            .map_err(|_| Error::InvalidRecord("resource_policies"))?
        {
            return Ok(false);
        }
    }
    Ok(true)
}
