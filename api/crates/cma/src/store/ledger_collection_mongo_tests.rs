use super::*;
use crate::store::ledger_collection::CollectionReadRefusal;
use layer3_adapter::runtime_ledger_collection::{
    CollectionGroup, CollectionRequest, LedgerGroupDecisionV1, COLLECTION_FORMAT,
};

#[path = "ledger_collection_cancellation_tests.rs"]
mod cancellation;

fn collection_documents() -> Vec<(&'static str, Document)> {
    let mut documents = documents();
    let mut parent = crate::store::sandboxes::read_sandbox(&documents[0].1).unwrap();
    parent.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32));
    documents[0].1 = crate::store::sandboxes::sandbox_document(&parent, 0).unwrap();
    documents
}

fn request(documents: &[(&str, Document)]) -> CollectionRequest {
    let parent = crate::store::sandboxes::read_sandbox(&documents[0].1).unwrap();
    let mut groups = vec![
        CollectionGroup::Agent {
            agent_id: "agt_atomic".into(),
        },
        CollectionGroup::Worktree {
            workspace: "project".into(),
            name: "wt-1".into(),
        },
    ];
    groups.sort_by_key(CollectionGroup::key);
    CollectionRequest {
        format: COLLECTION_FORMAT.into(), operation_id: uuid::Uuid::new_v4().to_string(),
        binding: serde_json::from_value(serde_json::json!({
            "format":"runtime-ledger-binding-v1", "sandbox_id":parent.id.as_str(),
            "home_identity":parent.lifecycle.home.as_ref().unwrap().attachment().unwrap().identity().unwrap(),
            "generation":1, "boot_id":"b".repeat(32), "roots_sha256":"a".repeat(64),
            "source_format":"v2", "archive_id":uuid::Uuid::from_u128(1).to_string(), "maintenance_id":"fixture-maintenance",
            "maintenance_epoch":1, "sandbox_revision":parent.revision,
            "candidate_sha256":"b".repeat(64), "recovery_sha256":"c".repeat(64),
            "exclusion_receipt_sha256":"d".repeat(64),
        })).unwrap(),
        active_revision: 1, page_sha256: "e".repeat(64), groups,
    }
}

async fn indexes(db: &mongodb::Database) {
    for (collection, keys) in [
        ("agents", doc! {"worktree_id":1,"_id":1}),
        ("agents", doc! {"record.worktree_id":1,"_id":1}),
        ("workspaces", doc! {"sandbox_id":1,"name":1,"_id":1}),
        (
            "workspaces",
            doc! {"record.sandbox_id":1,"record.name":1,"_id":1},
        ),
        ("worktrees", doc! {"workspace_id":1,"name":1}),
        (
            "worktrees",
            doc! {"record.workspace_id":1,"record.name":1,"_id":1},
        ),
    ] {
        db.collection::<Document>(collection)
            .create_index(mongodb::IndexModel::builder().keys(keys).build())
            .await
            .unwrap();
    }
}

async fn traced_client() -> (Client, Arc<Mutex<Vec<(String, Document)>>>) {
    traced_client_named(None).await
}

