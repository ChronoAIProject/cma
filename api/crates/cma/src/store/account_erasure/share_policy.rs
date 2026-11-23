//! Reserve a readable policy fence before a foreign personal Share changes.
use super::Error;
use crate::store::{durability::majority_collection, resource_access::records};
use layer2_core::{resource_access::ResourceShare, SandboxId};
use mongodb::{
    bson::{doc, Document},
    Database,
};

/// This is a concurrency fence, not a content redaction. Policy-first ordering
/// keeps policy.revision >= share.revision in every visible intermediate state.
/// A failed later Share CAS may leave an extra policy increment, just as an
/// admission fence can be touched without creating a new resource.
pub(super) async fn reserve(
    db: &Database,
    sandbox_id: &SandboxId,
    share: &ResourceShare,
) -> Result<bool, Error> {
    const NAME: &str = "resource_policies";
    let policies = majority_collection::<Document>(db, NAME);
    let stored = policies.find_one(doc! {"_id":sandbox_id.as_str()}).await?;
    let sandbox = majority_collection::<Document>(db, "sandboxes")
        .find_one(doc! {"_id":sandbox_id.as_str()})
        .await?;
    let Some(row) = stored else {
        // Old standalone Share evidence may outlive its root and sidecar. It
        // cannot establish an accessible resource scope without that root.
        return if sandbox.is_none() {
            Ok(true)
        } else {
            Err(Error::InvalidRecord(NAME))
        };
    };
    let mut policy = records::read_policy(&row).map_err(|_| Error::InvalidRecord(NAME))?;
    if policy.sandbox_id != *sandbox_id
        || policy.owner != share.owner
        || policy.revision < share.revision
    {
        return Err(Error::InvalidRecord(NAME));
    }
    if let Some(row) = sandbox {
        let sandbox = crate::store::sandboxes::read_sandbox(&row)
            .map_err(|_| Error::InvalidRecord("sandboxes"))?;
        if !policy
            .personal_owner_of(&sandbox)
            .map_err(|_| Error::InvalidRecord(NAME))?
        {
            return Err(Error::InvalidRecord(NAME));
        }
    }
    policy.revision = policy
        .revision
        .checked_add(1)
        .filter(|revision| *revision <= i64::MAX as u64)
        .ok_or(Error::InvalidRecord(NAME))?;
    let mut replacement = row.clone();
    replacement.extend(records::policy_document(&policy).map_err(|_| Error::InvalidRecord(NAME))?);
    let result = policies
        .replace_one(
            doc! {"_id":sandbox_id.as_str(),"$expr":{"$eq":["$$ROOT",{"$literal":row}]}},
            replacement,
        )
        .await?;
    Ok(result.matched_count == 1)
}
