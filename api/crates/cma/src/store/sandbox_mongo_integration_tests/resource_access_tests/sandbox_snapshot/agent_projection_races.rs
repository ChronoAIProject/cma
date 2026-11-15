use super::{batch_fixture::*, *};
use layer2_core::sandbox::HierarchyReadError;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn projected_detail_pins_children_but_keeps_a_fresh_release_fence() {
    let (admin, name) = database("sandbox_projection_races").await;
    let trace = Arc::new(Trace::default());
    let client = observed_client(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let template = template(&store, &target).await;
    let roots = crate::store::durability::majority_collection::<Document>(
        &admin.database(&name),
        SANDBOXES,
    );
    let agents =
        crate::store::durability::majority_collection::<Document>(&admin.database(&name), AGENTS);
    for hierarchy_mode in 0..3 {
        for lose_owner in [false, true] {
            let root = seed(
                &store,
                &template,
                &format!("race-{hierarchy_mode}-{lose_owner}"),
                1,
                1,
            )
            .await;
            let caller = Caller {
                subject: root.owner.clone(),
                authorization: Authorization::new([Capability::UseOwnSandboxes]),
            };
            let reader = service(mode(&store, hierarchy_mode).with_sandbox_agent_projection(true));
            let expected = reader.sandbox(&caller, &root.id).await.unwrap();
            *trace.resume.0.lock().unwrap() = false;
            trace.pause_children.store(true, Ordering::SeqCst);
            let read_id = root.id.clone();
            let task = tokio::spawn(async move { reader.sandbox(&caller, &read_id).await });
            tokio::time::timeout(std::time::Duration::from_secs(5), trace.entered.notified())
                .await
                .unwrap();
            if lose_owner {
                roots
                    .update_one(
                        doc! {"_id":root.id.as_str()},
                        doc! {"$set": {
                            "owner":"changed-fixture-owner", "record.owner":"changed-fixture-owner"
                        }},
                    )
                    .await
                    .unwrap();
            } else {
                // This committed deletion follows the root read; the projected
                // child cursor must still observe the earlier hierarchy snapshot.
                agents
                    .delete_one(doc! {"sandbox_id":root.id.as_str()})
                    .await
                    .unwrap();
            }
            *trace.resume.0.lock().unwrap() = true;
            trace.resume.1.notify_one();
            let result = task.await.unwrap();
            if lose_owner {
                assert_eq!(
                    result,
                    Err(HierarchyReadError::Access(ResourceAccessError::Store(
                        ResourceAccessStoreError::Conflict
                    )))
                );
            } else {
                assert_eq!(result.unwrap(), expected);
            }
        }
    }
    admin.database(&name).drop().await.unwrap();
    client.shutdown().await;
    admin.shutdown().await;
}
