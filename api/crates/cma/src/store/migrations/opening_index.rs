//! Immutable additive opening lookup index. No startup or observer DDL.
use super::Result;
use mongodb::bson::{doc, Bson, Document};
use sha2::{Digest, Sha256};

pub(super) const ID: &str = "am-agent-opening-index-v1";
pub(super) const COLLECTIONS: [&str; 1] = ["agent_log"];

pub(super) fn checksum() -> String {
    format!("{:x}", Sha256::digest(include_bytes!("opening_index.rs")))
}

pub(in crate::store) fn indexes(collection: &str) -> Vec<Document> {
    match collection {
        "agent_log" => vec![doc! {
            "name":"agent_log_kind_sequence",
            "key":{"agent_id":1,"record.kind.type":1,"seq":1},
        }],
        _ => Vec::new(),
    }
}

pub(in crate::store) fn missing(collection: &str, existing: &[Document]) -> Result<Vec<Document>> {
    if !COLLECTIONS.contains(&collection) {
        return Err("unknown opening index step");
    }
    if existing
        .iter()
        .any(|index| index.get_str("name").ok() == Some("_id_") && index.contains_key("collation"))
    {
        return Err("opening collection has an unreviewed default collation");
    }
    let mut pending = Vec::new();
    for desired in indexes(collection) {
        let same_keys = |index: &&Document| {
            index
                .get_document("key")
                .ok()
                .is_some_and(|key| key.iter().eq(desired.get_document("key").unwrap().iter()))
        };
        let mut found = existing
            .iter()
            .filter(|index| index.get("name") == desired.get("name") || same_keys(index));
        match found.next() {
            None => pending.push(desired),
            Some(index) => {
                if found.next().is_some()
                    || index.get("name") != desired.get("name")
                    || !same_keys(&index)
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
                    return Err("opening index differs from reviewed prior state");
                }
            }
        }
    }
    Ok(pending)
}

pub(in crate::store) async fn inspect(
    database: &mongodb::Database,
) -> Result<Vec<(&'static str, Document)>> {
    let mut pending = Vec::new();
    for collection in COLLECTIONS {
        // Refuse missing collections: this step never initializes the service.
        let response = database
            .run_command(doc! {"listIndexes":collection,"cursor":{"batchSize":64},"maxTimeMS":5000})
            .await
            .map_err(|_| {
                "opening index inventory unavailable; initialize service collections first"
            })?;
        let cursor = response
            .get_document("cursor")
            .map_err(|_| "index cursor malformed")?;
        if cursor.get_i64("id").ok() != Some(0) {
            return Err("opening index inventory exceeds bound");
        }
        let existing = cursor
            .get_array("firstBatch")
            .map_err(|_| "index batch malformed")?
            .iter()
            .map(|value| value.as_document().cloned().ok_or("index malformed"))
            .collect::<Result<Vec<_>>>()?;
        for index in missing(collection, &existing)? {
            pending.push((collection, index));
        }
    }
    Ok(pending)
}

pub(super) async fn create(
    database: &mongodb::Database,
    collection: &str,
    index: Document,
) -> Result<bool> {
    if !indexes(collection).iter().any(|expected| {
        index.len() == 2
            && expected.get("name") == index.get("name")
            && index
                .get_document("key")
                .ok()
                .is_some_and(|key| key.iter().eq(expected.get_document("key").unwrap().iter()))
    }) {
        return Err("unreviewed opening index request");
    }
    let result = database
        .run_command(
            doc! {"createIndexes":collection,"indexes":[index],"maxTimeMS":10000,
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
            .is_none_or(|max| !(before..=max).contains(&after))
    {
        return Err("additive index count inconsistent");
    }
    Ok(after > before)
}
