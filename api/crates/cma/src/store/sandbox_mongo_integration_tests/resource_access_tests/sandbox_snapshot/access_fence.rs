use super::{batch_fixture::*, *};
use layer2_core::sandbox::HierarchyReadError;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn access_sessions_reject_one_thousand_owner_changes_before_release() {
    let (admin, name) = database("access_session_release_fence").await;
    let trace = Arc::new(Trace::default());
    let client = observed_client(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let root = template(&store, &target).await.sandbox;
    let caller = Caller {
        subject: root.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    let roots = crate::store::durability::majority_collection::<Document>(
        &admin.database(&name),
        SANDBOXES,
    );
    for enabled in [false, true] {
        let reader = service(store.clone().with_access_snapshot_session(enabled));
        // Each iteration pauses after the hierarchy root has fixed its snapshot,
        // commits a loss of ownership, then lets the final authorization run.
        for _ in 0..500 {
            clear(&trace);
            *trace.resume.0.lock().unwrap() = false;
            trace.pause_children.store(true, Ordering::SeqCst);
            let service = reader.clone();
            let caller = caller.clone();
            let id = root.id.clone();
            let task = tokio::spawn(async move { service.sandbox(&caller, &id).await });
            tokio::time::timeout(std::time::Duration::from_secs(5), trace.entered.notified())
                .await
                .unwrap();
            roots.update_one(doc! {"_id":root.id.as_str()},
                doc! {"$set":{"owner":"changed-fixture-owner","record.owner":"changed-fixture-owner"}})
                .await.unwrap();
            *trace.resume.0.lock().unwrap() = true;
            trace.resume.1.notify_one();
            assert_eq!(
                task.await.unwrap(),
                Err(HierarchyReadError::Access(ResourceAccessError::Store(
                    ResourceAccessStoreError::Conflict
                )))
            );
            roots
                .update_one(
                    doc! {"_id":root.id.as_str()},
                    doc! {"$set":{"owner":root.owner.as_str(),"record.owner":root.owner.as_str()}},
                )
                .await
                .unwrap();
        }
        assert_eq!(
            reader.sandbox(&caller, &root.id).await.unwrap().sandbox,
            root
        );
    }
    admin.database(&name).drop().await.unwrap();
    client.shutdown().await;
    admin.shutdown().await;
}
