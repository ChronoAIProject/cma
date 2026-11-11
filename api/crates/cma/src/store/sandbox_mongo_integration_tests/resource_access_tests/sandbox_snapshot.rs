use super::*;
use layer2_core::sandbox::{HierarchyReadService, HierarchyReadStore};
use mongodb::event::cmap::CmapEvent;
use std::sync::atomic::AtomicBool;

#[derive(Default)]
struct Trace {
    commands: Mutex<Vec<(String, Document)>>,
    checkouts: AtomicU64,
    pause_children: AtomicBool,
    entered: tokio::sync::Notify,
    resume: (Mutex<bool>, std::sync::Condvar),
}

async fn observed_client(trace: &Arc<Trace>) -> Client {
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.app_name = Some(format!("sandbox-read-{}", uuid::Uuid::new_v4()));
    let observed = trace.clone();
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        if matches!(event, CmapEvent::ConnectionCheckedOut(_)) {
            observed.checkouts.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let observed = trace.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            let pause = matches!(event.command.get_str("find"), Ok("workspaces"))
                && observed.pause_children.swap(false, Ordering::SeqCst);
            observed
                .commands
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
            if pause {
                observed.entered.notify_one();
                let (lock, wake) = &observed.resume;
                let (_ready, timeout) = wake
                    .wait_timeout_while(
                        lock.lock().unwrap(),
                        std::time::Duration::from_secs(5),
                        |ready| !*ready,
                    )
                    .unwrap();
                assert!(
                    !timeout.timed_out(),
                    "concurrent writer must release snapshot reader"
                );
            }
        }
    }));
    Client::with_options(options).unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_snapshot_preserves_results_and_removes_only_abort_checkout() {
    let (admin, name) = database("sandbox_snapshot_parity").await;
    let trace = Arc::new(Trace::default());
    let client = observed_client(&trace).await;
    let (legacy, target) = fixture(&client, &name).await;
    let root = legacy
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage
        .sandbox;
    let snapshot = legacy.clone().with_sandbox_snapshot_reads(true);
    let expected = legacy
        .read_sandbox_hierarchy(&root.id)
        .await
        .unwrap()
        .unwrap();
    for (store, aborts) in [(&legacy, 1), (&snapshot, 0)] {
        trace.commands.lock().unwrap().clear();
        trace.checkouts.store(0, Ordering::SeqCst);
        assert_eq!(
            store.read_sandbox_hierarchy(&root.id).await.unwrap(),
            Some(expected.clone())
        );
        let commands = trace.commands.lock().unwrap().clone();
        assert_eq!(commands.len(), 4 + aborts);
        assert_eq!(trace.checkouts.load(Ordering::SeqCst), (4 + aborts) as u64);
        assert!(commands[..4].iter().all(|(name, _)| name == "find"));
        if aborts == 0 {
            let first = &commands[0].1;
            let time = commands[1]
                .1
                .get_document("readConcern")
                .unwrap()
                .get_timestamp("atClusterTime")
                .unwrap();
            for (i, (_, command)) in commands.iter().enumerate() {
                assert_eq!(
                    command.get_document("lsid").unwrap(),
                    first.get_document("lsid").unwrap()
                );
                assert!(!command.contains_key("startTransaction"));
                assert!(!command.contains_key("txnNumber"));
                assert!(!command.contains_key("autocommit"));
                let concern = command.get_document("readConcern").unwrap();
                assert_eq!(concern.get_str("level").unwrap(), "snapshot");
                if i > 0 {
                    assert_eq!(concern.get_timestamp("atClusterTime").unwrap(), time);
                }
            }
        } else {
            assert_eq!(commands.last().unwrap().0, "abortTransaction");
        }
    }
    trace.commands.lock().unwrap().clear();
    assert_eq!(
        snapshot
            .read_workspace_hierarchy(&expected.workspaces[0].id)
            .await
            .unwrap(),
        legacy
            .read_workspace_hierarchy(&expected.workspaces[0].id)
            .await
            .unwrap()
    );
    assert_eq!(
        trace
            .commands
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, _)| name == "abortTransaction")
            .count(),
        2
    );
    for store in [&legacy, &snapshot] {
        assert_eq!(
            store
                .read_sandbox_hierarchy(&SandboxId("sbx_absent".into()))
                .await
                .unwrap(),
            None
        );
    }
    legacy
        .agents
        .update_one(doc! {}, doc! {"$set":{"record.sandbox_id":"sbx_wrong"}})
        .await
        .unwrap();
    for store in [&legacy, &snapshot] {
        assert!(matches!(
            store.read_sandbox_hierarchy(&root.id).await,
            Err(SandboxStoreError::Corrupt(_))
        ));
    }
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_snapshot_keeps_children_before_concurrent_move_and_delete() {
    let (admin, name) = database("sandbox_snapshot_concurrent").await;
    let trace = Arc::new(Trace::default());
    let client = observed_client(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let store = store.with_sandbox_snapshot_reads(true);
    let root = store
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage
        .sandbox;
    let before = store.read_sandbox_hierarchy(&root.id).await.unwrap();
    trace.pause_children.store(true, Ordering::SeqCst);
    let reader = store.clone();
    let id = root.id.clone();
    let task = tokio::spawn(async move { reader.read_sandbox_hierarchy(&id).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), trace.entered.notified())
        .await
        .unwrap();
    // A different client commits changes after the root has fixed atClusterTime.
    let db = admin.database(&name);
    let mut session = admin.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    db.collection::<Document>(WORKSPACES)
        .update_many(
            doc! {},
            doc! {"$set":{"sandbox_id":"sbx_moved","record.sandbox_id":"sbx_moved"}},
        )
        .session(&mut session)
        .await
        .unwrap();
    db.collection::<Document>(WORKTREES)
        .delete_many(doc! {})
        .session(&mut session)
        .await
        .unwrap();
    db.collection::<Document>(AGENTS)
        .delete_many(doc! {})
        .session(&mut session)
        .await
        .unwrap();
    session.commit_transaction().await.unwrap();
    *trace.resume.0.lock().unwrap() = true;
    trace.resume.1.notify_one();
    assert_eq!(task.await.unwrap().unwrap(), before);
    let after = store
        .read_sandbox_hierarchy(&root.id)
        .await
        .unwrap()
        .unwrap();
    assert!(after.workspaces.is_empty() && after.worktrees.is_empty() && after.agents.is_empty());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn sandbox_snapshot_fails_closed_on_server_timeout() {
    let (admin, name) = database("sandbox_snapshot_failure").await;
    let trace = Arc::new(Trace::default());
    let client = observed_client(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let root = store
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage
        .sandbox;
    for enabled in [false, true] {
        let reader = store.clone().with_sandbox_snapshot_reads(enabled);
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":{"times":1},
                "data":{"failCommands":["find"], "errorCode":50}
            })
            .await
            .unwrap();
        let result = reader.read_sandbox_hierarchy(&root.id).await;
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":"off"
            })
            .await
            .unwrap();
        assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
        assert!(reader
            .read_sandbox_hierarchy(&root.id)
            .await
            .unwrap()
            .is_some());
    }
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_snapshot_preserves_large_hierarchy_cursor_batches() {
    let (admin, name) = database("sandbox_snapshot_batches").await;
    let trace = Arc::new(Trace::default());
    let client = observed_client(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let root = store
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage
        .sandbox;
    let rows: Vec<_> = (0..130)
        .map(|i| {
            crate::store::workspaces::workspace_document(&ready_workspace(
                &root,
                &format!("wks_batch_{i:03}"),
                &format!("batch-{i}"),
            ))
            .unwrap()
        })
        .collect();
    store.workspaces.insert_many(rows).await.unwrap();
    let expected = store.read_sandbox_hierarchy(&root.id).await.unwrap();
    trace.commands.lock().unwrap().clear();
    let actual = store
        .with_sandbox_snapshot_reads(true)
        .read_sandbox_hierarchy(&root.id)
        .await
        .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.unwrap().workspaces.len(), 131);
    {
        let commands = trace.commands.lock().unwrap();
        assert!(commands.iter().any(|(name, _)| name == "getMore"));
        assert!(commands
            .iter()
            .all(|(name, _)| name == "find" || name == "getMore"));
    }
    admin.database(&name).drop().await.unwrap();
}

#[path = "sandbox_snapshot/measurement.rs"]
mod measurement;
