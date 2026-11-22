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

async fn database_now(db: &Database) -> String {
    let hello = db.run_command(doc! {"hello":1}).await.unwrap();
    layer2_core::rfc3339_at(hello.get_datetime("localTime").unwrap().to_system_time())
}

async fn wait_until(db: &Database, at: &str) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while database_now(db).await.as_str() < at {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("database clock must reach test deadline");
}

mod completion;
mod corruption;
mod leases;
mod queries;
mod receipts;
mod startup;
mod worker_claims;

#[path = "tests/worker_schema.rs"]
mod journal_schema;
mod worker_authority;
mod worker_boundaries;
mod worker_commit_errors;
mod worker_compatibility;
mod worker_fixture;
mod worker_journal;
mod worker_recovery;
