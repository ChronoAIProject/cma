//! Exact-target abort with a durable barrier until its destructive effect settles.
use super::super::{index_inventory, ledger, Result};
use super::{catalog::Query, open_database};
use mongodb::{bson::doc, Client, Database};
use serde_json::{json, Value};

pub(super) async fn abort(query: &Query, checksum: &str, image: &str, fence: i64) -> Result<Value> {
    let step = query.migration()?;
    if checksum != step.checksum() {
        return Err("abort manifest checksum mismatch");
    }
    let identity = ledger::Identity::new(crate::build_stamp().1.unwrap_or("development"), image)?;
    let (client, database, mode) = open_database().await?;
    super::authority::verify(&database, mode, &[query.collection], true).await?;
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
    execute_inner(
        query,
        client,
        database,
        identity,
        image,
        previous_fence,
        #[cfg(test)]
        Hooks::default(),
    )
    .await
}

pub(super) async fn execute_inner(
    query: &Query,
    client: Client,
    database: Database,
    identity: &ledger::Identity,
    image: &str,
    previous_fence: i64,
    #[cfg(test)] mut hooks: Hooks,
) -> Result<Value> {
    let step = query.migration()?;
    crate::store::sandbox_mongo::require_transaction_topology(&database)
        .await
        .map_err(|_| "index abort requires supported transactional topology")?;
    let ledger = ledger::Ledger::for_step(client, database.clone(), step);
    ledger.preflight().await?;
    let status = ledger.status().await?;
    previous_attempt(&status, image, previous_fence)?;
    let lease = ledger.claim_abort(identity, previous_fence).await?;
    #[cfg(test)]
    pause(hooks.pending.take()).await;
    let inventory =
        index_inventory::strict_pending(&database, query.collection, &query.index).await;
    let refusal = match inventory {
        Ok(pending) if pending.is_empty() => None,
        Ok(_) => Some("abort target is not visible; reconcile server build state"),
        Err(reason) => Some(reason),
    };
    if let Some(reason) = refusal {
        ledger.settle_abort(&lease, false, reason).await?;
        return Err(reason);
    }
    // Persist dispatch authority before constructing the command future. No
    // successor may claim while this continuation can still issue a deletion.
    ledger.begin_abort(&lease).await?;
    #[cfg(test)]
    pause(hooks.draining.take()).await;
    tracing::warn!(
        query = query.name,
        migration = step.id(),
        fence = lease.fence,
        phase = "abort_started",
        "Dropping the exact approved incomplete index"
    );
    let response = database
        .run_command(drop_command(query)?)
        .await
        .map_err(|_| "index drop outcome uncertain; destructive barrier retained for reviewed forward recovery")?;
    if response.contains_key("writeConcernError") {
        return Err("index drop acknowledgement uncertain; destructive barrier retained for reviewed forward recovery");
    }
    #[cfg(test)]
    pause(hooks.acknowledged.take()).await;
    let pending =
        index_inventory::strict_pending(&database, query.collection, &query.index).await?;
    if pending.len() != 1 {
        return Err(
            "index remains after drop; destructive barrier retained for server reconciliation",
        );
    }
    // This future has no remaining send/retry path. Settlement and its failed
    // build receipt commit together; an expired lease alone cannot release it.
    ledger
        .settle_abort(&lease, true, "index build aborted by approved operator")
        .await?;
    Ok(
        json!({"migration":step.id(),"query":query.name,"outcome":"drop_acknowledged",
        "fence":lease.fence,"manifest_completed":false,"server_reconciliation_required":true,"destructive_state":"settled"}),
    )
}

#[cfg(test)]
pub(super) type Checkpoint = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);

#[cfg(test)]
#[derive(Default)]
pub(super) struct Hooks {
    pub pending: Option<Checkpoint>,
    pub draining: Option<Checkpoint>,
    pub acknowledged: Option<Checkpoint>,
}

#[cfg(test)]
async fn pause(checkpoint: Option<Checkpoint>) {
    if let Some((reached, resume)) = checkpoint {
        reached.send(()).unwrap();
        resume.await.unwrap();
    }
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

pub(super) fn drop_command(query: &Query) -> Result<mongodb::bson::Document> {
    let step = query.migration()?;
    Ok(
        doc! {"dropIndexes":query.collection,"index":query.index_name(),"maxTimeMS":5000,
        "writeConcern":{"w":"majority","wtimeout":5000},"comment":step.id()},
    )
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
        let command = drop_command(&super::super::catalog::activity()).unwrap();
        assert_eq!(
            command.get_str("index").unwrap(),
            "activity_recorded_at_id_desc"
        );
        assert_eq!(command.get_i32("maxTimeMS").unwrap(), 5000);
    }
}
