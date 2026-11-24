//! Review regressions for billing mode, progress and bounded physical queries.
use super::{tests::*, *};
use mongodb::{
    bson::{doc, Bson, Document},
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Arc, Mutex};

async fn mode(db: &Database, enabled: bool) -> Result<BTreeMap<String, u64>, Error> {
    let mut cursor = Cursor {
        test_drain_seconds: Some(0),
        ..Default::default()
    };
    let subject = SubjectId::new("a");
    let mut inventory = ErasureInventory::default();
    let mut counts = BTreeMap::new();
    for _ in 0..2000 {
        let batch = erase_records_with_options(
            db,
            &[Target {
                subject: &subject,
                inventory: &inventory,
            }],
            &mut cursor,
            NOW,
            Options {
                allocation_rollups_enabled: Some(false),
                agent_shared_observation: Some(false),
                retain_legal_consents: false,
                drain_qualification: Some(TransactionDrainQualification::fixture()),
                billing_enabled: enabled,
                transaction_drain_minimum_secs: 120,
            },
        )
        .await?;
        super::progress::acknowledge(&mut inventory, &batch, "a");
        for (name, count) in batch.deleted {
            *counts.entry(name).or_default() += count;
        }
        if !batch.has_more {
            return Ok(counts);
        }
    }
    panic!("billing mode did not converge");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn billing_mode_fences_unclaimed_events_and_attributes_the_subject() {
    let db = database().await;
    put(
        &db,
        "sandbox_usage_events",
        doc! {"_id":"event","owner":"a","billing_claim":null,"billing_delivered":false},
    )
    .await;
    put(&db, "sandbox_usage", doc! {"_id":"usage","owner":"a"}).await;
    let error = mode(&db, true)
        .await
        .expect_err("enabled billing must retain all undelivered usage");
    let Error::Blocked(block) = &error else {
        panic!("unexpected billing error: {error:?}");
    };
    assert_eq!(
        block.subject_hash,
        layer2_core::document_artifacts::digest(b"a")
    );
    assert_eq!(block.reason, ErasureBlock::BillingSettlementPending);
    assert_eq!(count(&db, "sandbox_usage_events").await, 1);
    assert_eq!(count(&db, "sandbox_usage").await, 1);
    let counts = mode(&db, false).await.unwrap();
    assert_eq!(counts.get("sandbox_usage_events"), Some(&1));
    assert_eq!(counts.get("sandbox_usage"), Some(&1));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unsupported_residue_identifies_only_its_cohort_subject() {
    let db = database().await;
    put(
        &db,
        "bridge_callback_inbox",
        doc! {"_id":"old","record":{"owner":"b"}},
    )
    .await;
    let (a, b) = (SubjectId::new("a"), SubjectId::new("b"));
    let inventory = ErasureInventory::default();
    let targets = [
        Target {
            subject: &a,
            inventory: &inventory,
        },
        Target {
            subject: &b,
            inventory: &inventory,
        },
    ];
    let mut cursor = Cursor::default();
    let error = loop {
        match erase_records(&db, &targets, &mut cursor, NOW).await {
            Ok(batch) => assert!(batch.has_more),
            Err(error) => break error,
        }
    };
    let Error::Blocked(block) = &error else {
        panic!("unexpected residue error: {error:?}");
    };
    assert_eq!(
        block.subject_hash,
        layer2_core::document_artifacts::digest(b"b")
    );
    assert!(
        matches!(&block.reason, ErasureBlock::UnsupportedResidue { collection } if collection == "bridge_callback_inbox")
    );
    assert!(!format!("{error:?}").contains(&layer2_core::document_artifacts::digest(b"a")));
    drain(&db, "a", &mut ErasureInventory::default()).await;
    assert_eq!(count(&db, "bridge_callback_inbox").await, 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cursor_catalog_or_operation_mismatch_restarts_safely() {
    let db = database().await;
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory::default();
    let targets = [Target {
        subject: &subject,
        inventory: &inventory,
    }];
    for field in ["catalog_digest", "collection", "indexed"] {
        put(&db, "users", doc! {"_id":"a"}).await;
        let mut value = serde_json::to_value(Cursor::default()).unwrap();
        value["subjects"] = serde_json::json!(["a"]);
        value["operation"] = serde_json::json!(usize::MAX);
        value["billing_checked"] = serde_json::json!(true);
        value[field] = if field == "indexed" {
            serde_json::json!(false)
        } else {
            serde_json::json!("wrong-catalog-position")
        };
        let mut cursor: Cursor = serde_json::from_value(value).unwrap();
        for _ in 0..2000 {
            if !erase_records(&db, &targets, &mut cursor, NOW)
                .await
                .unwrap()
                .has_more
            {
                break;
            }
        }
        assert_eq!(
            count(&db, "users").await,
            0,
            "stale {field} must not certify completion"
        );
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cursor_bindings_independently_guard_a_compatible_shared_scan_resume() {
    let db = database().await;
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory::default();
    let targets = [Target {
        subject: &subject,
        inventory: &inventory,
    }];
    let operations = super::progress::operations();
    for mismatch in [
        None,
        Some("catalog_digest"),
        Some("collection"),
        Some("indexed"),
    ] {
        db.collection::<Document>("activity")
            .insert_many((0..501).map(|n| {
                doc! {
                    "_id":format!("event-{n:04}"),
                    "record":{"actor":"survivor","target":{"kind":"subject","id":"a"}}
                }
            }))
            .await
            .unwrap();
        let mut cursor = Cursor::default();
        let first = erase_records(&db, &targets, &mut cursor, NOW)
            .await
            .unwrap();
        assert!(first.has_more);
        assert!(first.deleted.is_empty());
        cursor.operation = operations
            .iter()
            .position(|(entry, indexed)| entry.collection == "activity" && !indexed)
            .unwrap();
        super::progress::bind(&mut cursor, &operations);
        cursor.billing_checked = true;
        cursor.after = Some(Bson::String("event-0499".into()));
        match mismatch {
            Some("catalog_digest") => cursor.catalog_digest.push_str("-stale"),
            Some("collection") => cursor.collection = Some("users".into()),
            Some("indexed") => cursor.indexed = Some(true),
            None => {}
            _ => unreachable!(),
        }
        // Serialization must preserve a compatible position. Each mismatch
        // changes exactly one binding while all other metadata remains valid.
        cursor = serde_json::from_str(&serde_json::to_string(&cursor).unwrap()).unwrap();
        let mut removed = 0;
        for _ in 0..2000 {
            let batch = erase_records(&db, &targets, &mut cursor, NOW)
                .await
                .unwrap();
            removed += batch.deleted.get("activity").copied().unwrap_or(0);
            if !batch.has_more {
                break;
            }
        }
        assert_eq!(
            removed,
            if mismatch.is_none() { 1 } else { 501 },
            "binding {mismatch:?}"
        );
        assert_eq!(
            count(&db, "activity").await,
            if mismatch.is_none() { 500 } else { 0 }
        );
        db.collection::<Document>("activity")
            .delete_many(doc! {})
            .await
            .unwrap();
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn diagnostic_bindings_are_personal_but_control_authority_survives() {
    let db = database().await;
    for (id, owner) in [
        ("diagnostic:a", "a"),
        ("diagnostic:b", "b"),
        ("observer", "a"),
    ] {
        put(&db,"sandbox_usage_report_control",doc! {"_id":id,"document_schema_version":1_i64,"record":{"owner_subject":owner,"sandbox_id":format!("sbx_{owner}")}}).await;
    }
    drain(&db, "a", &mut ErasureInventory::default()).await;
    assert_eq!(count(&db, "sandbox_usage_report_control").await, 2);
    assert!(db
        .collection::<Document>("sandbox_usage_report_control")
        .find_one(doc! {"_id":"observer"})
        .await
        .unwrap()
        .is_some());
    assert!(db
        .collection::<Document>("sandbox_usage_report_control")
        .find_one(doc! {"_id":"diagnostic:b"})
        .await
        .unwrap()
        .is_some());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lease_expiry_uses_instant_equality_across_timezone_offsets() {
    let db = database().await;
    sandbox(&db, "a", "sbx_a").await;
    db.collection::<Document>("sandboxes").update_one(doc! {"_id":"sbx_a"},doc! {"$set":{"record.work.lease":{"holder":"worker","fence":1_i64,"expires_at":"2026-11-14T00:30:00+08:00"},"work_lease_expires_at":"2026-11-14T00:30:00+08:00"}}).await.unwrap();
    drain(&db, "a", &mut ErasureInventory::default()).await;
    assert_eq!(count(&db, "sandboxes").await, 0);
    sandbox(&db, "a", "sbx_a").await;
    db.collection::<Document>("sandboxes").update_one(doc! {"_id":"sbx_a"},doc! {"$set":{"record.work.lease":{"holder":"worker","fence":1_i64,"expires_at":"2026-11-13T23:00:00-08:00"},"work_lease_expires_at":"2026-11-13T23:00:00-08:00"}}).await.unwrap();
    let mut cursor = Cursor::default();
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory::default();
    let batch = erase_records(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
        &mut cursor,
        NOW,
    )
    .await
    .unwrap();
    assert!(batch.waiting);
    assert_eq!(count(&db, "sandboxes").await, 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn indexed_history_batches_have_bounded_execution_stats() {
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let commands = Arc::new(Mutex::new(Vec::<Document>::new()));
    let captured = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command.get_str("find").ok() == Some("agent_log") {
                captured.lock().unwrap().push(event.command);
            }
        }
    }));
    let db = Client::with_options(options).unwrap().database(&format!(
        "cma_erasure_explain_{}",
        uuid::Uuid::new_v4().simple()
    ));
    sandbox(&db, "a", "sbx_a").await;
    put(
        &db,
        "agents",
        doc! {"_id":"agt_a","sandbox_id":"sbx_a","owner":"a"},
    )
    .await;
    crate::store::agent_log::ensure_indexes(&db.collection("agent_log"))
        .await
        .unwrap();
    db.collection::<Document>("agent_log").insert_many((0..20_000).map(|n|doc! {"_id":format!("log-{n:05}"),"agent_id":if n%20==0 {"agt_a"} else {"agt_b"},"seq":n as i64,"payload":"retained conversation"})).await.unwrap();
    let mut cursor = Cursor::default();
    let subject = SubjectId::new("a");
    let inventory = ErasureInventory {
        sandboxes: vec!["sbx_a".into()],
        agents: vec!["agt_a".into()],
        ..Default::default()
    };
    for _ in 0..2000 {
        let batch = erase_records(
            &db,
            &[Target {
                subject: &subject,
                inventory: &inventory,
            }],
            &mut cursor,
            NOW,
        )
        .await
        .unwrap();
        if batch.deleted.contains_key("agent_log") {
            break;
        }
        assert!(batch.has_more);
    }
    let command = commands.lock().unwrap().last().unwrap().clone();
    let find: Document = command
        .into_iter()
        .filter(|(key, _)| {
            ["find", "filter", "sort", "limit", "hint", "projection"].contains(&key.as_str())
        })
        .collect();
    let explain = db
        .run_command(doc! {"explain":find,"verbosity":"executionStats"})
        .await
        .unwrap();
    let plan = format!(
        "{:?}",
        explain
            .get_document("queryPlanner")
            .unwrap()
            .get("winningPlan")
            .unwrap()
    );
    assert!(plan.contains("IXSCAN"), "{explain:?}");
    assert!(
        !plan.contains("SORT") && !plan.contains("COLLSCAN"),
        "{explain:?}"
    );
    let stats = explain.get_document("executionStats").unwrap();
    for field in ["totalKeysExamined", "totalDocsExamined"] {
        let value = match stats.get(field).unwrap() {
            Bson::Int32(v) => *v as i64,
            Bson::Int64(v) => *v,
            _ => panic!("invalid execution counter"),
        };
        assert!(value <= 600, "{field}={value}: {explain:?}");
    }
    assert_eq!(
        db.collection::<Document>("agent_log")
            .count_documents(doc! {"agent_id":"agt_b"})
            .await
            .unwrap(),
        19_000
    );
    db.drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn overlapping_usage_deletion_is_not_a_billing_block() {
    let (db, independent, notice, release) =
        super::race_tests::intercepted("delete", "sandbox_usage_events").await;
    put(
        &independent,
        "sandbox_usage_events",
        doc! {"_id":"delivered","owner":"a","billing_delivered":true},
    )
    .await;
    let task = tokio::spawn(async move { mode(&db, true).await });
    tokio::time::timeout(std::time::Duration::from_secs(20), notice.notified())
        .await
        .unwrap();
    independent
        .collection::<Document>("sandbox_usage_events")
        .delete_one(doc! {"_id":"delivered"})
        .await
        .unwrap();
    release.send(()).unwrap();
    assert!(
        task.await.unwrap().is_ok(),
        "another worker's successful delete is idempotent progress"
    );
    assert_eq!(count(&independent, "sandbox_usage_events").await, 0);
    independent.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn provenance_deletion_uses_agent_identity_and_preserves_other_owner() {
    use super::catalog::{Erasure, InventorySet};
    let db = database().await;
    for owner in ["a", "b"] {
        put(&db,"agent_creation_provenance",doc! {"_id":format!("agent-{owner}"),"record":{"owner":{"kind":"person","subject":owner},"sandbox_id":format!("sandbox-{owner}")}}).await;
    }
    drain(&db, "a", &mut ErasureInventory::default()).await;
    assert_eq!(count(&db, "agent_creation_provenance").await, 1);
    let entry = catalog::ERASURE
        .iter()
        .find(|entry| entry.collection == "agent_creation_provenance")
        .unwrap();
    assert!(entry.rules.iter().any(|rule| rule.indexed
        && matches!(rule.erasure, Erasure::Linked(InventorySet::Agents, ["_id"]))));
    assert!(db
        .collection::<Document>("agent_creation_provenance")
        .find_one(doc! {"_id":"agent-b"})
        .await
        .unwrap()
        .is_some());
    db.drop().await.unwrap();
}
