use super::{batch_fixture::*, *};
use mongodb::bson::serialize_to_vec;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn batched_children_preserve_three_mode_results_and_empty_branches() {
    let (admin, name) = database("sandbox_batch_parity").await;
    let trace = Arc::new(Trace::default());
    let client = observed_client(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let template = template(&store, &target).await;
    for (label, workspaces, agents) in [("empty", 0, 0), ("workspaces", 4, 0), ("many", 4, 8)] {
        let root = seed(&store, &template, label, workspaces, agents).await;
        let expected = store.read_sandbox_hierarchy(&root.id).await.unwrap();
        for reader_mode in 0..3 {
            clear(&trace);
            let reader = mode(&store, reader_mode);
            assert_eq!(
                reader.read_sandbox_hierarchy(&root.id).await.unwrap(),
                expected
            );
            let commands = trace.commands.lock().unwrap().clone();
            if reader_mode == 2 {
                assert_eq!(commands.len(), 2);
                assert_eq!(trace.checkouts.load(Ordering::SeqCst), 2);
                assert_eq!(commands[0].0, "find");
                assert_eq!(commands[1].0, "aggregate");
                assert_eq!(commands[1].1.get_i32("maxTimeMS").unwrap(), 10_000);
                assert_eq!(
                    commands[1]
                        .1
                        .get_document("readConcern")
                        .unwrap()
                        .get_str("level")
                        .unwrap(),
                    "snapshot"
                );
                assert!(commands[1]
                    .1
                    .get_document("readConcern")
                    .unwrap()
                    .contains_key("atClusterTime"));
                assert_eq!(
                    commands[0].1.get_document("lsid").unwrap(),
                    commands[1].1.get_document("lsid").unwrap()
                );
                assert!(commands
                    .iter()
                    .all(|(_, cmd)| !cmd.contains_key("txnNumber")
                        && !cmd.contains_key("autocommit")));
            }
        }
    }
    let root = &template.sandbox;
    // Direct store composition cannot accidentally run unionWith in a transaction.
    clear(&trace);
    store
        .clone()
        .with_sandbox_batched_reads(true)
        .read_sandbox_hierarchy(&root.id)
        .await
        .unwrap();
    assert!(trace
        .commands
        .lock()
        .unwrap()
        .iter()
        .any(|(n, _)| n == "abortTransaction"));
    assert!(mode(&store, 2)
        .read_sandbox_hierarchy(&SandboxId("sbx_absent".into()))
        .await
        .unwrap()
        .is_none());
    store
        .agents
        .update_one(
            doc! {"sandbox_id":root.id.as_str()},
            doc! {"$set":{"record.sandbox_id":"sbx_wrong"}},
        )
        .await
        .unwrap();
    for reader_mode in 0..3 {
        assert!(matches!(
            mode(&store, reader_mode)
                .read_sandbox_hierarchy(&root.id)
                .await,
            Err(SandboxStoreError::Corrupt(_))
        ));
    }
    admin.database(&name).drop().await.unwrap();
    client.shutdown().await;
    admin.shutdown().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn batched_children_keep_individual_bson_limits_and_same_snapshot_fallback() {
    let (admin, name) = database("sandbox_batch_sizes").await;
    let trace = Arc::new(Trace::default());
    let client = observed_client(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let template = template(&store, &target).await;
    let root = &template.sandbox;
    // The combined hierarchy exceeds 16 MiB, while every original row is legal.
    for collection in [&store.workspaces, &store.worktrees, &store.agents] {
        collection
            .update_many(
                doc! {},
                doc! {"$set":{"retained_padding":"x".repeat(6 * 1024 * 1024)}},
            )
            .await
            .unwrap();
    }
    assert_eq!(
        mode(&store, 2)
            .read_sandbox_hierarchy(&root.id)
            .await
            .unwrap(),
        Some(template.clone())
    );
    // A single row too close to 16 MiB for an envelope selects a compatibility
    // read at the original snapshot; no server error is hidden or reclassified.
    let mut row = store.workspaces.find_one(doc! {}).await.unwrap().unwrap();
    row.insert("retained_padding", "");
    let padding = 16 * 1024 * 1024 - 64 - serialize_to_vec(&row).unwrap().len();
    row.insert("retained_padding", "x".repeat(padding));
    // Fixture setup only: avoid a large old+new replacement transaction in the
    // contract harness's small WiredTiger cache. The read starts after insertion.
    store
        .workspaces
        .delete_one(doc! {"_id":row.get("_id").unwrap()})
        .await
        .unwrap();
    store.workspaces.insert_one(row).await.unwrap();
    clear(&trace);
    assert_eq!(
        mode(&store, 2)
            .read_sandbox_hierarchy(&root.id)
            .await
            .unwrap(),
        Some(template)
    );
    let commands = trace.commands.lock().unwrap().clone();
    assert!(commands.iter().any(|(n, _)| n == "aggregate"));
    let reads: Vec<_> = commands
        .iter()
        .filter(|(n, _)| n == "find" || n == "aggregate")
        .collect();
    assert_eq!(
        reads.len(),
        5,
        "root, attempted batch, and three compatibility reads"
    );
    let time = reads[1]
        .1
        .get_document("readConcern")
        .unwrap()
        .get_timestamp("atClusterTime")
        .unwrap();
    for (_, command) in &reads[1..] {
        assert_eq!(
            command
                .get_document("readConcern")
                .unwrap()
                .get_timestamp("atClusterTime")
                .unwrap(),
            time
        );
        assert_eq!(
            command.get_document("lsid").unwrap(),
            reads[0].1.get_document("lsid").unwrap()
        );
    }
    assert!(!commands
        .iter()
        .any(|(n, _)| n == "abortTransaction" || n == "commitTransaction"));
    admin.database(&name).drop().await.unwrap();
    client.shutdown().await;
    admin.shutdown().await;
}

fn plans(value: &Bson, indexes: &mut Vec<String>) {
    match value {
        Bson::Document(doc) => {
            // Reject a scan only in a winning plan, not the rejected alternatives.
            if let Some(plan) = doc.get("winningPlan") {
                winning(plan, indexes);
            }
            for (key, child) in doc {
                if key != "winningPlan" && key != "rejectedPlans" {
                    plans(child, indexes);
                }
            }
        }
        Bson::Array(items) => {
            for item in items {
                plans(item, indexes);
            }
        }
        _ => (),
    }
}
fn winning(value: &Bson, indexes: &mut Vec<String>) {
    match value {
        Bson::Document(doc) => {
            assert_ne!(doc.get_str("stage").ok(), Some("COLLSCAN"));
            if let Ok(name) = doc.get_str("indexName") {
                indexes.push(name.into());
            }
            for child in doc.values() {
                winning(child, indexes);
            }
        }
        Bson::Array(items) => {
            for item in items {
                winning(item, indexes);
            }
        }
        _ => (),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn batched_children_use_all_three_sandbox_indexes_and_cross_cursor_batches() {
    let (admin, name) = database("sandbox_batch_indexes").await;
    let trace = Arc::new(Trace::default());
    let client = observed_client(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let template = template(&store, &target).await;
    let root = seed(&store, &template, "target", 8, 16).await;
    seed(&store, &template, "unrelated", 32, 32).await;
    let expected = store.read_sandbox_hierarchy(&root.id).await.unwrap();
    clear(&trace);
    assert_eq!(
        mode(&store, 2)
            .read_sandbox_hierarchy(&root.id)
            .await
            .unwrap(),
        expected
    );
    let commands = trace.commands.lock().unwrap().clone();
    assert!(commands.iter().any(|(n, _)| n == "getMore"));
    assert!(commands
        .iter()
        .all(|(n, _)| ["find", "aggregate", "getMore"].contains(&n.as_str())));
    for (_, command) in &commands {
        assert_eq!(
            command.get_document("lsid").unwrap(),
            commands[0].1.get_document("lsid").unwrap()
        );
    }
    let aggregate = &commands.iter().find(|(n, _)| n == "aggregate").unwrap().1;
    let explain = admin.database(&name).run_command(doc! {
        "explain":{"aggregate":"workspaces","pipeline":aggregate.get_array("pipeline").unwrap(),"cursor":{}},
        "verbosity":"executionStats"
    }).await.unwrap();
    let mut indexes = Vec::new();
    plans(&Bson::Document(explain), &mut indexes);
    for expected in ["workspaces_sandbox", "worktrees_sandbox", "agents_sandbox"] {
        assert!(
            indexes.iter().any(|name| name == expected),
            "{expected}: {indexes:?}"
        );
    }
    println!("BATCH_INDEXES branches=3 sandbox_indexes=3 collection_scans=0");
    admin.database(&name).drop().await.unwrap();
    client.shutdown().await;
    admin.shutdown().await;
}
