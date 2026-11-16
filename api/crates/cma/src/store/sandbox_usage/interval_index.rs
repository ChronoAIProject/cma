//! Additive interval lookup in the existing Sandbox store startup index path.
use layer2_core::StoreError;
use mongodb::{
    bson::{doc, Bson, Document},
    Database,
};
use std::time::Duration;

const NAME: &str = "sandbox_usage_events_interval";
const INVENTORY_MS: i32 = 2_000;
const BUILD_MS: i32 = 45_000;

fn keys() -> Document {
    doc! {"sandbox_id":1,"ended_at":1,"started_at":1,"revision":1}
}

fn unavailable(reason: &str) -> StoreError {
    StoreError::Unavailable(format!("usage interval index: {reason}"))
}

fn present(inventory: &[Document]) -> Result<bool, StoreError> {
    let expected = keys();
    // Query collation follows the collection; a differently collated lookup
    // cannot qualify the ordinary string Sandbox-ID predicates.
    if inventory
        .iter()
        .any(|index| index.get_str("name").ok() == Some("_id_") && index.contains_key("collation"))
    {
        return Err(unavailable("unreviewed collection collation"));
    }
    let mut found = false;
    for entry in inventory {
        let index = entry.get_document("spec").unwrap_or(entry);
        let same_keys = index
            .get_document("key")
            .is_ok_and(|key| key.iter().eq(expected.iter()));
        if !same_keys && index.get_str("name").ok() != Some(NAME) {
            continue;
        }
        if entry.contains_key("buildUUID")
            || entry.contains_key("spec")
            || found
            || !same_keys
            || ["unique", "sparse", "hidden"]
                .iter()
                .any(|field| !matches!(index.get(*field), None | Some(Bson::Boolean(false))))
            || index.keys().any(|field| {
                ![
                    "v",
                    "ns",
                    "name",
                    "key",
                    "unique",
                    "sparse",
                    "hidden",
                    "background",
                ]
                .contains(&field.as_str())
            })
            || index
                .get_str("name")
                .map_or(true, |name| name.is_empty() || name.len() > 128)
        {
            return Err(unavailable("conflicting or incomplete index"));
        }
        found = true;
    }
    Ok(found)
}

async fn inspect(database: &Database) -> Result<bool, StoreError> {
    let response = database
        .run_command(doc! {
            "listIndexes":super::EVENTS,"includeBuildUUIDs":true,
            "cursor":{"batchSize":64},"maxTimeMS":INVENTORY_MS,
        })
        .await
        .map_err(|_| unavailable("inventory unavailable"))?;
    let cursor = response
        .get_document("cursor")
        .map_err(|_| unavailable("inventory malformed"))?;
    let id = cursor
        .get_i64("id")
        .map_err(|_| unavailable("cursor malformed"))?;
    if id != 0 {
        let _ = database
            .run_command(doc! {
                "killCursors":super::EVENTS,"cursors":[id],"maxTimeMS":INVENTORY_MS,
            })
            .await;
        return Err(unavailable("inventory exceeds 64 indexes"));
    }
    let batch = cursor
        .get_array("firstBatch")
        .map_err(|_| unavailable("index batch malformed"))?;
    if batch.is_empty() || batch.len() > 64 {
        return Err(unavailable("index batch exceeds bounds"));
    }
    let inventory = batch
        .iter()
        .map(|index| {
            index
                .as_document()
                .cloned()
                .ok_or_else(|| unavailable("index malformed"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    present(&inventory)
}

pub(super) async fn ensure(database: &Database) -> Result<(), StoreError> {
    // Cancellation can leave an additive build in progress. A later startup
    // inspects the durable result and resumes the identical specification.
    tokio::time::timeout(Duration::from_secs(50), async {
        if inspect(database).await? {
            return Ok(());
        }
        let result = database
            .run_command(doc! {
                "createIndexes":super::EVENTS,
                "indexes":[{"name":NAME,"key":keys()}],
                "maxTimeMS":BUILD_MS,
                "writeConcern":{"w":"majority","wtimeout":5_000},
            })
            .await
            .map_err(|_| unavailable("build incomplete; inspect on next startup"))?;
        if result.contains_key("writeConcernError") {
            return Err(unavailable("build acknowledgement unavailable"));
        }
        if !inspect(database).await? {
            return Err(unavailable("postcondition missing"));
        }
        tracing::info!(index = NAME, "usage interval index ready");
        Ok(())
    })
    .await
    .map_err(|_| unavailable("startup deadline exceeded"))?
}

#[cfg(test)]
mod tests;
