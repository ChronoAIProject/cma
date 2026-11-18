//! Immutable additive binding preparation; no constraint retirement or TTL effects.

use mongodb::{
    bson::{doc, Bson, Document},
    Database,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{Result, IO};
use crate::store::{bindings::schema::indexes_v1 as bindings, schema::index_contract_v1 as schema};

pub(super) const ID: &str = "am-binding-indexes-v1";

pub(super) fn checksum() -> String {
    let mut digest = Sha256::new();
    digest.update(ID);
    digest.update(include_bytes!("binding_indexes.rs"));
    digest.update(include_bytes!("../bindings/schema/indexes_v1.rs"));
    digest.update(include_bytes!("../schema/index_contract_v1.rs"));
    format!("{:x}", digest.finalize())
}

async fn require_collections(database: &Database) -> Result<()> {
    for &collection in bindings::COLLECTIONS {
        if schema::inspect_collection(database, collection)
            .await?
            .is_none()
        {
            return Err("binding collections require separately approved bootstrap");
        }
    }
    schema::require_binding_versions(database, bindings::COLLECTIONS).await?;
    Ok(())
}

pub(super) async fn inspect(database: &Database) -> Result<Vec<(&'static str, Document)>> {
    require_collections(database).await?;
    let mut pending = Vec::new();
    for &collection in bindings::COLLECTIONS {
        let existing = schema::inspect_collection(database, collection)
            .await?
            .ok_or("binding collection prior state drift")?;
        for index in bindings::missing(collection, &existing)? {
            if index.get_bool("unique").unwrap_or(false) {
                refuse_duplicates(database, collection, &index).await?;
            }
            pending.push((collection, index));
        }
    }
    Ok(pending)
}

// This scan is advisory preflight, never a writer fence. Mongo's unique-index
// commit validates concurrent writes too; conflicts abort without repairing rows.
async fn refuse_duplicates(database: &Database, collection: &str, index: &Document) -> Result<()> {
    let keys = index
        .get_document("key")
        .map_err(|_| "invalid binding index contract")?;
    let mut group = Document::new();
    let mut arrays = Vec::new();
    for (position, key) in keys.keys().enumerate() {
        group.insert(
            format!("key{position}"),
            doc! {"$ifNull":[format!("${key}"), Bson::Null]},
        );
        arrays.push(doc! {key: {"$type":"array"}});
    }
    let rows = database.collection::<Document>(collection);
    if rows
        .find_one(doc! {"$or":arrays})
        .projection(doc! {"_id":1})
        .max_time(IO)
        .await
        .map_err(|_| "binding unique-key shape inspection unavailable")?
        .is_some()
    {
        return Err("binding unique-key arrays require a reviewed data migration");
    }
    let result = database
        .run_command(doc! {
            "aggregate":collection,
            "pipeline":[{"$group":{"_id":group,"count":{"$sum":1}}},
                {"$match":{"count":{"$gt":1}}}, {"$limit":1}, {"$project":{"_id":0,"count":1}}],
            "cursor":{"batchSize":2}, "allowDiskUse":false, "maxTimeMS":5000,
        })
        .await
        .map_err(|_| "binding duplicate preflight unavailable or exceeded bound")?;
    if !schema::entries(&result)?.is_empty() {
        return Err("binding duplicate keys require a reviewed data migration");
    }
    Ok(())
}

pub(super) fn plan(pending: usize) -> Value {
    json!({"migration":ID,"checksum":checksum(),"phase":"expand",
        "prerequisites":[],"missing_indexes":pending,"writes":false,
        "required_prior_state":{"existing_collections":bindings::COLLECTIONS,"collection_options":"default","document_schema_version":1,"duplicate_unique_keys":0},
        "effects":"add binding query and unique indexes; retain legacy constraints",
        "duplicate_policy":"refuse; never delete or rewrite records",
        "concurrent_writes":"unique index commit validates concurrent keys; conflict requires inspection and retry",
        "creates_collections":false,"retention_changes":false,"drops_constraints":false})
}

pub(super) async fn create(database: &Database, collection: &str, index: Document) -> Result<bool> {
    require_collections(database).await?;
    if !bindings::indexes(collection).iter().any(|expected| {
        expected == &index
            && expected
                .get_document("key")
                .ok()
                .zip(index.get_document("key").ok())
                .is_some_and(|(left, right)| left.iter().eq(right.iter()))
    }) {
        return Err("unreviewed binding index request");
    }
    let existing = schema::inspect_collection(database, collection)
        .await?
        .ok_or("binding collection prior state drift")?;
    if bindings::missing(collection, &existing)?.is_empty() {
        return Ok(false);
    }
    let result = database
        .run_command(doc! {"createIndexes":collection,"indexes":[index],
        "maxTimeMS":10000,"writeConcern":{"w":"majority","wtimeout":5000}})
        .await
        .map_err(|_| "binding index outcome uncertain; inspect and resume")?;
    if result.contains_key("writeConcernError") {
        return Err("binding index acknowledgement uncertain; inspect and resume");
    }
    let count = |field| {
        result
            .get_i64(field)
            .or_else(|_| result.get_i32(field).map(i64::from))
            .map_err(|_| "binding index result malformed")
    };
    let before = count("numIndexesBefore")?;
    let after = count("numIndexesAfter")?;
    if before < 0
        || before
            .checked_add(1)
            .is_none_or(|maximum| !(before..=maximum).contains(&after))
    {
        return Err("binding index count inconsistent");
    }
    Ok(after > before)
}
