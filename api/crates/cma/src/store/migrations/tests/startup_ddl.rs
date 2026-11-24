use super::*;
use crate::store::bindings::schema::indexes_v1 as bindings;
use mongodb::bson::Bson;
use std::cell::Cell;
use tokio::sync::oneshot;

const RECORDED_BINDING_CHECKSUM: &str =
    "0933c5ce3a6bd7688c4573024daa35db28e3d486614f11a41d96d2203ef51867";

#[test]
fn binding_manifest_keeps_its_recorded_identity() {
    assert_eq!(binding_indexes::checksum(), RECORDED_BINDING_CHECKSUM);
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn binding_recorded_manifest_survives_runtime_policy_and_refuses_checksum_drift() {
    let (client, database, _) = fixture().await;
    let rows = database.collection::<Document>(ledger::COLLECTION);
    rows.insert_one(doc! {
        "_id":"manifest:am-binding-indexes-v1", "schema":1_i64, "kind":"manifest",
        "migration":"am-binding-indexes-v1", "checksum":RECORDED_BINDING_CHECKSUM,
        "completed":false,
    })
    .await
    .unwrap();
    let ledger = ledger::Ledger::for_step(client, database.clone(), Step::BindingIndexes);
    for mode in [
        crate::config::StartupSchemaMode::Observe,
        crate::config::StartupSchemaMode::Enforce,
    ] {
        let mut observer = crate::store::schema::StartupObserver::new(mode);
        let _ = observer
            .assess(crate::store::schema::Contract::Binding, async {
                Ok(crate::store::schema::Assessment::<()> {
                    capability: (),
                    findings: [crate::store::schema::SchemaFinding::MissingCollection].into(),
                })
            })
            .await;
        ledger.preflight().await.unwrap();
        assert_eq!(
            ledger.status().await.unwrap()["checksum"],
            RECORDED_BINDING_CHECKSUM
        );
    }
    rows.update_one(
        doc! {"kind":"manifest"},
        doc! {"$set":{"checksum":"changed-contract"}},
    )
    .await
    .unwrap();
    assert_eq!(
        ledger.preflight().await.unwrap_err(),
        "unknown migration history; older migrator refused"
    );
    assert!(ledger.claim(&identity()).await.is_err());
    assert_eq!(
        database.list_collection_names().await.unwrap(),
        [ledger::COLLECTION]
    );
    database.drop().await.unwrap();
}

async fn fixture_collections(database: &mongodb::Database) {
    // Explicit disposable setup; no production bootstrap executor is shipped.
    for &collection in bindings::COLLECTIONS {
        database.create_collection(collection).await.unwrap();
    }
}

pub(in crate::store) async fn prepare_bindings_for_test(database: &mongodb::Database) {
    fixture_collections(database).await;
    apply_binding_indexes_for_test(database).await.unwrap();
}

pub(in crate::store) async fn apply_binding_indexes_for_test(
    database: &mongodb::Database,
) -> Result<serde_json::Value> {
    let ledger = ledger::Ledger::for_step(
        database.client().clone(),
        database.clone(),
        Step::BindingIndexes,
    );
    apply_indexes(&ledger, database, &identity()).await
}

type BindingIndexPause = (oneshot::Sender<()>, oneshot::Receiver<()>);

tokio::task_local! {
    static BINDING_INDEX_PAUSE: Cell<Option<BindingIndexPause>>;
}

pub(in crate::store) async fn apply_binding_indexes_with_pause_for_test(
    database: &mongodb::Database,
    created: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
) -> Result<serde_json::Value> {
    BINDING_INDEX_PAUSE
        .scope(
            Cell::new(Some((created, resume))),
            apply_binding_indexes_for_test(database),
        )
        .await
}

pub(in crate::store::migrations) async fn pause_after_binding_index() -> Result<()> {
    if let Some((created, resume)) = BINDING_INDEX_PAUSE.try_with(Cell::take).ok().flatten() {
        created.send(()).map_err(|_| "test dropped index receipt")?;
        tokio::time::timeout(Duration::from_secs(20), resume)
            .await
            .map_err(|_| "test did not resume binding migration")?
            .map_err(|_| "test dropped binding migration release")?;
    }
    Ok(())
}

#[test]
fn binding_contract_rejects_index_order_and_options_drift() {
    for &collection in bindings::COLLECTIONS {
        let indexes = bindings::indexes(collection);
        assert_eq!(bindings::missing(collection, &[]).unwrap(), indexes);
        assert!(bindings::missing(collection, &indexes).unwrap().is_empty());
        for (position, index) in indexes.iter().enumerate() {
            let mutations = [
                ("name", Bson::String("unexpected-name".into())),
                ("key", doc! {"unexpected":1}.into()),
                (
                    "unique",
                    (!index.get_bool("unique").unwrap_or(false)).into(),
                ),
                ("sparse", true.into()),
                ("hidden", true.into()),
                ("partialFilterExpression", doc! {"present":true}.into()),
                ("collation", doc! {"locale":"en"}.into()),
                ("expireAfterSeconds", 1_i64.into()),
            ];
            for (key, value) in mutations {
                let mut changed = indexes.clone();
                changed[position].insert(key, value);
                assert!(
                    bindings::missing(collection, &changed).is_err(),
                    "{collection}: {key}"
                );
            }
            let keys = index.get_document("key").unwrap();
            if keys.len() > 1 {
                let mut changed = indexes.clone();
                changed[position].insert(
                    "key",
                    keys.iter()
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<Document>(),
                );
                assert!(bindings::missing(collection, &changed).is_err());
            }
        }
    }
    let mut existing = bindings::indexes("binding_conversations");
    existing
        .push(doc! {"name":bindings::LEGACY_AGENT_INDEX,"key":{"record.agent_id":1},"unique":true});
    assert!(bindings::missing("binding_conversations", &existing)
        .unwrap()
        .is_empty());
}

#[test]
fn binding_plan_exposes_prior_state_and_never_authorizes_bootstrap_or_retirement() {
    let plan = binding_indexes::plan(11);
    assert_eq!(plan["prerequisites"], json!([]));
    assert_eq!(
        plan["required_prior_state"]["existing_collections"],
        json!(bindings::COLLECTIONS)
    );
    for field in [
        "writes",
        "retention_changes",
        "drops_constraints",
        "creates_collections",
    ] {
        assert_eq!(plan[field], false);
    }
    let step = Step::BindingIndexes;
    assert_eq!(Step::parse(step.id()).unwrap(), step);
    let role = authority::role_for("fixture", true, step);
    let grants = role.get_array("privileges").unwrap().clone();
    let status = doc! {"authInfo":{"authenticatedUsers":[{"user":"fixture","db":"fixture"}],
    "authenticatedUserPrivileges":grants.clone()}};
    for mode in [
        MongoAuthorityMode::Isolated,
        MongoAuthorityMode::SharedSchema,
    ] {
        assert!(authority::verify_for(&status, "fixture", mode, true, step).is_ok());
    }
    for grant in grants {
        let grant = grant.as_document().unwrap();
        let actions = grant.get_array("actions").unwrap();
        for forbidden in ["collMod", "dropIndex", "dropCollection", "remove"] {
            assert!(!actions.contains(&forbidden.into()));
        }
        if grant
            .get_document("resource")
            .unwrap()
            .get_str("collection")
            .unwrap()
            != ledger::COLLECTION
        {
            assert!(!actions.contains(&"createCollection".into()));
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn binding_plan_is_read_only_and_requires_live_prior_state() {
    let (client, database, _) = fixture().await;
    let ledger = ledger::Ledger::for_step(client, database.clone(), Step::BindingIndexes);
    ledger.status().await.unwrap();
    assert_eq!(
        Step::BindingIndexes.inspect(&database).await.unwrap_err(),
        "binding collections require separately approved bootstrap"
    );
    assert!(apply_indexes(&ledger, &database, &identity())
        .await
        .is_err());
    assert!(database.list_collection_names().await.unwrap().is_empty());
    fixture_collections(&database).await;
    assert_eq!(
        Step::BindingIndexes.inspect(&database).await.unwrap().len(),
        11
    );
    assert_eq!(database.list_collection_names().await.unwrap().len(), 5);
    database
        .collection::<Document>("provider_tool_opens")
        .drop()
        .await
        .unwrap();
    assert!(apply_indexes(&ledger, &database, &identity())
        .await
        .is_err());
    assert!(!ledger.completed().await.unwrap());
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn binding_partial_apply_resumes_preserves_legacy_and_checks_completed_drift() {
    let (client, database, _) = fixture().await;
    fixture_collections(&database).await;
    let legacy =
        doc! {"name":bindings::LEGACY_AGENT_INDEX,"key":{"record.agent_id":1},"unique":true};
    database
        .run_command(doc! {"createIndexes":"binding_conversations","indexes":[legacy]})
        .await
        .unwrap();
    let retained = doc! {"_id":"retained","document_schema_version":1_i64,
    "binding_id":"binding-one","generation":1_i64,"record":{"agent_id":"agent-one"}};
    database
        .collection("binding_conversations")
        .insert_one(retained.clone())
        .await
        .unwrap();
    let ledger = ledger::Ledger::for_step(client, database.clone(), Step::BindingIndexes);
    ledger.initialize().await.unwrap();
    let abandoned = ledger.claim(&identity()).await.unwrap();
    let (collection, index) = Step::BindingIndexes
        .inspect(&database)
        .await
        .unwrap()
        .remove(0);
    assert!(Step::BindingIndexes
        .create(&database, collection, index)
        .await
        .unwrap());
    expire(&database).await;
    let result = apply_indexes(&ledger, &database, &identity())
        .await
        .unwrap();
    assert_eq!(result["created_indexes"], 10);
    assert!(ledger.finish(&abandoned, Ok(1)).await.is_err());
    assert_eq!(
        apply_indexes(&ledger, &database, &identity())
            .await
            .unwrap()["outcome"],
        "already_completed"
    );
    assert_eq!(
        database
            .collection::<Document>("binding_conversations")
            .find_one(doc! {})
            .await
            .unwrap(),
        Some(retained)
    );
    let existing = crate::store::schema::inspect_collection(&database, "binding_conversations")
        .await
        .unwrap()
        .unwrap();
    assert!(existing
        .iter()
        .any(|index| index.get_str("name").ok() == Some(bindings::LEGACY_AGENT_INDEX)));
    assert_eq!(
        ledger.status().await.unwrap()["recent_attempts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    database
        .collection::<Document>("provider_bindings")
        .drop_index("pending_1__id_1")
        .await
        .unwrap();
    assert_eq!(
        apply_indexes(&ledger, &database, &identity())
            .await
            .unwrap_err(),
        "completed migration postcondition drift"
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn binding_duplicates_refuse_before_any_index_or_record_mutation() {
    let (client, database, _) = fixture().await;
    fixture_collections(&database).await;
    let rows = database.collection::<Document>("provider_bindings");
    let saved = [
        doc! {"_id":"one","document_schema_version":1_i64,"record":{"key_sha256":"duplicate"}},
        doc! {"_id":"two","document_schema_version":1_i64,"record":{"key_sha256":"duplicate"}},
    ];
    rows.insert_many(saved.clone()).await.unwrap();
    let indexes = ledger::Ledger::for_step(client, database.clone(), Step::BindingIndexes);
    assert_eq!(
        apply_indexes(&indexes, &database, &identity())
            .await
            .unwrap_err(),
        "binding duplicate keys require a reviewed data migration"
    );
    assert_eq!(
        crate::store::schema::inspect_collection(&database, "provider_bindings")
            .await
            .unwrap()
            .unwrap()
            .len(),
        1
    );
    for row in saved {
        assert_eq!(
            rows.find_one(doc! {"_id":row.get("_id").unwrap().clone()})
                .await
                .unwrap(),
            Some(row)
        );
    }
    assert!(!indexes.completed().await.unwrap());
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn binding_plan_refuses_unreviewed_validator_without_writes() {
    let (client, database, _) = fixture().await;
    database
        .create_collection("provider_bindings")
        .validator(doc! {"unexpected":{"$exists":true}})
        .await
        .unwrap();
    let ledger = ledger::Ledger::for_step(client, database.clone(), Step::BindingIndexes);
    assert!(Step::BindingIndexes.inspect(&database).await.is_err());
    assert!(apply_indexes(&ledger, &database, &identity())
        .await
        .is_err());
    assert_eq!(
        database.list_collection_names().await.unwrap(),
        ["provider_bindings"]
    );
    database.drop().await.unwrap();
}
