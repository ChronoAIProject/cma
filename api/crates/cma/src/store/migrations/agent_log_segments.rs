//! Additive reader-format migration. Registration and ledger live in the runner.
use mongodb::Database;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::store::agent_log_segments::{schema, COLLECTIONS, MIGRATION, READER};

pub(super) const ID: &str = MIGRATION;
pub(super) const MAX_SCHEMA_STEPS: i64 = 64;

pub(super) fn checksum() -> String {
    let mut digest = Sha256::new();
    digest.update(include_bytes!("agent_log_segments.rs"));
    digest.update(include_bytes!("../agent_log_segments/schema.rs"));
    digest.update(include_bytes!("../agent_log_segments/schema/controls.rs"));
    digest.update(include_bytes!("../agent_log_segments/schema/validators.rs"));
    for identity in [ID, READER].into_iter().chain(COLLECTIONS.iter().copied()) {
        digest.update(identity.as_bytes());
        digest.update([0]);
    }
    digest.update(include_bytes!("../agent_log_segments/schema/recovery.rs"));
    digest.update(include_bytes!(
        "../agent_log_segments/schema/index_contract.rs"
    ));
    digest.update(include_bytes!(
        "../agent_capacity_recovery/format2.schema.json"
    ));
    digest.update(include_bytes!(
        "../agent_capacity_recovery/format2.indexes.json"
    ));
    format!("{:x}", digest.finalize())
}

pub(super) async fn inspect(database: &Database) -> Result<Value, &'static str> {
    let pending = pending_count(database).await?;
    Ok(
        json!({"migration":ID,"checksum":checksum(),"phase":"expand",
        "required_reader":READER,"collections":COLLECTIONS,"pending_steps":pending,
        "effects":"add sidecar collections, validators and indexes only",
        "activates_agents":false,"rewrites_events":false,"writes":false}),
    )
}

pub(super) async fn apply_next(database: &Database) -> Result<bool, &'static str> {
    schema::apply_next(database)
        .await
        .map_err(|_| "segment DDL outcome uncertain; inspect and resume")
}

async fn pending_count(database: &Database) -> super::Result<usize> {
    let count = schema::pending_count(database)
        .await
        .map_err(|_| "segment schema drift or unavailable")?;
    if count > MAX_SCHEMA_STEPS as usize {
        return Err("segment schema exceeds the reviewed migration step bound");
    }
    Ok(count)
}

pub(super) async fn apply(
    ledger: &super::ledger::Ledger,
    database: &Database,
    identity: &super::ledger::Identity,
) -> super::Result<Value> {
    if ledger.step != super::steps::Step::AgentLogSegments {
        return Err("segment migration ledger mismatch");
    }
    // Inspect before the first ledger or DDL write; drift never self-repairs.
    pending_count(database).await?;
    crate::store::sandbox_mongo::require_transaction_topology(database)
        .await
        .map_err(|_| "segment migration requires a supported replica set or mongos")?;
    ledger.initialize().await?;
    if ledger.completed().await? {
        if pending_count(database).await? != 0 {
            return Err("completed segment migration postcondition drift");
        }
        return Ok(json!({"migration":ID,"outcome":"already_completed"}));
    }
    let lease = ledger.claim(identity).await?;
    let outcome = async {
        let mut changed = 0_i64;
        for _ in 0..MAX_SCHEMA_STEPS {
            if pending_count(database).await? == 0 {
                break;
            }
            ledger.renew(&lease).await?;
            // One identical additive effect; no validator replacement or TTL.
            let effect = apply_next(database);
            tokio::pin!(effect);
            let mut renewal = tokio::time::interval(std::time::Duration::from_secs(5));
            let created = loop {
                tokio::select! {
                    result = &mut effect => break result?,
                    _ = renewal.tick() => ledger.renew(&lease).await?,
                }
            };
            changed += i64::from(created);
        }
        if pending_count(database).await? != 0 {
            return Err("segment migration postcondition unavailable");
        }
        Ok(changed)
    }
    .await;
    ledger.finish(&lease, outcome).await?;
    Ok(json!({"migration":ID,"outcome":"completed","applied_schema_steps":outcome?}))
}
