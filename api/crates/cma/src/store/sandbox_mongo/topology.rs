//! Inspect the store connection for transaction and snapshot support.
use super::unavailable;
use cma_am_mongo_primitives::topology::validate_transaction_server;
use layer2_core::StoreError;
use mongodb::{bson::doc, Database};

pub(in crate::store) async fn require_transaction_topology(
    database: &Database,
) -> Result<(), StoreError> {
    let reply = database
        .run_command(doc! { "hello": 1_i32 })
        .await
        .map_err(|error| unavailable("inspect Mongo transaction topology", error))?;
    validate_transaction_server(&reply).map_err(StoreError::Unavailable)
}
