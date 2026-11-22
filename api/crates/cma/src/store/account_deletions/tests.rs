use super::*;
use layer2_core::account_deletion::{
    AccountDeletionStatus, AccountDeletionStore, ErasureStep, ResidueReport,
};
use std::time::Duration;
mod legacy_blocks;
const NOW: &str = "2026-11-14T00:00:00Z";
const LATER: &str = "2026-11-14T00:02:00Z";
async fn fixture() -> (mongodb::Client, Database, MongoAccountDeletionStore) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("isolated replica set required");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!("cma_acd_{}", uuid::Uuid::new_v4().simple()));
    let store = MongoAccountDeletionStore::open(&db).await.unwrap();
    (client, db, store)
}

mod completion;
mod corruption;
mod leases;
mod queries;
mod receipts;
mod startup;
