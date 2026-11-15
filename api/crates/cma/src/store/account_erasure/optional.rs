//! Fresh namespace and command authority receipts, never persisted capabilities.
use super::{CohortBlock, Error};
use crate::store::{allocation_buckets::COLLECTION, durability::majority_collection};
use mongodb::{
    bson::{doc, Document},
    error::ErrorKind,
    Database,
};

pub(super) struct Receipt {
    pub allocation_absent: bool,
}
impl Receipt {
    pub fn skips(&self, collection: &str) -> bool {
        self.allocation_absent && collection == COLLECTION
    }
}

pub(super) fn failure(error: mongodb::error::Error, operation: &'static str) -> Error {
    if matches!(error.kind.as_ref(), ErrorKind::Command(command) if command.code == 13) {
        return Error::cohort(CohortBlock::CollectionAuthority {
            collection: COLLECTION,
            operation,
        });
    }
    error.into()
}

pub(super) async fn inspect(
    db: &Database,
    allocation_rollups_enabled: Option<bool>,
) -> Result<Receipt, Error> {
    // Explicitly require complete database metadata authority. A filtered
    // authorizedCollections=true response can never prove namespace absence.
    let names = db
        .list_collection_names()
        .filter(doc! {"name":COLLECTION})
        .authorized_collections(false)
        .await
        .map_err(|error| failure(error, "listCollections"))?;
    if allocation_rollups_enabled == Some(false) && !names.iter().any(|name| name == COLLECTION) {
        return Ok(Receipt {
            allocation_absent: true,
        });
    }
    // Exercise the actual credential, including inherited/shared grants. Both
    // selectors are impossible, so permission probes cannot read or erase a
    // tenant row, or create an absent namespace. Never turn a denial into zero.
    let collection = majority_collection::<Document>(db, COLLECTION);
    collection
        .find_one(doc! {"_id":{"$in":[]}})
        .projection(doc! {"_id":1})
        .await
        .map_err(|error| failure(error, "find"))?;
    collection
        .delete_many(doc! {"_id":{"$in":[]}})
        .await
        .map_err(|error| failure(error, "remove"))?;
    Ok(Receipt {
        allocation_absent: false,
    })
}
