//! Explain selector keysets through protected pages and unrelated tenants.
use super::{tests::*, *};
use mongodb::{
    bson::{doc, Bson, Document},
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Arc, Mutex};

fn number(row: &Document, key: &str) -> i64 {
    row.get_i64(key)
        .or_else(|_| row.get_i32(key).map(i64::from))
        .unwrap()
}
fn ixscan(value: &Bson) -> bool {
    match value {
        Bson::Document(row) => {
            row.get_str("stage").is_ok_and(|stage| stage == "IXSCAN") || row.values().any(ixscan)
        }
        Bson::Array(values) => values.iter().any(ixscan),
        _ => false,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn protected_indexed_pages_examine_only_the_matching_personal_range() {
    let observed = Arc::new(Mutex::new(Vec::<Document>::new()));
    let captured = observed.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "find"
                && event.command.get_str("find").ok() == Some("agent_log")
            {
                captured.lock().unwrap().push(event.command);
            }
        }
    }));
    let db = Client::with_options(options)
        .unwrap()
        .database(&format!("cma_keyset_{}", uuid::Uuid::new_v4().simple()));
    let logs = db.collection::<Document>("agent_log");
    logs.insert_many((0..1101).map(|n| doc! {
        "_id":format!("a-protected-{n:05}"),"agent_id":"owned-agent","sandbox_id":"organization-root","seq":n
    })).await.unwrap();
    logs.insert_many(
        (0..3).map(|n| doc! {"_id":format!("z-delete-{n}"),"agent_id":"owned-agent","seq":1101+n}),
    )
    .await
    .unwrap();
    logs.insert_many(
        (0..20000)
            .map(|n| doc! {"_id":format!("foreign-{n:05}"),"agent_id":"foreign-agent","seq":n}),
    )
    .await
    .unwrap();
    indexes::ensure(&db).await.unwrap();
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory {
        agents: vec!["owned-agent".into()],
        ..Default::default()
    };
    let mut contexts = context::load_cohort(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
    )
    .await
    .unwrap();
    contexts[0].protected.insert("organization-root".into());
    let entry = catalog::ERASURE
        .iter()
        .find(|entry| entry.collection == "agent_log")
        .unwrap();
    observed.lock().unwrap().clear();
    let mut selector = None;
    let mut after = None;
    let mut seen = BTreeSet::new();
    let mut protected = 0;
    let mut deletable = vec![];
    for _ in 0..10 {
        let page = indexed::page(&db, entry, &contexts, selector.as_ref(), after.as_ref())
            .await
            .unwrap();
        for row in &page.rows {
            assert!(
                seen.insert(row.get_str("_id").unwrap().to_owned()),
                "a protected page repeated"
            );
            if contexts[0].protects(row) {
                protected += 1;
            } else {
                deletable.push(row.get_str("_id").unwrap().to_owned());
            }
        }
        // Exercise the persisted representation between unchanged pages.
        selector = serde_json::from_slice(&serde_json::to_vec(&page.selector).unwrap()).unwrap();
        after = page.after;
        if page.complete {
            break;
        }
    }
    assert_eq!(protected, 1101);
    assert_eq!(deletable.len(), 3);
    assert_eq!(seen.len(), 1104);
    let queries = std::mem::take(&mut *observed.lock().unwrap());
    assert_eq!(queries.len(), 3);
    let (mut keys, mut docs) = (0, 0);
    for command in queries {
        let query: Document = ["find", "filter", "sort", "hint", "limit"]
            .into_iter()
            .filter_map(|key| {
                command
                    .get(key)
                    .map(|value| (key.to_owned(), value.clone()))
            })
            .collect();
        let plan = db
            .run_command(doc! {"explain":query,"verbosity":"executionStats"})
            .await
            .unwrap();
        let winning = plan
            .get_document("queryPlanner")
            .unwrap()
            .get("winningPlan")
            .unwrap();
        assert!(ixscan(winning), "{plan:?}");
        assert!(!format!("{winning:?}").contains("\"SORT\""), "{plan:?}");
        let stats = plan.get_document("executionStats").unwrap();
        assert!(number(stats, "totalKeysExamined") <= 501, "{plan:?}");
        assert!(number(stats, "totalDocsExamined") <= 500, "{plan:?}");
        keys += number(stats, "totalKeysExamined");
        docs += number(stats, "totalDocsExamined");
    }
    assert!(keys <= 1107, "unrelated tenant keys were scanned: {keys}");
    assert_eq!(docs, 1104);
    logs.delete_many(doc! {"_id":{"$in":&deletable}})
        .await
        .unwrap();
    assert_eq!(
        logs.count_documents(doc! {"agent_id":"owned-agent"})
            .await
            .unwrap(),
        1101
    );
    assert_eq!(
        logs.count_documents(doc! {"agent_id":"foreign-agent"})
            .await
            .unwrap(),
        20000
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_disappearing_selector_does_not_skip_the_next_resource_keyset() {
    let db = database().await;
    indexes::ensure(&db).await.unwrap();
    let logs = db.collection::<Document>("agent_log");
    logs.insert_many((0..501).map(|n| doc! {"_id":format!("a-{n:04}"),"agent_id":"a-agent"}))
        .await
        .unwrap();
    put(&db, "agent_log", doc! {"_id":"b-only","agent_id":"b-agent"}).await;
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory {
        agents: vec!["a-agent".into(), "b-agent".into()],
        ..Default::default()
    };
    let mut contexts = context::load_cohort(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
    )
    .await
    .unwrap();
    let entry = catalog::ERASURE
        .iter()
        .find(|entry| entry.collection == "agent_log")
        .unwrap();
    let first = indexed::page(&db, entry, &contexts, None, None)
        .await
        .unwrap();
    assert_eq!(first.rows.len(), 500);
    contexts[0]
        .sets
        .get_mut(&catalog::InventorySet::Agents)
        .unwrap()
        .remove("a-agent");
    let second = indexed::page(
        &db,
        entry,
        &contexts,
        first.selector.as_ref(),
        first.after.as_ref(),
    )
    .await
    .unwrap();
    assert!(second.complete);
    assert_eq!(second.rows.len(), 1);
    assert_eq!(second.rows[0].get_str("_id").unwrap(), "b-only");
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn indexed_history_crosses_a_string_to_object_identity_boundary() {
    let db = database().await;
    indexes::ensure(&db).await.unwrap();
    let logs = db.collection::<Document>("agent_log");
    logs.insert_many(
        (0..500).map(|n| doc! {"_id":format!("string-{n:04}"),"agent_id":"owned-agent"}),
    )
    .await
    .unwrap();
    let object = mongodb::bson::oid::ObjectId::new();
    put(
        &db,
        "agent_log",
        doc! {"_id":object,"agent_id":"owned-agent"},
    )
    .await;
    put(
        &db,
        "agent_log",
        doc! {"_id":mongodb::bson::oid::ObjectId::new(),"agent_id":"foreign-agent"},
    )
    .await;
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory {
        agents: vec!["owned-agent".into()],
        ..Default::default()
    };
    let contexts = context::load_cohort(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
    )
    .await
    .unwrap();
    let entry = catalog::ERASURE
        .iter()
        .find(|entry| entry.collection == "agent_log")
        .unwrap();
    let first = indexed::page(&db, entry, &contexts, None, None)
        .await
        .unwrap();
    assert_eq!(first.rows.len(), 500);
    assert!(!first.complete);
    let second = indexed::page(
        &db,
        entry,
        &contexts,
        first.selector.as_ref(),
        first.after.as_ref(),
    )
    .await
    .unwrap();
    assert_eq!(second.rows.len(), 1);
    assert_eq!(second.rows[0].get_object_id("_id").unwrap(), object);
    assert!(second.complete);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn effect_indexes_preserve_absent_and_exact_segmented_log_ddl() {
    use crate::store::agent_log_segments::{schema, COLLECTIONS};
    for prepared in [false, true] {
        let db = database().await;
        if prepared {
            schema::migrate(&db).await.unwrap();
        }
        let mut before = Vec::new();
        for name in COLLECTIONS {
            before.push(if prepared {
                db.collection::<Document>(name)
                    .list_index_names()
                    .await
                    .unwrap()
            } else {
                vec![]
            });
        }
        indexes::ensure(&db).await.unwrap();
        schema::verify(&db).await.unwrap();
        let names = db.list_collection_names().await.unwrap();
        for (name, expected) in COLLECTIONS.iter().zip(before) {
            if prepared {
                assert_eq!(
                    db.collection::<Document>(name)
                        .list_index_names()
                        .await
                        .unwrap(),
                    expected,
                    "effect preparation must preserve the complete frozen index inventory"
                );
            } else {
                assert!(!names.iter().any(|actual| actual == name));
            }
        }
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn segmented_effect_keysets_reuse_frozen_indexes_with_bounded_explains() {
    use crate::store::agent_log_segments::schema;
    let observed = Arc::new(Mutex::new(Vec::<Document>::new()));
    let captured = observed.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "find" {
                captured.lock().unwrap().push(event.command);
            }
        }
    }));
    let db = Client::with_options(options)
        .unwrap()
        .database(&format!("cma_log_keyset_{}", uuid::Uuid::new_v4().simple()));
    schema::migrate(&db).await.unwrap();
    indexes::ensure(&db).await.unwrap();
    schema::verify(&db).await.unwrap();
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory {
        agents: vec!["owned-agent".into()],
        ..Default::default()
    };
    let mut contexts = context::load_cohort(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
    )
    .await
    .unwrap();
    contexts[0].protected.insert("organization-root".into());
    for name in [
        "agent_log_segments",
        "agent_log_publications",
        "agent_log_evidence",
        "agent_log_audits",
        "agent_log_admissions",
        "agent_capacity_recovery_work",
    ] {
        // Synthetic carrier bodies exercise only persisted identity/index keys.
        // The installed validators and frozen index inventory remain unchanged.
        db.collection::<Document>(name).insert_many((0..3104).map(|n| doc! {
            "_id":format!("row-{n:05}"),
            "agent_id":if n < 1104 {"owned-agent"} else {"foreign-agent"},
            "sandbox_id":if n < 1101 {"organization-root"} else {"personal-root"},
            "incarnation":format!("inc-{}",n/700),"ordinal":n%700,"start_seq":n,
            "head_revision":n,"publication_key":format!("key-{n:05}"),
            "evidence_generation":n/700,"kind":"command","entity_key":format!("entity-{n:05}"),
            "audit_id":format!("audit-{n:05}"),"format_version":2_i32
        })).bypass_document_validation(true).await.unwrap();
        let entry = catalog::ERASURE
            .iter()
            .find(|entry| entry.collection == name)
            .unwrap();
        observed.lock().unwrap().clear();
        let (mut selector, mut after) = (None, None);
        let mut seen = BTreeSet::new();
        let (mut protected, mut deletable) = (0, 0);
        for _ in 0..4 {
            let page = indexed::page(&db, entry, &contexts, selector.as_ref(), after.as_ref())
                .await
                .unwrap();
            for row in &page.rows {
                assert!(seen.insert(row.get_str("_id").unwrap().to_owned()));
                assert_eq!(row.get_str("agent_id").unwrap(), "owned-agent");
                if contexts[0].protects(row) {
                    protected += 1;
                } else {
                    deletable += 1;
                }
            }
            selector =
                serde_json::from_slice(&serde_json::to_vec(&page.selector).unwrap()).unwrap();
            after = serde_json::from_slice(&serde_json::to_vec(&page.after).unwrap()).unwrap();
            if page.complete {
                break;
            }
        }
        assert_eq!(
            (seen.len(), protected, deletable),
            (1104, 1101, 3),
            "{name}"
        );
        let queries = std::mem::take(&mut *observed.lock().unwrap());
        assert_eq!(queries.len(), 3, "{name}");
        let (mut keys, mut docs) = (0, 0);
        for command in queries {
            let query: Document = ["find", "filter", "sort", "hint", "limit"]
                .into_iter()
                .filter_map(|key| {
                    command
                        .get(key)
                        .map(|value| (key.to_owned(), value.clone()))
                })
                .collect();
            let plan = db
                .run_command(doc! {"explain":query,"verbosity":"executionStats"})
                .await
                .unwrap();
            let winning = plan
                .get_document("queryPlanner")
                .unwrap()
                .get("winningPlan")
                .unwrap();
            assert!(ixscan(winning), "{name}: {plan:?}");
            assert!(
                !format!("{winning:?}").contains("\"SORT\""),
                "{name}: {plan:?}"
            );
            let stats = plan.get_document("executionStats").unwrap();
            assert!(
                number(stats, "totalKeysExamined") <= 510,
                "{name}: {plan:?}"
            );
            assert!(
                number(stats, "totalDocsExamined") <= 500,
                "{name}: {plan:?}"
            );
            keys += number(stats, "totalKeysExamined");
            docs += number(stats, "totalDocsExamined");
        }
        assert!(keys <= 1120, "{name}: {keys}");
        assert_eq!(docs, 1104, "{name}");
        schema::verify(&db).await.unwrap();
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn effect_preparation_cannot_poison_a_partly_applied_segmented_migration() {
    use crate::store::agent_log_segments::schema;
    let db = database().await;
    assert!(schema::apply_next(&db).await.unwrap());
    assert!(schema::apply_next(&db).await.unwrap());
    let names = db.list_collection_names().await.unwrap();
    assert!(names.iter().any(|name| name == "agent_log_segments"));
    let indexes_before = db
        .collection::<Document>("agent_log_segments")
        .list_index_names()
        .await
        .unwrap();
    assert_eq!(indexes_before, vec!["_id_".to_owned()]);
    let pending = schema::pending_count(&db).await.unwrap();
    assert!(pending > 0);
    indexes::ensure(&db).await.unwrap();
    assert_eq!(
        schema::pending_count(&db).await.unwrap(),
        pending,
        "erasure must not change the frozen migration's remaining steps"
    );
    assert_eq!(
        db.collection::<Document>("agent_log_segments")
            .list_index_names()
            .await
            .unwrap(),
        indexes_before
    );
    schema::migrate(&db).await.unwrap();
    schema::verify(&db).await.unwrap();
    db.drop().await.unwrap();
}
