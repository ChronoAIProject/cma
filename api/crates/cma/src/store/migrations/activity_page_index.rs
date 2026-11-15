//! Explicit global Activity page index; never installed by startup or reads.
use mongodb::bson::{doc, Bson, Document};
use sha2::{Digest, Sha256};

use super::Result;

pub(super) const ID: &str = "am-activity-page-index-v1";
const COLLECTION: &str = "activity";

pub(super) fn checksum() -> String {
    format!(
        "{:x}",
        Sha256::digest(include_bytes!("activity_page_index.rs"))
    )
}

pub(super) fn indexes() -> [Document; 1] {
    [doc! {"name":"activity_recorded_at_id_desc", "key":{"recorded_at":-1,"_id":-1}}]
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
        let mut matches = existing
            .iter()
            .filter(|index| index.get("name") == desired.get("name") || same_keys(index, &desired));
        match matches.next() {
            None => pending.push(desired),
            Some(index) => {
                if matches.next().is_some()
                    || index.get("name") != desired.get("name")
                    || !same_keys(index, &desired)
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

fn same_keys(left: &Document, right: &Document) -> bool {
    match (left.get_document("key"), right.get_document("key")) {
        // BSON Document equality ignores field order; compound indexes do not.
        (Ok(left), Ok(right)) => left.iter().eq(right.iter()),
        _ => false,
    }
}

pub(super) async fn inspect(database: &mongodb::Database) -> Result<Vec<Document>> {
    let response = database
        .run_command(doc! {"listIndexes":COLLECTION,"cursor":{"batchSize":64},"maxTimeMS":5000})
        .await;
    // Collection creation and retention belong to service initialization, not this step.
    let response = response.map_err(|_| "activity collection/index inventory unavailable")?;
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
    let desired = indexes()[0].clone();
    if index.len() != 2 || index.get("name") != desired.get("name") || !same_keys(&index, &desired)
    {
        return Err("unreviewed activity page index request");
    }
    super::activity_indexes::create(database, index).await
}
