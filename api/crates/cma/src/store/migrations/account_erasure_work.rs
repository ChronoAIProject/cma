use super::{ledger::Ledger, steps::Step, Result};
use crate::store::{
    account_deletions::worker_schema as schema, schema::index_contract_v1 as contract,
};
use mongodb::{
    bson::{doc, Document},
    Database,
};
use sha2::{Digest, Sha256};

pub(super) const ID: &str = "am-account-erasure-work-v1";
pub(super) fn checksum() -> String {
    let mut hash = Sha256::new();
    hash.update(ID);
    hash.update(include_bytes!("account_erasure_work.rs"));
    hash.update(include_bytes!("../account_deletions/worker_schema.rs"));
    hash.update(include_bytes!("../schema/index_contract_v1.rs"));
    format!("{:x}", hash.finalize())
}
pub(super) async fn inspect(database: &Database) -> Result<Vec<(&'static str, Document)>> {
    schema::pending(database).await
}
pub(super) async fn create(
    database: &Database,
    collection: &str,
    definition: Document,
) -> Result<bool> {
    if !schema::COLLECTIONS.contains(&collection) {
        return Err("erasure migration namespace mismatch");
    }
    let existing = contract::inspect_collection(database, collection).await?;
    if definition == doc! {"kind":"collection"} {
        if existing.is_some() {
            return Ok(false);
        }
        let result = database.run_command(doc! {"create":collection,"writeConcern":{"w":"majority","wtimeout":5000},"maxTimeMS":10000}).await.map_err(|_| "erasure collection preparation uncertain")?;
        if result.contains_key("writeConcernError") {
            return Err("erasure collection acknowledgement uncertain");
        }
        return Ok(true);
    }
    if !schema::indexes(collection).iter().any(|expected| {
        expected == &definition
            && expected
                .get_document("key")
                .ok()
                .map(|key| key.iter().collect::<Vec<_>>())
                == definition
                    .get_document("key")
                    .ok()
                    .map(|key| key.iter().collect::<Vec<_>>())
    }) {
        return Err("erasure migration index mismatch");
    }
    let existing = existing.ok_or("erasure collection preparation required")?;
    if contract::missing(&existing, &definition)?.is_empty() {
        return Ok(false);
    }
    let result = database.run_command(doc! {"createIndexes":collection,"indexes":[definition],"writeConcern":{"w":"majority","wtimeout":5000},"maxTimeMS":10000}).await.map_err(|_| "erasure index preparation uncertain")?;
    if result.contains_key("writeConcernError") {
        return Err("erasure index acknowledgement uncertain");
    }
    Ok(true)
}
pub(in crate::store) async fn prepared(database: &Database) -> Result<()> {
    let ledger = Ledger::for_step(
        database.client().clone(),
        database.clone(),
        Step::AccountErasureWork,
    );
    if !ledger.completed().await? {
        return Err("erasure migration completion receipt required");
    }
    Ok(())
}

pub(super) async fn apply(
    ledger: &Ledger,
    database: &Database,
    identity: &super::ledger::Identity,
) -> Result<serde_json::Value> {
    inspect(database).await?;
    crate::store::sandbox_mongo::require_transaction_topology(database)
        .await
        .map_err(|_| "erasure preparation needs transaction topology")?;
    ledger.initialize().await?;
    if ledger.completed().await? {
        if !inspect(database).await?.is_empty() {
            return Err("erasure preparation drift");
        }
        return Ok(serde_json::json!({"migration":ID,"outcome":"already_completed"}));
    }
    let lease = ledger.claim(identity).await?;
    let outcome = async {
        let mut changed = 0;
        for _ in 0..4 {
            let Some((collection, definition)) = inspect(database).await?.into_iter().next() else {
                break;
            };
            ledger.renew(&lease).await?;
            changed += i64::from(create(database, collection, definition).await?);
        }
        if !inspect(database).await?.is_empty() {
            return Err("erasure preparation incomplete");
        }
        Ok(changed)
    }
    .await;
    ledger.finish(&lease, outcome).await?;
    Ok(serde_json::json!({"migration":ID,"outcome":"completed","applied_schema_steps":outcome?}))
}

#[cfg(test)]
pub(in crate::store) async fn prepare_for_test(database: &Database) {
    let ledger = Ledger::for_step(
        database.client().clone(),
        database.clone(),
        Step::AccountErasureWork,
    );
    let identity = super::ledger::Identity::new(
        &"a".repeat(40),
        &format!("ghcr.io/chronoaiproject/cma-api@sha256:{}", "b".repeat(64)),
    )
    .unwrap();
    apply(&ledger, database, &identity).await.unwrap();
}
