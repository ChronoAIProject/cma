//! Operation-only effective-grant checks using the serving app credential.
use super::super::Result;
use super::{catalog::Query, open_database};
use cma_service_auth::mongodb_authority::{verify_shared_schema, MongoAuthorityMode};
use mongodb::{
    bson::{doc, Document},
    Database,
};
use serde_json::{json, Value};

async fn status(database: &Database) -> Result<Document> {
    database
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true,"maxTimeMS":5000})
        .await
        .map_err(|_| "index operation authority unavailable")
}

pub(super) async fn verify(
    database: &Database,
    mode: MongoAuthorityMode,
    collections: &[&str],
    build: bool,
) -> Result<()> {
    verify_effective(
        &status(database).await?,
        database.name(),
        mode,
        collections,
        build,
    )
}

pub(super) fn verify_effective(
    status: &Document,
    database: &str,
    mode: MongoAuthorityMode,
    collections: &[&str],
    build: bool,
) -> Result<()> {
    crate::store::isolation::verify_index_operation_authority(
        status,
        database,
        mode,
        collections,
        build,
    )
    .map_err(|_| "index operation authority is insufficient")?;
    // Admission must prove the approved abort can run with this same credential.
    // listIndexes success is not evidence of either DDL privilege.
    if build
        && collections.iter().any(|collection| {
            !has_actions(status, database, collection, &["createIndex", "dropIndex"])
        })
    {
        return Err("index operation requires effective createIndex and dropIndex grants");
    }
    Ok(())
}

fn has_actions(status: &Document, database: &str, collection: &str, actions: &[&str]) -> bool {
    let required = doc! {"privileges":[{"resource":{"db":database,"collection":collection},"actions":actions}]};
    verify_shared_schema(status, database, &required).is_ok()
}

pub(super) async fn preflight(queries: &[Query]) -> Result<Value> {
    let (_, database, mode) = open_database().await?;
    Ok(preflight_report(
        &status(&database).await?,
        database.name(),
        mode,
        queries,
    ))
}

pub(super) fn preflight_report(
    status: &Document,
    database: &str,
    mode: MongoAuthorityMode,
    queries: &[Query],
) -> Value {
    let checks = queries.iter().map(|query| {
        let create = has_actions(status, database, query.collection, &["createIndex"]);
        let drop = has_actions(status, database, query.collection, &["dropIndex"]);
        let admitted = verify_effective(status, database, mode, &[query.collection], true).is_ok();
        json!({"query":query.name,"collection":query.collection,"create_index":create,
            "drop_index":drop,"operation_authority":admitted,"build_supported":query.step.is_some()})
    }).collect::<Vec<_>>();
    let blocked = checks
        .iter()
        .filter(|check| check["operation_authority"] != true)
        .map(|check| check["collection"].clone())
        .collect::<Vec<_>>();
    json!({"schema":"index-toolkit-preflight-v1","read_only":true,"checks":checks,
        "preflight_passed":!queries.is_empty() && blocked.is_empty(),"blocked_collections":blocked})
}
