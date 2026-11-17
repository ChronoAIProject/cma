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
                r["inspection_only"] == false
                    && r["index_present"] == true
                    && r["first_page_explain"]["uses_expected_index"] == true
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
    let explain = if let Some(filter) = super::binding::resolve(database, query).await? {
        let mut explain = explain(database, query, &filter).await?;
        explain["filter_binding"] = query.qualification.into();
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
        "read_only":true,"inspection_only":query.step.is_none(),"qualification":query.qualification,"server_bound_ms":5000,"index_present":pending.is_empty(),
        "metadata":{"document_count":number(&stats,"count")?,"logical_bytes":number(&stats,"size")?,
            "storage_bytes":number(&stats,"storageSize")?,"index_bytes":number(&stats,"totalIndexSize")?},
        "indexes":sanitize_indexes(&indexes,query)?,"first_page_explain":explain}),
    )
}

pub(super) fn find(query: &Query, filter: &Document) -> Document {
    doc! {"find":query.collection,"filter":filter.clone(),"sort":query.sort.clone(),
    "projection":query.projection.clone(),"limit":query.limit,"batchSize":query.limit+1,"maxTimeMS":5000}
}

async fn explain(database: &Database, query: &Query, filter: &Document) -> Result<Value> {
    let response = database
        .run_command(
            doc! {"explain":find(query,filter),"verbosity":"executionStats","maxTimeMS":5000},
        )
        .await
        .map_err(|_| "query explain unavailable")?;
    project_explain(query, &response)
}

fn project_explain(query: &Query, response: &Document) -> Result<Value> {
    let planner = response
        .get_document("queryPlanner")
        .map_err(|_| "query planner malformed")?;
    let stats = response
        .get_document("executionStats")
        .map_err(|_| "query execution stats malformed")?;
    // A successful explain command can still contain failed execution and
    // partial counters. Never accept its plan or forward the server error text.
    if stats.get_bool("executionSuccess").ok() != Some(true) {
        return Err("query explain execution did not succeed");
    }
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
    fn explain_requires_successful_execution_without_exposing_error_text() {
        use mongodb::bson::Bson;
        let query = super::super::catalog::activity();
        let response = doc! {
            "queryPlanner":{"winningPlan":{"stage":"IXSCAN","indexName":query.index_name()}},
            "executionStats":{"executionSuccess":true,"executionTimeMillis":7,"nReturned":51,
                "totalKeysExamined":51,"totalDocsExamined":51,"errorMessage":"private-error-canary"}
        };
        let output = project_explain(&query, &response).unwrap();
        assert_eq!(output["uses_expected_index"], true);
        assert_eq!(output["execution_time_ms"], 7);
        assert_eq!(output["returned"], 51);
        assert!(!output.to_string().contains("private-error-canary"));
        for success in [
            None,
            Some(Bson::Boolean(false)),
            Some(Bson::Null),
            Some(Bson::Int32(1)),
            Some(Bson::String("true".into())),
        ] {
            let mut failed = response.clone();
            let stats = failed.get_document_mut("executionStats").unwrap();
            stats.remove("executionSuccess");
            if let Some(success) = success {
                stats.insert("executionSuccess", success);
            }
            assert_eq!(
                project_explain(&query, &failed).unwrap_err(),
                "query explain execution did not succeed"
            );
        }
    }

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
