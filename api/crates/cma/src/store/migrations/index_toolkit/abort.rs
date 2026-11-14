//! Exact-target abort after the build client is stopped and its lease expires.
use super::super::{index_inventory, ledger, Result};
use super::{catalog::Query, open_database};
use mongodb::{bson::doc, Client, Database};
use serde_json::{json, Value};

pub(super) async fn abort(query: &Query, checksum: &str, image: &str, fence: i64) -> Result<Value> {
    if checksum != query.step.checksum() {
        return Err("abort manifest checksum mismatch");
    }
    let identity = ledger::Identity::new(
        option_env!("CHRONO_BUILD_SHA").unwrap_or("development"),
        image,
    )?;
    let (client, database, _) = open_database().await?;
    let status = database
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true,"maxTimeMS":5000})
        .await
        .map_err(|_| "abort authority unavailable")?;
    super::authority::verify_abort(&status, database.name(), query.collection)?;
    execute(query, client, database, &identity, image, fence).await
}

pub(super) async fn execute(
    query: &Query,
    client: Client,
    database: Database,
    identity: &ledger::Identity,
    image: &str,
    previous_fence: i64,
) -> Result<Value> {
    crate::store::sandbox_mongo::require_transaction_topology(&database)
        .await
        .map_err(|_| "index abort requires supported transactional topology")?;
    let ledger = ledger::Ledger::for_step(client, database.clone(), query.step);
    ledger.preflight().await?;
    let status = ledger.status().await?;
    previous_attempt(&status, image, previous_fence)?;
    // This transactional claim refuses a live executor or concurrent completion.
    // Requiring the immediate next global fence prevents aborting a later build.
    let lease = ledger.claim(identity).await?;
    let outcome = async {
        if lease.fence
            != previous_fence
                .checked_add(1)
                .ok_or("abort fence overflow")?
        {
            return Err("abort predecessor changed; reconcile before retry");
        }
        if !index_inventory::strict_pending(&database, query.collection, &query.index)
            .await?
            .is_empty()
        {
            return Err("abort target is not visible; reconcile server build state");
        }
        ledger.renew(&lease).await?;
        tracing::warn!(
            query = query.name,
            migration = query.step.id(),
            fence = lease.fence,
            phase = "abort_started",
            "Dropping the exact approved incomplete index"
        );
        let response = database
            .run_command(drop_command(query))
            .await
            .map_err(|_| "index drop outcome uncertain; reconcile server and receipt")?;
        if response.contains_key("writeConcernError") {
            return Err("index drop acknowledgement uncertain");
        }
        let pending =
            index_inventory::strict_pending(&database, query.collection, &query.index).await?;
        if pending.len() != 1 {
            return Err("index remains after drop; reconcile server state");
        }
        Ok(())
    }
    .await;
    // An abort is always a failed build receipt. It cannot mark the manifest
    // complete, erase history or let the predecessor publish after the drop.
    ledger
        .finish(
            &lease,
            Err(outcome
                .err()
                .unwrap_or("index build aborted by approved operator")),
        )
        .await?;
    outcome?;
    Ok(
        json!({"migration":query.step.id(),"query":query.name,"outcome":"drop_acknowledged",
        "fence":lease.fence,"manifest_completed":false,"server_reconciliation_required":true}),
    )
}

fn previous_attempt(status: &Value, image: &str, fence: i64) -> Result<()> {
    if status["completed"] != false {
        return Err("completed manifest requires reviewed forward recovery");
    }
    let previous = status["recent_attempts"]
        .as_array()
        .and_then(|attempts| attempts.first())
        .ok_or("abort requires an existing build attempt")?;
    if previous["image"].as_str() != Some(image)
        || previous["fence"].as_i64() != Some(fence)
        || !matches!(previous["outcome"].as_str(), Some("running" | "failed"))
    {
        return Err("abort attempt identity does not match");
    }
    Ok(())
}

pub(super) fn drop_command(query: &Query) -> mongodb::bson::Document {
    doc! {"dropIndexes":query.collection,"index":query.index_name(),"maxTimeMS":5000,
    "writeConcern":{"w":"majority","wtimeout":5000},"comment":query.step.id()}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abort_is_exact_and_rejects_completed_or_stale_attempts() {
        let status = json!({"completed":false,"recent_attempts":[{"fence":8,"image":"image","outcome":"failed"}]});
        assert!(previous_attempt(&status, "image", 8).is_ok());
        assert!(previous_attempt(&status, "other-image", 8).is_err());
        assert!(previous_attempt(&status, "image", 7).is_err());
        let mut completed = status.clone();
        completed["completed"] = true.into();
        assert!(previous_attempt(&completed, "image", 8).is_err());
        let command = drop_command(&super::super::catalog::activity());
        assert_eq!(
            command.get_str("index").unwrap(),
            "activity_recorded_at_id_desc"
        );
        assert_eq!(command.get_i32("maxTimeMS").unwrap(), 5000);
    }
}
