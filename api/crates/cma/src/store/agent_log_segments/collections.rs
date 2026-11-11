use crate::store::{durability::majority_collection, sandbox_mongo::MongoSandboxStore};
use mongodb::{bson::Document, Collection};

/// Handles use the same client/database as the original Agent collection.
/// Constructing them performs no DDL, preparation or configuration mutation.
pub(super) fn collection(store: &MongoSandboxStore, name: &str) -> Collection<Document> {
    majority_collection(&store.client.database(&store.agents.namespace().db), name)
}
