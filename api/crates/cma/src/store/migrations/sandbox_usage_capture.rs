use crate::store::sandbox_usage_export::{schema, COLLECTIONS};
use mongodb::Database;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(super) const ID: &str = schema::MIGRATION;

pub(super) fn checksum() -> String {
    let mut digest = Sha256::new();
    digest.update(ID);
    digest.update(include_bytes!("sandbox_usage_capture.rs"));
    digest.update(include_bytes!("../sandbox_usage_export/schema.rs"));
    digest.update(include_bytes!("../schema/index_contract_v1.rs"));
    for name in COLLECTIONS {
        digest.update(name);
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())
}

pub(super) async fn inspect(database: &Database) -> super::Result<Value> {
    Ok(
        json!({"migration":ID,"checksum":checksum(),"phase":"expand",
        "collections":COLLECTIONS,"pending_steps":schema::pending(database).await?,
        "effects":"add retained usage collections and unique indexes",
        "writes":false,"backfill":false,"activation":false,"ttl":false}),
    )
}

pub(super) async fn apply(
    ledger: &super::ledger::Ledger,
    database: &Database,
    identity: &super::ledger::Identity,
) -> super::Result<Value> {
    if ledger.step != super::steps::Step::SandboxUsageCapture {
        return Err("usage migration identity mismatch");
    }
    schema::pending(database).await?;
    crate::store::sandbox_mongo::require_transaction_topology(database)
        .await
        .map_err(|_| "usage migration requires transactions")?;
    ledger.initialize().await?;
    if ledger.completed().await? {
        if schema::pending(database).await? != 0 {
            return Err("usage schema drift");
        }
        return Ok(json!({"migration":ID,"outcome":"already_completed"}));
    }
    let lease = ledger.claim(identity).await?;
    let outcome = async {
        let mut changed = 0;
        for _ in 0..schema::MAX_STEPS {
            if schema::pending(database).await? == 0 {
                break;
            }
            ledger.renew(&lease).await?;
            let effect = schema::apply_next(database);
            tokio::pin!(effect);
            let mut renew = tokio::time::interval(std::time::Duration::from_secs(5));
            let applied = loop {
                tokio::select! {
                    result = &mut effect => break result?,
                    _ = renew.tick() => ledger.renew(&lease).await?,
                }
            };
            changed += i64::from(applied);
        }
        if schema::pending(database).await? != 0 {
            return Err("usage schema incomplete");
        }
        Ok(changed)
    }
    .await;
    ledger.finish(&lease, outcome).await?;
    Ok(json!({"migration":ID,"outcome":"completed","applied_schema_steps":outcome?}))
}
