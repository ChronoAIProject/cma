use super::{batch_fixture::*, *};
use layer2_core::sandbox::HierarchyReadError;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn batched_children_preserve_snapshot_during_move_delete_before_aggregate_and_getmore() {
    for after_first_batch in [false, true] {
        let (admin, name) = database("sandbox_batch_race").await;
        let trace = Arc::new(Trace::default());
        let client = observed_client(&trace).await;
        let (store, target) = fixture(&client, &name).await;
        let root = seed(&store, &template(&store, &target).await, "race", 8, 16).await;
        let expected = store.read_sandbox_hierarchy(&root.id).await.unwrap();
        if after_first_batch {
            trace.pause_get_more.store(true, Ordering::SeqCst);
        } else {
            trace.pause_children.store(true, Ordering::SeqCst);
        }
        let reader = mode(&store, 2);
        let id = root.id.clone();
        let task = tokio::spawn(async move { reader.read_sandbox_hierarchy(&id).await });
        tokio::time::timeout(Duration::from_secs(5), trace.entered.notified())
            .await
            .unwrap();
        let db = admin.database(&name);
        let mut session = admin.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        db.collection::<Document>(WORKSPACES)
            .update_many(
                doc! {"sandbox_id":root.id.as_str()},
                doc! {"$set":{"sandbox_id":"sbx_moved","record.sandbox_id":"sbx_moved"}},
            )
            .session(&mut session)
            .await
            .unwrap();
        for collection in [WORKTREES, AGENTS] {
            db.collection::<Document>(collection)
                .delete_many(doc! {"sandbox_id":root.id.as_str()})
                .session(&mut session)
                .await
                .unwrap();
        }
        session.commit_transaction().await.unwrap();
        *trace.resume.0.lock().unwrap() = true;
        trace.resume.1.notify_one();
        assert_eq!(task.await.unwrap().unwrap(), expected);
        let current = mode(&store, 2)
            .read_sandbox_hierarchy(&root.id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            current.workspaces.is_empty()
                && current.worktrees.is_empty()
                && current.agents.is_empty()
        );
        db.drop().await.unwrap();
        drop(session);
        client.shutdown().await;
        admin.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn batched_hierarchy_still_requires_fresh_authority_after_buffered_read() {
    let (admin, name) = database("sandbox_batch_fence").await;
    let trace = Arc::new(Trace::default());
    let client = observed_client(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let root = template(&store, &target).await.sandbox;
    let target = ResourceTarget::Sandbox {
        sandbox_id: root.id.clone(),
    };
    let writer = MongoSandboxStore::open(&admin, &name).await.unwrap();
    let original = writer.resource_access_snapshot(&target).await.unwrap();
    let service = service(mode(&store, 2));
    trace.pause_children.store(true, Ordering::SeqCst);
    let id = root.id.clone();
    let caller = Caller {
        subject: root.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    let task = tokio::spawn(async move { service.sandbox(&caller, &id).await });
    tokio::time::timeout(Duration::from_secs(5), trace.entered.notified())
        .await
        .unwrap();
    writer
        .mutate_share(mutation(&original, "batch-fence-change", ShareMode::View))
        .await
        .unwrap();
    *trace.resume.0.lock().unwrap() = true;
    trace.resume.1.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(HierarchyReadError::Access(ResourceAccessError::Store(
            ResourceAccessStoreError::Conflict
        )))
    ));
    admin.database(&name).drop().await.unwrap();
    client.shutdown().await;
    admin.shutdown().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn batched_hierarchy_refuses_aggregate_cursor_failure_and_cancellation_without_writes() {
    let (admin, name) = database("sandbox_batch_failure").await;
    let trace = Arc::new(Trace::default());
    let client = observed_client(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let root = seed(&store, &template(&store, &target).await, "failure", 8, 16).await;
    let reader = mode(&store, 2);
    let expected = reader.read_sandbox_hierarchy(&root.id).await.unwrap();
    clear(&trace);
    for command in ["aggregate", "getMore"] {
        admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},
            "data":{"failCommands":[command],"errorCode":50}})
            .await
            .unwrap();
        let result = reader.read_sandbox_hierarchy(&root.id).await;
        admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
        assert_eq!(
            reader.read_sandbox_hierarchy(&root.id).await.unwrap(),
            expected
        );
    }
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},
        "data":{"failCommands":["aggregate"],"blockConnection":true,"blockTimeMS":200}})
        .await
        .unwrap();
    let result = tokio::time::timeout(
        Duration::from_millis(50),
        reader.read_sandbox_hierarchy(&root.id),
    )
    .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(result.is_err());
    assert_eq!(
        reader.read_sandbox_hierarchy(&root.id).await.unwrap(),
        expected
    );
    assert!(trace.commands.lock().unwrap().iter().all(|(n, _)| [
        "find",
        "aggregate",
        "getMore",
        "killCursors"
    ]
    .contains(&n.as_str())));
    admin.database(&name).drop().await.unwrap();
    client.shutdown().await;
    admin.shutdown().await;
}
