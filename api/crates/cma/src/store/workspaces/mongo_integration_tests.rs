use super::*;
use layer2_core::StoreError;
use mongodb::{bson::Bson, options::IndexOptions, Client, Database, IndexModel};

const INDEX: &str = "workspaces_sandbox";

async fn database() -> (Client, Database) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a disposable replica set");
    let client = Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!(
        "cma_workspace_index_{}",
        uuid::Uuid::new_v4().simple()
    ));
    (client, database)
}

// Only persisted query fields matter to this index regression. Include every
// origin and removal state so a partial index cannot silently hide children.
fn child(sandbox: &str, origin: &str, removed: bool) -> Document {
    let id = format!("{sandbox}-{origin}-{removed}");
    doc! {
        "_id": &id,
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "sandbox_id": sandbox,
        "name": origin,
        "origin_type": origin,
        "origin_index": i64::from(removed),
        "origin_launch_id": &id,
        "origin_operation_id": &id,
        "removed_at": if removed { Bson::String("2026-11-05T00:00:00Z".into()) } else { Bson::Null },
    }
}

async fn indexes(collection: &Collection<Document>) -> Vec<Document> {
    let mut indexes: Vec<Document> = collection
        .list_indexes()
        .await
        .unwrap()
        .map_ok(|index| mongodb::bson::serialize_to_document(&index).unwrap())
        .try_collect()
        .await
        .unwrap();
    indexes.sort_by(|a, b| a.get_str("name").unwrap().cmp(b.get_str("name").unwrap()));
    indexes
}

async fn explain(database: &Database, sandbox: &str) -> Document {
    database
        .run_command(doc! {
            "explain": {
                "find": "workspaces",
                // Exactly the complete-child predicate used by maintenance;
                // no hint or extra predicate that makes a partial index eligible.
                "filter": { "sandbox_id": sandbox },
            },
            "verbosity": "executionStats",
        })
        .await
        .unwrap()
}

fn plan_contains(value: &Bson, stage: &str, index: Option<&str>) -> bool {
    match value {
        Bson::Document(document) => {
            (document.get_str("stage").ok() == Some(stage)
                && index.is_none_or(|name| document.get_str("indexName").ok() == Some(name)))
                || document
                    .values()
                    .any(|value| plan_contains(value, stage, index))
        }
        Bson::Array(values) => values
            .iter()
            .any(|value| plan_contains(value, stage, index)),
        _ => false,
    }
}

fn winning_plan(explain: &Document) -> &Bson {
    explain
        .get_document("queryPlanner")
        .unwrap()
        .get("winningPlan")
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn startup_indexes_complete_children_and_reopens_idempotently() {
    let (client, database) = database().await;
    let collection = database.collection::<Document>("workspaces");
    // Model an existing deployment with only the previous partial/due indexes.
    collection
        .create_indexes(workspace_indexes().into_iter().filter(|index| {
            index
                .options
                .as_ref()
                .and_then(|options| options.name.as_deref())
                != Some(INDEX)
        }))
        .await
        .unwrap();
    let previous_indexes = indexes(&collection).await;
    let expected: Vec<_> = ["manual", "sandbox_profile", "trigger", "lifecycle"]
        .into_iter()
        .flat_map(|origin| [false, true].map(|removed| child("sbx_target", origin, removed)))
        .collect();
    let unrelated = (0..341).map(|id| child(&format!("sbx_other_{id}"), "manual", false));
    collection
        .insert_many(expected.iter().cloned().chain(unrelated))
        .await
        .unwrap();
    let before = explain(&database, "sbx_target").await;
    assert!(plan_contains(winning_plan(&before), "COLLSCAN", None));

    let store = MongoSandboxStore::open(&client, database.name())
        .await
        .unwrap();
    let installed = indexes(&store.workspaces).await;
    let complete = installed
        .iter()
        .find(|index| index.get_str("name").ok() == Some(INDEX))
        .unwrap();
    assert_eq!(
        complete.get_document("key").unwrap(),
        &doc! { "sandbox_id": 1 }
    );
    assert!(!complete.contains_key("partialFilterExpression"));
    assert!(!complete.get_bool("unique").unwrap_or(false));
    assert!(!complete.get_bool("sparse").unwrap_or(false));
    assert_eq!(installed.len(), previous_indexes.len() + 1);
    for index in previous_indexes {
        assert!(
            installed.contains(&index),
            "startup must retain existing indexes"
        );
    }
    let reopened = MongoSandboxStore::open(&client, database.name())
        .await
        .unwrap();
    assert_eq!(indexes(&reopened.workspaces).await, installed);

    let actual: Vec<Document> = reopened
        .workspaces
        .find(doc! { "sandbox_id": "sbx_target" })
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(actual.len(), expected.len());
    for child in &expected {
        assert!(
            actual.contains(child),
            "complete-child reads must retain every origin/removal state"
        );
    }
    for (sandbox, count) in [("sbx_target", expected.len() as i32), ("sbx_missing", 0)] {
        let explained = explain(&database, sandbox).await;
        let plan = winning_plan(&explained);
        assert!(plan_contains(plan, "IXSCAN", Some(INDEX)), "{plan:?}");
        assert!(!plan_contains(plan, "COLLSCAN", None), "{plan:?}");
        let stats = explained.get_document("executionStats").unwrap();
        assert_eq!(stats.get_i32("nReturned").unwrap(), count);
        assert_eq!(stats.get_i32("totalDocsExamined").unwrap(), count);
    }
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn startup_refuses_conflicting_workspace_index_without_replacing_it() {
    let (client, database) = database().await;
    let collection = database.collection::<Document>("workspaces");
    collection
        .create_index(
            IndexModel::builder()
                .keys(doc! { "sandbox_id": 1 })
                .options(
                    IndexOptions::builder()
                        .name(INDEX.to_string())
                        .partial_filter_expression(doc! { "removed_at": Bson::Null })
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    let original = indexes(&collection).await;
    let retained = child("sbx_retained", "manual", true);
    collection.insert_one(retained.clone()).await.unwrap();

    let result = MongoSandboxStore::open(&client, database.name()).await;
    assert!(matches!(result, Err(StoreError::Unavailable(message))
        if message.starts_with("workspace indexes:") && message.contains(INDEX)));
    let after = indexes(&collection).await;
    for index in original {
        assert!(
            after.contains(&index),
            "failed startup must not replace an index"
        );
    }
    assert_eq!(collection.find_one(doc! {}).await.unwrap(), Some(retained));
    database.drop().await.unwrap();
}
