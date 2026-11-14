use super::super::{index_inventory, index_plan, Result};
use super::{catalog::Query, open_database};
use mongodb::{
    bson::{doc, Document},
    Database,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(super) async fn inspect(queries: &[Query], verify: bool, legacy: bool) -> Result<Value> {
    let (_, database, mode) = open_database().await?;
    let collections = queries.iter().map(|q| q.collection).collect::<Vec<_>>();
    super::authority::verify(&database, mode, &collections, false).await?;
    let mut results = Vec::new();
    for query in queries {
        results.push(inspect_query(&database, query).await?);
    }
    if legacy {
        let mut result = results.remove(0);
        result["schema"] = "activity-index-sizing-v1".into();
        return Ok(result);
    }
    let mut result =
        json!({"schema":"index-toolkit-inspection-v1","read_only":true,"queries":results});
    if verify {
        result["verification_passed"] =
            Value::Bool(result["queries"].as_array().unwrap().iter().all(|r| {
                r["index_present"] == true && r["first_page_explain"]["uses_expected_index"] == true
            }));
    }
    Ok(result)
}

pub(super) async fn inspect_query(database: &Database, query: &Query) -> Result<Value> {
    let stats = database
        .run_command(doc! {"collStats":query.collection,"maxTimeMS":5000})
        .await
        .map_err(|_| "query collection metadata unavailable")?;
    let response = database
        .run_command(
            doc! {"listIndexes":query.collection,"cursor":{"batchSize":64},"maxTimeMS":5000},
        )
        .await
        .map_err(|_| "query index inventory unavailable")?;
    let indexes = index_inventory::entries(&response)?;
    let pending = index_inventory::strict_missing(&indexes, &query.index)?;
    let explain = if let Some(filter) = resolve_filter(database, query).await? {
        let mut explain = explain(database, query, &filter).await?;
        explain["filter_binding"] = if query.sample_equality.is_some() {
            "one sampled equality value; not all owner strata"
        } else {
            "static catalog filter"
        }
        .into();
        explain["filter_sha256"] = format!(
            "{:x}",
            Sha256::digest(
                mongodb::bson::serialize_to_vec(&filter)
                    .map_err(|_| "filter fingerprint unavailable")?
            )
        )
        .into();
        explain
    } else {
        json!({"available":false,"uses_expected_index":false,"reason":"no representative equality value; incomplete query evidence"})
    };
    Ok(
        json!({"schema":"index-query-sizing-v1","query":query.name,"collection":query.collection,
        "read_only":true,"server_bound_ms":5000,"index_present":pending.is_empty(),
        "metadata":{"document_count":number(&stats,"count")?,"logical_bytes":number(&stats,"size")?,
            "storage_bytes":number(&stats,"storageSize")?,"index_bytes":number(&stats,"totalIndexSize")?},
        "indexes":sanitize_indexes(&indexes,query)?,"first_page_explain":explain}),
    )
}

pub(super) fn find(query: &Query, filter: &Document) -> Document {
    doc! {"find":query.collection,"filter":filter.clone(),"sort":query.sort.clone(),
    "projection":query.projection.clone(),"limit":query.limit,"batchSize":query.limit+1,"maxTimeMS":5000}
}

async fn resolve_filter(database: &Database, query: &Query) -> Result<Option<Document>> {
    let Some(field) = query.sample_equality else {
        return Ok(Some(query.filter.clone()));
    };
    // Bind a real equality value internally. Never print a synthetic query as a
    // production plan or return this owner identifier in the inspection output.
    let mut predicate = Document::new();
    predicate.insert(field, doc! {"$type":"string"});
    let mut projection = doc! {"_id":0};
    projection.insert(field, 1);
    let response = database
        .run_command(
            doc! {"find":query.collection,"filter":predicate,"sort":{"_id":1},
            "projection":projection,"limit":1,"batchSize":1,"singleBatch":true,"maxTimeMS":5000},
        )
        .await
        .map_err(|_| "representative query value unavailable")?;
    let cursor = response
        .get_document("cursor")
        .map_err(|_| "query sample cursor malformed")?;
    if cursor.get_i64("id").ok() != Some(0) {
        return Err("query sample cursor exceeds bound");
    }
    let rows = cursor
        .get_array("firstBatch")
        .map_err(|_| "query sample rows malformed")?;
    if rows.len() > 1 {
        return Err("query sample exceeds bound");
    }
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let value = row
        .as_document()
        .and_then(|doc| doc.get_str(field).ok())
        .ok_or("query sample value malformed")?;
    if value.is_empty() || value.len() > 1024 {
        return Err("query sample value outside bound");
    }
    let mut filter = query.filter.clone();
    filter.insert(field, value);
    Ok(Some(filter))
}

async fn explain(database: &Database, query: &Query, filter: &Document) -> Result<Value> {
    let response = database
        .run_command(
            doc! {"explain":find(query,filter),"verbosity":"executionStats","maxTimeMS":5000},
        )
        .await
        .map_err(|_| "query explain unavailable")?;
    let planner = response
        .get_document("queryPlanner")
        .map_err(|_| "query planner malformed")?;
    let stats = response
        .get_document("executionStats")
        .map_err(|_| "query execution stats malformed")?;
    let plan = planner
        .get_document("winningPlan")
        .map_err(|_| "query winning plan malformed")?;
    let projected = index_plan::sanitize(plan, query.index_name(), &query.safe_fields());
    let passed = index_plan::verifies(&projected, query.index_name());
    Ok(
        json!({"winning_plan":projected,"uses_expected_index":passed,
        "execution_time_ms":number(stats,"executionTimeMillis")?,"returned":number(stats,"nReturned")?,
        "keys_examined":number(stats,"totalKeysExamined")?,"documents_examined":number(stats,"totalDocsExamined")?}),
    )
}

fn sanitize_indexes(indexes: &[Document], query: &Query) -> Result<Vec<Value>> {
    indexes.iter().map(|index|{
        let name=index.get_str("name").map_err(|_| "index name malformed")?;
        let key=index.get_document("key").map_err(|_| "index keys malformed")?;
        let known=name==query.index_name() || name=="_id_";
        let projected=index_plan::sanitize(&doc!{"stage":"IXSCAN","indexName":name,"keyPattern":key.clone()},query.index_name(),&query.safe_fields());
        Ok(json!({"name":if known {name} else {"<redacted>"},
            "name_sha256":format!("{:x}",Sha256::digest(name.as_bytes())),"key":projected["key_pattern"],
            "key_count":key.len(),"unique":index.get_bool("unique").unwrap_or(false),
            "sparse":index.get_bool("sparse").unwrap_or(false),"hidden":index.get_bool("hidden").unwrap_or(false),
            "has_partial_filter":index.contains_key("partialFilterExpression"),"has_collation":index.contains_key("collation")}))
    }).collect()
}

pub(super) fn number(document: &Document, key: &str) -> Result<i64> {
    document
        .get_i64(key)
        .or_else(|_| document.get_i32(key).map(i64::from))
        .map_err(|_| "index metadata number malformed")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commands_are_bounded_and_unknown_inventory_fields_do_not_escape() {
        let q = super::super::catalog::activity();
        let find = find(&q, &q.filter);
        assert_eq!(find.get_i32("maxTimeMS").unwrap(), 5000);
        assert!(!find.contains_key("hint"));
        let index = doc! {"name":"private-canary","key":{"secret-key":1},"partialFilterExpression":{"private":"canary"}};
        let output = sanitize_indexes(&[index], &q).unwrap();
        assert!(!serde_json::to_string(&output)
            .unwrap()
            .contains("private-canary"));
        assert!(!serde_json::to_string(&output)
            .unwrap()
            .contains("secret-key"));
        assert!(sanitize_indexes(&[doc! {"name":7}], &q).is_err());
        assert!(index_inventory::entries(&doc! {"cursor":{"id":1_i64,"firstBatch":[]}}).is_err());
    }
}
