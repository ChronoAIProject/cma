use super::super::{activity_plan, Result};
use super::{open_database, COLLECTION};
use mongodb::{
    bson::{doc, Document},
    Database,
};
use serde_json::{json, Value};

pub(super) async fn size() -> Result<Value> {
    let (_client, database, mode) = open_database().await?;
    let status = database
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
        .await
        .map_err(|_| "database authority unavailable")?;
    crate::store::isolation::verify_activity_index_authority(&status, database.name(), mode, false)
        .map_err(|_| "Activity sizing authority is insufficient")?;
    let stats = database
        .run_command(doc! {"collStats":COLLECTION,"maxTimeMS":5000_i32})
        .await
        .map_err(|_| "Activity collection metadata unavailable")?;
    let indexes = database
        .run_command(doc! {"listIndexes":COLLECTION,"cursor":{"batchSize":64},"maxTimeMS":5000_i32})
        .await
        .map_err(|_| "Activity index inventory unavailable")?;
    let explain = explain_first_page(&database).await?;
    Ok(json!({
        "schema":"activity-index-sizing-v1",
        "collection":COLLECTION,
        "read_only":true,
        "server_bound_ms":5000,
        "metadata": {
            "document_count": number(&stats, "count")?,
            "logical_bytes": number(&stats, "size")?,
            "storage_bytes": number(&stats, "storageSize")?,
            "index_bytes": number(&stats, "totalIndexSize")?,
        },
        "indexes": sanitize_indexes(&indexes)?,
        "first_page_explain": explain,
    }))
}

async fn explain_first_page(database: &Database) -> Result<Value> {
    let query = doc! {
        "find":COLLECTION,
        "filter":{},
        "sort":{"recorded_at":-1,"_id":-1},
        "limit":51_i64,
        "batchSize":52_i32,
        "maxTimeMS":5000_i32,
        "projection":{
            "_id":1,"document_schema_version":1,"recorded_at":1,
            "record.id":1,"record.at":1,"record.actor":1,"record.action":1,
            "record.target":1,"record.outcome":1,
        },
    };
    let explained = database
        .run_command(doc! {"explain":query,"verbosity":"executionStats","maxTimeMS":5000_i32})
        .await
        .map_err(|_| "Activity first-page explain unavailable")?;
    let planner = explained
        .get_document("queryPlanner")
        .map_err(|_| "Activity explain planner malformed")?;
    let stats = explained
        .get_document("executionStats")
        .map_err(|_| "Activity explain execution stats malformed")?;
    let plan = planner
        .get_document("winningPlan")
        .map_err(|_| "Activity explain winning plan malformed")?;
    Ok(json!({
        "winning_plan": activity_plan::sanitize(plan),
        "execution_time_ms": number(stats, "executionTimeMillis")?,
        "returned": number(stats, "nReturned")?,
        "keys_examined": number(stats, "totalKeysExamined")?,
        "documents_examined": number(stats, "totalDocsExamined")?,
    }))
}

fn sanitize_indexes(response: &Document) -> Result<Vec<Value>> {
    let cursor = response
        .get_document("cursor")
        .map_err(|_| "Activity index cursor malformed")?;
    if cursor_id(cursor)? != 0 {
        return Err("Activity index inventory exceeds bound");
    }
    cursor
        .get_array("firstBatch")
        .map_err(|_| "Activity index batch malformed")?
        .iter()
        .map(|value| {
            let index = value
                .as_document()
                .ok_or("Activity index entry malformed")?;
            let name = index
                .get_str("name")
                .map_err(|_| "Activity index name malformed")?;
            let key = index
                .get_document("key")
                .map_err(|_| "Activity index key malformed")?;
            Ok(json!({
                "name":name,
                "key":serde_json::to_value(key).map_err(|_| "Activity index key encoding")?,
                "unique":index.get_bool("unique").unwrap_or(false),
                "sparse":index.get_bool("sparse").unwrap_or(false),
                "hidden":index.get_bool("hidden").unwrap_or(false),
                "has_partial_filter":index.contains_key("partialFilterExpression"),
                "has_collation":index.contains_key("collation"),
            }))
        })
        .collect()
}

fn cursor_id(cursor: &Document) -> Result<i64> {
    cursor
        .get_i64("id")
        .or_else(|_| cursor.get_i32("id").map(i64::from))
        .map_err(|_| "Activity index cursor id malformed")
}

pub(super) fn number(document: &Document, key: &str) -> Result<i64> {
    document
        .get_i64(key)
        .or_else(|_| document.get_i32(key).map(i64::from))
        .map_err(|_| "Activity sizing number malformed")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn size_output_helpers_reject_unbounded_index_cursors() {
        assert!(sanitize_indexes(&doc! {"cursor":{"id":1_i64,"firstBatch":[]}}).is_err());
        assert_eq!(cursor_id(&doc! {"id":0_i32}).unwrap(), 0);
    }
}
