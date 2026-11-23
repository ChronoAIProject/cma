use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn large_history_is_erased_in_bounded_batches_without_transaction() {
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let commands = Arc::new(Mutex::new(Vec::<Document>::new()));
    let captured = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            captured.lock().unwrap().push(event.command);
        }
    }));
    let db = Client::with_options(options).unwrap().database(&format!(
        "cma_erasure_history_{}",
        uuid::Uuid::new_v4().simple()
    ));
    sandbox(&db, "a", "sbx_a").await;
    put(
        &db,
        "agents",
        doc! {"_id":"agt_a", "sandbox_id":"sbx_a", "owner":"a"},
    )
    .await;
    db.collection::<Document>("agent_log")
        .insert_many((0..10_000).map(|n| doc! {"_id":n,"agent_id":"agt_a"}))
        .await
        .unwrap();
    commands.lock().unwrap().clear();
    let mut inventory = ErasureInventory::default();
    let mut removed = 0;
    let mut cursor = Cursor::default();
    for _ in 0..2000 {
        let subject = SubjectId::new("a");
        let batch = super::erase_records(
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
        super::progress::acknowledge(&mut inventory, &batch, "a");
        assert!(batch.deleted.values().sum::<u64>() <= 500);
        removed += batch.deleted.get("agent_log").copied().unwrap_or(0);
        inventory.pending_key_ids.extend(
            batch
                .pending_key_ids
                .get("a")
                .into_iter()
                .flatten()
                .cloned(),
        );
        if !batch.has_more {
            break;
        }
    }
    assert_eq!(removed, 10_000);
    assert_eq!(count(&db, "agent_log").await, 0);
    for command in commands.lock().unwrap().iter() {
        assert!(!command.contains_key("startTransaction"));
        assert!(!command.contains_key("autocommit"));
        if let Ok(deletes) = command.get_array("deletes") {
            for deletion in deletes {
                let query = deletion.as_document().unwrap().get_document("q").unwrap();
                assert!(
                    query
                        .get_document("_id")
                        .unwrap()
                        .get_array("$in")
                        .unwrap()
                        .len()
                        <= 500
                );
            }
        }
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn rerun_is_idempotent_and_counts_accumulate() {
    let db = database().await;
    sandbox(&db, "a", "sbx_a").await;
    put(
        &db,
        "agents",
        doc! {"_id":"agt_a","sandbox_id":"sbx_a","owner":"a"},
    )
    .await;
    for n in 0..1001 {
        put(&db, "agent_log", doc! {"_id":n,"agent_id":"agt_a"}).await;
    }
    let mut inventory = ErasureInventory::default();
    let first = drain(&db, "a", &mut inventory).await;
    assert_eq!(first.get("agent_log"), Some(&1001));
    let second = drain(&db, "a", &mut inventory).await;
    assert_eq!(second.values().sum::<u64>(), 0);
    assert_eq!(first.get("sandboxes"), Some(&1));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_requires_retirement_and_expired_lease() {
    let db = database().await;
    sandbox(&db, "a", "sbx_a").await;
    db.collection::<Document>("sandboxes").update_one(doc! {"_id":"sbx_a"},doc! {"$set":{"status":"active","record.status":"active","retired_at":null,"purge_after":null},"$unset":{"record.retired_at":"","record.purge_after":""}}).await.unwrap();
    let batch = once(&db, &SubjectId::new("a"), &ErasureInventory::default(), NOW)
        .await
        .unwrap();
    assert!(batch.waiting && batch.has_more);
    assert_eq!(count(&db, "sandboxes").await, 1);
    db.collection::<Document>("sandboxes").update_one(doc! {"_id":"sbx_a"},doc! {"$set":{"status":"retired","record.status":"retired","retired_at":NOW,"purge_after":NOW,"record.retired_at":NOW,"record.purge_after":NOW,"record.work.lease":{"holder":"other","fence":1_i64,"expires_at":"2026-11-14T00:05:00Z"},"work_lease_expires_at":"2026-11-14T00:05:00Z"}}).await.unwrap();
    let batch = once(&db, &SubjectId::new("a"), &ErasureInventory::default(), NOW)
        .await
        .unwrap();
    assert!(batch.waiting && batch.has_more);
    assert_eq!(count(&db, "sandboxes").await, 1);
    db.collection::<Document>("sandboxes").update_one(doc! {"_id":"sbx_a"},doc! {"$set":{"record.work.lease.expires_at":"2026-11-13T23:59:00Z","work_lease_expires_at":"2026-11-13T23:59:00Z"}}).await.unwrap();
    drain(&db, "a", &mut ErasureInventory::default()).await;
    assert_eq!(count(&db, "sandboxes").await, 0);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_owned_resources_are_not_erased() {
    let db = database().await;
    sandbox(&db, "org", "sbx_org").await;
    put(
        &db,
        "resource_policies",
        doc! {"_id":"sbx_org","owner_kind":"organization","owner_subject":"org","revision":1_i64},
    )
    .await;
    put(
        &db,
        "agents",
        doc! {"_id":"agt_org","sandbox_id":"sbx_org","owner":"org"},
    )
    .await;
    put(
        &db,
        "agent_log",
        doc! {"_id":"org-log","agent_id":"agt_org"},
    )
    .await;
    put(&db, "sandbox_usage", doc! {"_id":"sbx_org","owner":"org"}).await;
    let mut inventory = ErasureInventory {
        sandboxes: vec!["sbx_org".into()],
        agents: vec!["agt_org".into()],
        ..Default::default()
    };
    let counts = drain(&db, "a", &mut inventory).await;
    assert_eq!(counts.values().sum::<u64>(), 0);
    for collection in [
        "sandboxes",
        "resource_policies",
        "agents",
        "agent_log",
        "sandbox_usage",
    ] {
        assert_eq!(count(&db, collection).await, 1, "{collection}");
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn erasure_leaves_other_owner_rows_intact() {
    let db = database().await;
    // Every authorized namespace contains another account's row. The B row is
    // independent of catalog rules, so broad selectors or global sweeps fail.
    for collection in crate::store::isolation::COLLECTIONS {
        put(&db,collection,doc! {"_id":"other","owner":"b","owner_subject":"b","subject":"b","principal":"b","agent_id":"agt_b","sandbox_id":"sbx_b","binding_id":"bnd_b","launch_id":"launch_b","profile_id":"profile_b","record":{"owner":"b","actor":"b","sandbox_id":"sbx_b","agent_id":"agt_b","binding_id":"bnd_b","requester":{"sandbox_id":"sbx_b"}}}).await;
    }
    sandbox(&db, "a", "sbx_a").await;
    put(
        &db,
        "agents",
        doc! {"_id":"agt_a","sandbox_id":"sbx_a","owner":"a"},
    )
    .await;
    for collection in [
        "agent_log",
        "agent_artifacts",
        "agent_suggestions",
        "agent_commands",
        "agent_access_tools",
        "agent_message_feedback",
    ] {
        put(&db, collection, doc! {"_id":"owned","agent_id":"agt_a"}).await;
    }
    let counts = drain(&db, "a", &mut ErasureInventory::default()).await;
    assert_eq!(counts.get("agent_log"), Some(&1));
    for collection in crate::store::isolation::COLLECTIONS {
        assert_eq!(
            db.collection::<Document>(collection)
                .count_documents(doc! {"_id":"other"})
                .await
                .unwrap(),
            1,
            "{collection}"
        );
    }
    assert_eq!(count(&db, "agents").await, 1);
    assert_eq!(count(&db, "sandboxes").await, 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unindexed_selectors_share_one_scan_for_concurrent_deletions() {
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let commands = Arc::new(Mutex::new(Vec::<Document>::new()));
    let captured = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command.get_str("find").ok() == Some("activity") {
                captured.lock().unwrap().push(event.command);
            }
        }
    }));
    let db = Client::with_options(options).unwrap().database(&format!(
        "cma_erasure_shared_{}",
        uuid::Uuid::new_v4().simple()
    ));
    // The actor belongs to a survivor. Only the unindexed target matches A/B.
    db.collection::<Document>("activity").insert_many((0..1001).map(|n|doc! {"_id":format!("event-{n:04}"),"record":{"actor":"survivor","target":{"kind":"subject","id":if n%2==0 {"a"} else {"b"}}}})).await.unwrap();
    let a = SubjectId::new("a");
    let b = SubjectId::new("b");
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
    let mut removed = 0;
    for _ in 0..500 {
        let batch = super::erase_records(&db, &targets, &mut cursor, NOW)
            .await
            .unwrap();
        removed += batch.deleted.get("activity").copied().unwrap_or(0);
        // A serialized cursor can be resumed by another invocation/process.
        cursor = serde_json::from_str(&serde_json::to_string(&cursor).unwrap()).unwrap();
        if !batch.has_more {
            break;
        }
    }
    assert_eq!(removed, 1001);
    assert_eq!(count(&db, "activity").await, 0);
    {
        let commands = commands.lock().unwrap();
        let scans: Vec<_> = commands
            .iter()
            .filter(|command| command.get_str("hint").ok() == Some("_id_"))
            .collect();
        assert_eq!(
            scans.len(),
            3,
            "each 500-row page must serve both subjects once: {commands:?}"
        );
        assert!(scans.iter().all(|command| command
            .get_i64("limit")
            .or_else(|_| command.get_i32("limit").map(i64::from))
            .unwrap()
            == 500));
        let indexed: Vec<_> = commands
            .iter()
            .filter(|command| {
                command
                    .get_document("hint")
                    .is_ok_and(|hint| *hint == doc! {"record.actor":1,"_id":1})
            })
            .collect();
        assert_eq!(indexed.len(), 2, "each subject has one exact indexed probe");
        let owners: BTreeSet<_> = indexed
            .iter()
            .map(|command| {
                command
                    .get_document("filter")
                    .unwrap()
                    .get_str("record.actor")
                    .unwrap()
            })
            .collect();
        assert_eq!(owners, BTreeSet::from(["a", "b"]));
    }
    db.drop().await.unwrap();
}
