use super::super::{index_inventory, ledger, Result};
use super::inspection::number;
use super::{open_database, LEASE_RENEWAL_INTERVAL, PROGRESS_INTERVAL};
use mongodb::{
    bson::{doc, Document},
    Database,
};
use serde_json::{json, Value};
use std::time::Instant;

pub(super) async fn build(
    query: &super::catalog::Query,
    checksum: &str,
    image: &str,
    timeout_seconds: u64,
) -> Result<Value> {
    let step = query.migration()?;
    if checksum != step.checksum() {
        return Err("Query index manifest checksum mismatch");
    }
    let source = crate::build_stamp().1.unwrap_or("development");
    let identity = ledger::Identity::new(source, image)?;
    let (client, database, mode) = open_database().await?;
    super::authority::verify(&database, mode, &[query.collection], true).await?;
    execute(query, client, database, &identity, timeout_seconds).await
}

pub(super) async fn execute(
    query: &super::catalog::Query,
    client: mongodb::Client,
    database: Database,
    identity: &ledger::Identity,
    timeout_seconds: u64,
) -> Result<Value> {
    let step = query.migration()?;
    crate::store::sandbox_mongo::require_transaction_topology(&database)
        .await
        .map_err(|_| "Query index build requires a supported replica set or mongos")?;
    let ledger = ledger::Ledger::for_step(client, database.clone(), step);
    ledger.preflight().await?;
    ledger.initialize().await?;
    if ledger.completed().await? {
        if !index_inventory::strict_pending(&database, query.collection, &query.index)
            .await?
            .is_empty()
        {
            return Err("completed Query index manifest drifted");
        }
        return Ok(json!({
            "migration":step.id(),
            "outcome":"already_completed",
            "online":true,
        }));
    }
    let lease = ledger.claim(identity).await?;
    tracing::info!(
        migration = step.id(),
        phase = "inventory",
        fence = lease.fence,
        timeout_seconds,
        "Query index build admitted"
    );
    let outcome = async {
        let pending =
            index_inventory::strict_pending(&database, query.collection, &query.index).await?;
        if pending.len() > 1 {
            return Err("Query index manifest has no single pending index");
        }
        if pending.is_empty() {
            tracing::info!(
                migration = step.id(),
                phase = "verified",
                created = false,
                "Query index already exists; completing reconciled manifest"
            );
            return Ok(0);
        }
        ledger.renew(&lease).await?;
        let started = Instant::now();
        tracing::info!(
            migration = step.id(),
            phase = "build_started",
            timeout_seconds,
            "Query index online build started"
        );
        let created = create_online(
            query,
            &ledger,
            &lease,
            &database,
            pending[0].clone(),
            timeout_seconds,
            started,
        )
        .await?;
        let remaining =
            index_inventory::strict_pending(&database, query.collection, &query.index).await?;
        if !remaining.is_empty() {
            return Err("Query index postcondition unavailable");
        }
        tracing::info!(
            migration = step.id(),
            phase = "verified",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Query index online build verified"
        );
        Ok(i64::from(created))
    }
    .await;
    ledger.finish(&lease, outcome).await?;
    let created = outcome?;
    Ok(json!({
        "migration":step.id(),
        "outcome":"completed",
        "created_indexes":created,
        "online":true,
        "timeout_seconds":timeout_seconds,
        "progress":"inventory,build_started,verified emitted as sanitized log phases",
    }))
}

async fn create_online(
    query: &super::catalog::Query,
    ledger: &ledger::Ledger,
    lease: &ledger::Lease,
    database: &Database,
    index: Document,
    timeout_seconds: u64,
    started: Instant,
) -> Result<bool> {
    let step = query.migration()?;
    let timeout_ms = i32::try_from(timeout_seconds.saturating_mul(1_000))
        .map_err(|_| "Query index timeout is too large")?;
    let command = async {
        database
            .run_command(doc! {
                "createIndexes":query.collection,
                "indexes":[index],
                "maxTimeMS":timeout_ms,
                "writeConcern":{"w":"majority","wtimeout":timeout_ms},
                // Modern MongoDB builds indexes online; omit removed `background`.
                "comment":step.id(),
            })
            .await
    };
    tokio::pin!(command);
    let mut progress = tokio::time::interval(PROGRESS_INTERVAL);
    let mut renewal = tokio::time::interval(LEASE_RENEWAL_INTERVAL);
    // Skip the immediate progress tick so heartbeats mean the command is in flight.
    progress.tick().await;
    let result = loop {
        tokio::select! {
            result = &mut command => {
                break result.map_err(|_| "Query index build outcome uncertain; inspect and resume")?;
            }
            _ = progress.tick() => {
                tracing::info!(
                    migration = step.id(),
                    phase = "build_progress",
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    timeout_seconds,
                    "Query index online build is still running"
                );
            }
            _ = renewal.tick() => {
                // Renew long builds so another executor cannot claim the migration.
                ledger.renew(lease).await?;
            }
        }
    };
    if result.contains_key("writeConcernError") {
        return Err("Query index acknowledgement uncertain; inspect and resume");
    }
    let before = number(&result, "numIndexesBefore")?;
    let after = number(&result, "numIndexesAfter")?;
    if before < 0 || after < before || after > before + 1 {
        return Err("Query index result count inconsistent");
    }
    tracing::info!(
        migration = step.id(),
        phase = "build_finished",
        elapsed_ms = started.elapsed().as_millis() as u64,
        created = after > before,
        "Query index build command returned"
    );
    Ok(after > before)
}
