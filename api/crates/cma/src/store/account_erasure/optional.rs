//! Fresh namespace and command authority receipts, never persisted capabilities.
use super::{CohortBlock, Error};
use crate::store::{
    agent_observation, allocation_buckets, creation_outcomes, durability::majority_collection,
};
use mongodb::{
    bson::{doc, Document},
    error::ErrorKind,
    Database,
};

pub(super) struct Receipt {
    pub allocation_absent: bool,
    pub observer_absent: bool,
    pub creation_proofs_absent: bool,
    pub active_log_proofs_absent: bool,
}
impl Receipt {
    pub fn skips(&self, collection: &str) -> bool {
        (self.allocation_absent && collection == allocation_buckets::COLLECTION)
            || (self.observer_absent && collection == agent_observation::COLLECTION)
            || (self.creation_proofs_absent && collection == creation_outcomes::COLLECTION)
            || (self.active_log_proofs_absent
                && collection == crate::store::agent_log_segments::active_proof::COLLECTION)
    }
}

pub(super) fn failure(
    error: mongodb::error::Error,
    collection: &'static str,
    operation: &'static str,
) -> Error {
    if matches!(error.kind.as_ref(), ErrorKind::Command(command) if command.code == 13) {
        return Error::cohort(CohortBlock::CollectionAuthority {
            collection,
            operation,
        });
    }
    error.into()
}

pub(super) async fn inspect(
    db: &Database,
    allocation_rollups_enabled: Option<bool>,
    agent_shared_observation: Option<bool>,
) -> Result<Receipt, Error> {
    Ok(Receipt {
        allocation_absent: absent(
            db,
            allocation_buckets::COLLECTION,
            allocation_rollups_enabled,
        )
        .await?,
        observer_absent: absent(db, agent_observation::COLLECTION, agent_shared_observation)
            .await?,
        // Disabling the reader can leave proofs that still belong to this owner.
        creation_proofs_absent: absent(db, creation_outcomes::COLLECTION, Some(false)).await?,
        active_log_proofs_absent: absent(
            db,
            crate::store::agent_log_segments::active_proof::COLLECTION,
            Some(false),
        )
        .await?,
    })
}

pub(super) async fn absent(
    db: &Database,
    name: &'static str,
    enabled: Option<bool>,
) -> Result<bool, Error> {
    // Explicitly require complete database metadata authority. A filtered
    // authorizedCollections=true response can never prove namespace absence.
    let names = db
        .list_collection_names()
        .filter(doc! {"name":name})
        .authorized_collections(false)
        .await
        .map_err(|error| failure(error, name, "listCollections"))?;
    if enabled == Some(false) && !names.iter().any(|found| found == name) {
        return Ok(true);
    }
    // Exercise the actual credential, including inherited/shared grants. Both
    // selectors are impossible, so permission probes cannot read or erase a
    // tenant row, or create an absent namespace. Never turn a denial into zero.
    let collection = majority_collection::<Document>(db, name);
    collection
        .find_one(doc! {"_id":{"$in":[]}})
        .projection(doc! {"_id":1})
        .await
        .map_err(|error| failure(error, name, "find"))?;
    collection
        .delete_many(doc! {"_id":{"$in":[]}})
        .await
        .map_err(|error| failure(error, name, "remove"))?;
    Ok(false)
}
