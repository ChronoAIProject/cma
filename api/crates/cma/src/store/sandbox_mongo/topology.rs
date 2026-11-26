use super::unavailable;
use cma_am_mongo_primitives::topology::{self, TopologyError};
use layer2_core::StoreError;
use mongodb::Database;

pub(in crate::store) async fn require_transaction_topology(
    database: &Database,
) -> Result<(), StoreError> {
    topology::require_transaction_topology(database)
        .await
        .map_err(|error| match error {
            TopologyError::Query(error) => unavailable("inspect Mongo transaction topology", error),
            TopologyError::Unsupported(detail) => StoreError::Unavailable(detail),
        })
}
