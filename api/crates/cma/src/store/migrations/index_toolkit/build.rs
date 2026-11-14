use super::super::{activity_inventory, activity_page_index, ledger, steps::Step, Result};
use super::inspection::number;
use super::{open_database, COLLECTION, LEASE_RENEWAL_INTERVAL, PROGRESS_INTERVAL};
use mongodb::{
    bson::{doc, Document},
    Database,
};
use serde_json::{json, Value};
use std::time::Instant;

pub(super) async fn build(checksum: &str, image: &str, timeout_seconds: u64) -> Result<Value> {
    if checksum != activity_page_index::checksum() {
        return Err("Activity index manifest checksum mismatch");
    }
    let source = option_env!("CHRONO_BUILD_SHA").unwrap_or("development");
    let identity = ledger::Identity::new(source, image)?;
    let (client, database, mode) = open_database().await?;
    let status = database
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
        .await
        .map_err(|_| "database authority unavailable")?;
    crate::store::isolation::verify_activity_index_authority(&status, database.name(), mode, true)
        .map_err(|_| "Activity index build authority is insufficient")?;
    crate::store::sandbox_mongo::require_transaction_topology(&database)
        .await
        .map_err(|_| "Activity index build requires a supported replica set or mongos")?;
    let ledger = ledger::Ledger::for_step(client, database.clone(), Step::ActivityPage);
    ledger.preflight().await?;
    ledger.initialize().await?;
    if ledger.completed().await? {
        if !activity_inventory::strict_pending(&database)
            .await?
            .is_empty()
        {
            return Err("completed Activity index manifest drifted");
        }
        return Ok(json!({
            "migration":Step::ActivityPage.id(),
            "outcome":"already_completed",
            "online":true,
        }));
    }
    let lease = ledger.claim(&identity).await?;
    tracing::info!(
        migration = Step::ActivityPage.id(),
        phase = "inventory",
        timeout_seconds,
        "Activity index build admitted"
    );
    let outcome = async {
        let pending = activity_inventory::strict_pending(&database).await?;
        if pending.len() > 1 {
            return Err("Activity index manifest has no single pending index");
        }
        if pending.is_empty() {
            tracing::info!(
                migration = Step::ActivityPage.id(),
                phase = "verified",
                created = false,
                "Activity index already exists; completing reconciled manifest"
            );
            return Ok(0);
        }
        ledger.renew(&lease).await?;
        let started = Instant::now();
        tracing::info!(
            migration = Step::ActivityPage.id(),
            phase = "build_started",
            timeout_seconds,
            "Activity index online build started"
        );
        let created = create_online(
            &ledger,
            &lease,
            &database,
            pending[0].clone(),
            timeout_seconds,
            started,
        )
        .await?;
        let remaining = activity_inventory::strict_pending(&database).await?;
        if !remaining.is_empty() {
            return Err("Activity index postcondition unavailable");
        }
        tracing::info!(
            migration = Step::ActivityPage.id(),
            phase = "verified",
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Activity index online build verified"
        );
        Ok(i64::from(created))
    }
    .await;
    ledger.finish(&lease, outcome).await?;
    let created = outcome?;
    Ok(json!({
        "migration":Step::ActivityPage.id(),
        "outcome":"completed",
        "created_indexes":created,
        "online":true,
        "timeout_seconds":timeout_seconds,
        "progress":"inventory,build_started,verified emitted as sanitized log phases",
    }))
}

async fn create_online(
    ledger: &ledger::Ledger,
    lease: &ledger::Lease,
    database: &Database,
    index: Document,
    timeout_seconds: u64,
    started: Instant,
) -> Result<bool> {
    let timeout_ms = i32::try_from(timeout_seconds.saturating_mul(1_000))
        .map_err(|_| "Activity index timeout is too large")?;
    let command = async {
        database
            .run_command(doc! {
                "createIndexes":COLLECTION,
                "indexes":[index],
                "maxTimeMS":timeout_ms,
                "writeConcern":{"w":"majority","wtimeout":timeout_ms},
                // Modern MongoDB builds indexes online; omit removed `background`.
                "comment":"cma-activity-page-index-v1",
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
                break result.map_err(|_| "Activity index build outcome uncertain; inspect and resume")?;
            }
            _ = progress.tick() => {
                tracing::info!(
                    migration = Step::ActivityPage.id(),
                    phase = "build_progress",
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    timeout_seconds,
                    "Activity index online build is still running"
                );
            }
            _ = renewal.tick() => {
                // Renew long builds so another executor cannot claim the migration.
                ledger.renew(lease).await?;
            }
        }
    };
    if result.contains_key("writeConcernError") {
        return Err("Activity index acknowledgement uncertain; inspect and resume");
    }
    let before = number(&result, "numIndexesBefore")?;
    let after = number(&result, "numIndexesAfter")?;
    if before < 0 || after < before || after > before + 1 {
        return Err("Activity index result count inconsistent");
    }
    tracing::info!(
        migration = Step::ActivityPage.id(),
        phase = "build_finished",
        elapsed_ms = started.elapsed().as_millis() as u64,
        created = after > before,
        "Activity index build command returned"
    );
    Ok(after > before)
}
