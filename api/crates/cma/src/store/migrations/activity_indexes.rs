//! Immutable first additive step. Changes require a new ID and reviewed checksum.
use mongodb::bson::{doc, Bson, Document};
use sha2::{Digest, Sha256};

use super::Result;

pub(super) const ID: &str = "am-activity-query-indexes-v1";
const COLLECTION: &str = "activity";

pub(super) fn checksum() -> String {
    format!(
        "{:x}",
        Sha256::digest(include_bytes!("activity_indexes.rs"))
    )
}

pub(super) fn indexes() -> [Document; 2] {
    [
        doc! {"name":"record.actor_1_recorded_at_-1__id_-1", "key":{"record.actor":1,"recorded_at":-1,"_id":-1}},
        doc! {"name":"record.action_1_recorded_at_-1__id_-1", "key":{"record.action":1,"recorded_at":-1,"_id":-1}},
    ]
}

pub(super) fn missing(existing: &[Document]) -> Result<Vec<Document>> {
    if existing
        .iter()
        .any(|index| index.get_str("name").ok() == Some("_id_") && index.contains_key("collation"))
    {
        return Err("activity collection has an unreviewed default collation");
    }
    let mut pending = Vec::new();
    for desired in indexes() {
        let mut matches = existing.iter().filter(|index| {
            index.get("name") == desired.get("name") || index.get("key") == desired.get("key")
        });
        match matches.next() {
            None => pending.push(desired),
            Some(index) => {
                if matches.next().is_some()
                    || index.get("name") != desired.get("name")
                    || index.get("key") != desired.get("key")
                    || ["unique", "sparse", "hidden"]
                        .iter()
                        .any(|key| !matches!(index.get(*key), None | Some(Bson::Boolean(false))))
                    || index.keys().any(|key| {
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
                        .contains(&key.as_str())
                    })
                {
                    return Err("activity query index differs from reviewed prior state");
                }
            }
        }
    }
    Ok(pending)
}

pub(super) async fn inspect(database: &mongodb::Database) -> Result<Vec<Document>> {
    let response = database
        .run_command(doc! {"listIndexes":COLLECTION,"cursor":{"batchSize":64},"maxTimeMS":5000})
        .await;
    let response = match response {
        Ok(value) => value,
        Err(error) if matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(command) if command.code == 26) =>
        {
            return Ok(indexes().to_vec());
        }
        Err(_) => return Err("activity query indexes unavailable"),
    };
    let cursor = response
        .get_document("cursor")
        .map_err(|_| "index cursor malformed")?;
    if cursor.get_i64("id").ok() != Some(0) {
        return Err("activity index inventory exceeds bound");
    }
    let indexes = cursor
        .get_array("firstBatch")
        .map_err(|_| "index batch malformed")?
        .iter()
        .map(|item| item.as_document().cloned().ok_or("index malformed"))
        .collect::<Result<Vec<_>>>()?;
    missing(&indexes)
}

pub(super) async fn create(database: &mongodb::Database, index: Document) -> Result<bool> {
    let result = database
        .run_command(
            doc! {"createIndexes":COLLECTION,"indexes":[index],"maxTimeMS":10000,
            "writeConcern":{"w":"majority","wtimeout":5000}},
        )
        .await
        .map_err(|_| "additive index outcome uncertain; inspect and resume")?;
    if result.contains_key("writeConcernError") {
        return Err("additive index acknowledgement uncertain; inspect and resume");
    }
    let count = |field| {
        result
            .get_i64(field)
            .or_else(|_| result.get_i32(field).map(i64::from))
            .map_err(|_| "additive index result malformed")
    };
    let before = count("numIndexesBefore")?;
    let after = count("numIndexesAfter")?;
    if before < 0
        || before
            .checked_add(1)
            .is_none_or(|maximum| !(before..=maximum).contains(&after))
    {
        return Err("additive index count inconsistent");
    }
    Ok(after > before)
}