async fn traced_client_named(
    app_name: Option<String>,
) -> (Client, Arc<Mutex<Vec<(String, Document)>>>) {
    let trace = Arc::new(Mutex::new(Vec::new()));
    let captured = trace.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.app_name = app_name;
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            captured
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    (Client::with_options(options).unwrap(), trace)
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn bounded_collection_observes_complete_groups_with_no_writes_or_history_scan() {
    let (admin, name) = database("bounded_collection").await;
    let db = admin.database(&name);
    let documents = collection_documents();
    install(&db, &documents).await;
    indexes(&db).await;
    let (client, trace) = traced_client().await;
    let request = request(&documents);
    let result =
        crate::store::ledger_collection::observe(&client, &client.database(&name), &request)
            .await
            .unwrap();
    assert!(result.validate_for(&request));
    assert_eq!(result.groups.len(), 2);
    assert!(result
        .groups
        .iter()
        .all(|g| g.decision == LedgerGroupDecisionV1::SettledDeleted));
    let commands = trace.lock().unwrap().clone();
    assert!(commands.iter().all(|(name, _)| matches!(
        name.as_str(),
        "find" | "listIndexes" | "getMore" | "abortTransaction" | "killCursors"
    )));
    let first_find = &commands.iter().find(|(name, _)| name == "find").unwrap().1;
    assert!(first_find
        .get_document("readConcern")
        .is_ok_and(|c| c.get_str("level").ok() == Some("snapshot")));
    assert_eq!(commands.last().unwrap().0, "abortTransaction");
    let inventories: Vec<_> = commands
        .iter()
        .filter(|(name, _)| name == "listIndexes")
        .collect();
    assert_eq!(inventories.len(), 4, "qualify each collection only once");
    for (_, command) in inventories {
        assert_eq!(
            command
                .get_document("cursor")
                .unwrap()
                .get_i32("batchSize")
                .unwrap(),
            64
        );
        assert_eq!(command.get_i32("maxTimeMS").unwrap(), 2_000);
    }
    for (_, command) in commands.iter().filter(|(name, _)| name == "find") {
        assert_ne!(command.get_str("find").unwrap(), "agent_log");
        assert!(command.get_str("hint").is_ok());
        assert!(command.get_i64("limit").unwrap() <= 257);
        assert_eq!(
            command.get_document("lsid").unwrap(),
            first_find.get_document("lsid").unwrap()
        );
    }
    for (collection, document) in &documents {
        assert_eq!(
            db.collection::<Document>(collection)
                .find_one(doc! {"_id":document.get_str("_id").unwrap()})
                .await
                .unwrap()
                .unwrap(),
            *document
        );
    }
    let unrelated: Vec<_> = (0..2_000)
        .map(|i| {
            doc! {"_id":format!("unrelated-{i}"),
            "sandbox_id":"sbx_retirement_fixture","worktree_id":"unrelated",
            "record":{"worktree_id":"unrelated"}}
        })
        .collect();
    db.collection::<Document>("agents")
        .insert_many(unrelated)
        .await
        .unwrap();
    trace.lock().unwrap().clear();
    let again =
        crate::store::ledger_collection::observe(&client, &client.database(&name), &request)
            .await
            .unwrap();
    assert_eq!(again.dependencies_read, result.dependencies_read);
    assert_eq!(again.source_bytes, result.source_bytes);
    assert_eq!(trace.lock().unwrap().len(), commands.len());
    let worktree_id = documents[2].1.get_str("_id").unwrap();
    let explained = db
        .run_command(doc! {"explain":{"find":"agents",
        "filter":{"worktree_id":worktree_id},"hint":{"worktree_id":1,"_id":1},"limit":257},
        "verbosity":"executionStats"})
        .await
        .unwrap();
    let stats = explained.get_document("executionStats").unwrap();
    assert_eq!(stats.get_i32("nReturned").unwrap(), 1);
    assert_eq!(stats.get_i32("totalDocsExamined").unwrap(), 1);
    assert!(stats.get_i32("totalKeysExamined").unwrap() <= 2);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn bounded_collection_refuses_missing_drift_foreign_quarantine_and_wrong_binding() {
    let (client, name) = database("collection_refusal").await;
    for case in 0..8 {
        let db = client.database(&format!("{name}_{case}"));
        let mut documents = collection_documents();
        let mut request = request(&documents);
        match case {
            0 => {
                documents.remove(1);
            }
            1 => {
                documents[3].1.insert("worktree_id", "foreign-projection");
            }
            2 => {
                documents[3].1.insert("owner", "foreign");
                documents[3]
                    .1
                    .get_document_mut("record")
                    .unwrap()
                    .insert("owner", "foreign");
            }
            3 => {
                documents[3].1.insert(
                    crate::store::agent_log_integrity::QUARANTINE_FIELD,
                    Bson::Null,
                );
            }
            4 => request.binding.generation += 1,
            5 => request.binding.sandbox_revision += 1,
            6 => request.binding.boot_id = "c".repeat(32),
            _ => {
                documents[3]
                    .1
                    .insert("retained_padding", "x".repeat(9 * 1024 * 1024));
            }
        }
        install(&db, &documents).await;
        indexes(&db).await;
        let refusal = crate::store::ledger_collection::observe(&client, &db, &request)
            .await
            .unwrap_err();
        let expected = match case {
            4..=6 => CollectionReadRefusal::StaleAuthority,
            7 => CollectionReadRefusal::BudgetExceeded,
            _ => CollectionReadRefusal::UnknownEligibility,
        };
        assert_eq!(refusal, expected, "case {case}");
        for (collection, document) in &documents {
            assert_eq!(
                db.collection::<Document>(collection)
                    .find_one(doc! {"_id":document.get_str("_id").unwrap()})
                    .await
                    .unwrap()
                    .unwrap(),
                *document
            );
        }
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn bounded_collection_live_shared_child_and_over_page_dependencies_are_preserved() {
    let (client, name) = database("collection_shared").await;
    let db = client.database(&name);
    let documents = collection_documents();
    let request = request(&documents);
    install(&db, &documents).await;
    indexes(&db).await;
    let (_, _, _, mut other) = hierarchy();
    other.id = AgentId("shared-live".into());
    other.status = AgentStatus::Ready;
    other.conversation.conversation_id = Some("fixture-conversation".into());
    other.conversation.runtime_generation = Some(1);
    other.deleted_at = None;
    let document = crate::store::agents::agent_document(&other).unwrap();
    db.collection::<Document>("agents")
        .insert_one(document)
        .await
        .unwrap();
    let result = crate::store::ledger_collection::observe(&client, &db, &request)
        .await
        .unwrap();
    assert!(result
        .groups
        .iter()
        .all(|g| g.decision == LedgerGroupDecisionV1::RetainLive));
    let children: Vec<_> = (0..260)
        .map(|i| {
            let (_, _, _, mut child) = hierarchy();
            child.id = AgentId(format!("shared-deleted-{i}"));
            crate::store::agents::agent_document(&child).unwrap()
        })
        .collect();
    db.collection::<Document>("agents")
        .insert_many(children)
        .await
        .unwrap();
    assert!(
        crate::store::ledger_collection::observe(&client, &db, &request)
            .await
            .is_err()
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn bounded_collection_missing_index_refuses_without_creating_it() {
    let (client, name) = database("collection_no_index").await;
    let db = client.database(&name);
    let documents = collection_documents();
    install(&db, &documents).await;
    assert!(
        crate::store::ledger_collection::observe(&client, &db, &request(&documents))
            .await
            .is_err()
    );
    unchanged(&db, &documents).await;
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn bounded_collection_invalid_request_has_zero_database_commands() {
    let (client, trace) = traced_client().await;
    let db = client.database("unused_collection_refusal");
    for case in 0..8 {
        let mut request = request(&collection_documents());
        match case {
            0 => request.binding.maintenance_epoch = 0,
            1 => request.groups.push(request.groups[0].clone()),
            2 => request.page_sha256.clear(),
            3 => request.active_revision = 0,
            4 => request.binding.boot_id = "fixture-boot".into(),
            5 => request.binding.archive_id = "fixture-archive".into(),
            6 => request.binding.sandbox_id.push(' '),
            _ => request.binding.archive_id = uuid::Uuid::nil().to_string(),
        }
        assert_eq!(
            crate::store::ledger_collection::observe(&client, &db, &request)
                .await
                .unwrap_err(),
            CollectionReadRefusal::InvalidEvidence
        );
    }
    assert!(trace.lock().unwrap().is_empty());
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn bounded_collection_partial_indexes_cannot_hide_a_live_shared_child() {
    let (client, name) = database("collection_partial_index").await;
    let db = client.database(&name);
    let documents = collection_documents();
    let request = request(&documents);
    install(&db, &documents).await;
    indexes(&db).await;
    let (_, _, _, mut live) = hierarchy();
    live.id = AgentId("shared-live-index-fixture".into());
    live.status = AgentStatus::Ready;
    live.conversation.conversation_id = Some("fixture-conversation".into());
    live.conversation.runtime_generation = Some(1);
    live.deleted_at = None;
    db.collection::<Document>("agents")
        .insert_one(crate::store::agents::agent_document(&live).unwrap())
        .await
        .unwrap();
    db.collection::<Document>("agents")
        .drop_indexes()
        .await
        .unwrap();
    for keys in [
        doc! {"worktree_id":1,"_id":1},
        doc! {"record.worktree_id":1,"_id":1},
    ] {
        let options = mongodb::options::IndexOptions::builder()
            .partial_filter_expression(doc! {"_id":"agt_atomic"})
            .build();
        db.collection::<Document>("agents")
            .create_index(
                mongodb::IndexModel::builder()
                    .keys(keys)
                    .options(options)
                    .build(),
            )
            .await
            .unwrap();
    }
    let observation = crate::store::ledger_collection::observe(&client, &db, &request).await;
    assert_eq!(
        db.collection::<Document>("agents")
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    db.drop().await.unwrap();
    assert!(
        observation.is_err(),
        "partial index must not certify complete authority"
    );
}
