//! Preserve instance scheduling policy after an explicitly authorized catalogue cleanup.
use super::transaction::ApplyError;
use crate::store::{durability::majority_collection, sandbox_mongo::MongoSandboxStore};
use layer2_core::sandbox::{store::SandboxStoreError, Sandbox};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

pub(super) async fn keep_alive(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    let rows = majority_collection::<Document>(
        &store.client.database(&store.sandboxes.namespace().db),
        "sandbox_retained_availability",
    );
    let Some(row) = rows
        .find_one(doc! {"_id":sandbox.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
    else {
        return Ok(false);
    };
    let corrupt = || {
        ApplyError::Domain(SandboxStoreError::Corrupt(
            "retained Sandbox availability identity is invalid".into(),
        ))
    };
    crate::store::sandbox_mongo::require_version(&row).map_err(|_| corrupt())?;
    if row.get_str("owner").ok() != Some(sandbox.owner.as_str())
        || row.get_str("sandbox_created_at").ok() != Some(sandbox.created_at.as_str())
        || row.get_str("profile_id").ok() != Some(sandbox.profile.id.as_str())
    {
        return Err(corrupt());
    }
    let keep = row.get_bool("always_keep_alive").map_err(|_| corrupt())?;
    if keep {
        return Ok(true);
    }
    let changed = rows
        .update_one(row, doc! {"$inc":{"inactivity_fence":1_i64}})
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if changed.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(false)
}
