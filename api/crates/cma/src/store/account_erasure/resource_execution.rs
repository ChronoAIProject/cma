//! Indexed attribution selection and transaction-bound removal of execution evidence.
use super::{catalog::InventorySet, context::Context, Batch, ErasureBlock, Error};
use crate::store::resource_access::execution::erasure::{self, EraseError, Scope};
use mongodb::{
    bson::{doc, Document},
    Database,
};
use std::collections::BTreeSet;

pub(super) fn managed(name: &str) -> bool {
    erasure::COLLECTIONS.contains(&name)
}

pub(super) fn dependency_field(name: &str) -> Option<&'static str> {
    (name == erasure::OPERATIONS).then_some(erasure::SHARE)
}

pub(super) async fn namespaces(db: &Database) -> Result<BTreeSet<String>, Error> {
    Ok(db
        .list_collection_names()
        .authorized_collections(false)
        .filter(doc! {"name":{"$in":erasure::COLLECTIONS}})
        .await
        .map_err(|error| super::optional::failure(error, erasure::GRANTS, "listCollections"))?
        .into_iter()
        .collect())
}

pub(super) async fn erase(
    db: &Database,
    collection: &'static str,
    row: &Document,
    contexts: &[Context],
    batch: &mut Batch,
) -> Result<(), Error> {
    let context = contexts
        .iter()
        .find(|context| {
            super::context::value(row, erasure::OWNER).and_then(mongodb::bson::Bson::as_str)
                == Some(context.subject.as_str())
                && super::context::value(row, erasure::SANDBOX)
                    .and_then(mongodb::bson::Bson::as_str)
                    .is_some_and(|id| {
                        context
                            .sets
                            .get(&InventorySet::Sandboxes)
                            .is_some_and(|ids| ids.contains(id))
                    })
        })
        .or_else(|| {
            contexts.iter().find(|context| {
                super::context::value(row, erasure::ACTOR).and_then(mongodb::bson::Bson::as_str)
                    == Some(context.subject.as_str())
            })
        })
        .ok_or(Error::InvalidRecord(collection))?;
    let empty = BTreeSet::new();
    let scope = Scope {
        subject: &context.subject,
        sandboxes: context.sets.get(&InventorySet::Sandboxes).unwrap_or(&empty),
        organization_sandboxes: &context.organization_sandboxes,
    };
    let id = row
        .get_str("_id")
        .map_err(|_| Error::InvalidRecord(collection))?;
    let deleted =
        erasure::erase(db, collection, id, &scope)
            .await
            .map_err(|error| match error {
                EraseError::Retained => Error::blocked(
                    &context.subject,
                    ErasureBlock::UnsupportedResidue {
                        collection: collection.into(),
                    },
                ),
                EraseError::Corrupt => Error::InvalidRecord(collection).attribute(&context.subject),
                EraseError::Capacity => Error::blocked(
                    &context.subject,
                    ErasureBlock::UnsupportedResidue {
                        collection: collection.into(),
                    },
                ),
                EraseError::Mongo(error) => {
                    super::optional::failure(error, collection, "delete transaction")
                }
                EraseError::Unresolved => Error::Store(error.to_string()),
            })?;
    for (name, count) in deleted {
        *batch.deleted.entry(name.into()).or_default() += count;
    }
    Ok(())
}
