use super::{support::*, *};
use crate::store::agent_capacity_recovery::RecoveryStream;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_refusal_is_atomic_discoverable_and_completes_after_release() {
    let (client, name) = database("quota_release_retry").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (caller, request, original) = admitted(&store).await;
    limit(&store, &request, 1).await;
    let other = ordinary(&store, &request, "reserved-slot").await;
    let at = layer2_core::now_rfc3339();
    let first = claim(&store, &caller, &request, &at).await;
    let before = durable(&store).await;
    assert_eq!(
        complete(&store, &caller, &first, &at).await,
        Err(RecoveryError::WorkspaceCapacityFull { limit: 1, live: 1 })
    );
    assert_eq!(durable(&store).await, before);
    let later = after(&at, 61);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let service = service(&reopened, false);
    assert!(service
        .discover(RecoveryStream::ExpiredLease, None, &at)
        .await
        .unwrap()
        .requests
        .is_empty());
    assert_eq!(
        service
            .discover(RecoveryStream::ExpiredLease, None, &later)
            .await
            .unwrap()
            .requests,
        [request.clone()]
    );
    release(&store, other).await;
    let next = claim(&reopened, &caller, &request, &later).await;
    assert_eq!(next.work.request, original.request);
    assert_eq!(next.work.target, original.target);
    assert!(next.work.next_fence > first.work.next_fence);
    let CommitOutcome::Recorded(completed) =
        complete(&reopened, &caller, &next, &later).await.unwrap()
    else {
        panic!("available capacity must allow the original recovery")
    };
    assert_eq!(completed.phase, RecoveryPhase::Completed);
    assert_eq!(completed.request, request);
    assert_eq!(live(&store, &request).await, 1);
    assert_eq!(
        store
            .get_agent(&request.agent_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Ready
    );
    let before = durable(&store).await;
    assert_eq!(
        service.reconcile(&caller, &completed).await.unwrap(),
        CommitOutcome::Recorded(completed.clone())
    );
    let mut wrong = completed;
    wrong.completed_at = Some(after(&later, 1));
    assert_eq!(
        service.reconcile(&caller, &wrong).await.unwrap(),
        CommitOutcome::Unknown
    );
    assert_eq!(
        durable(&store).await,
        before,
        "exact reconciliation never republishes at full capacity"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn remaining_slot_allows_completion_and_blocks_the_next_ordinary_admission() {
    let (client, name) = database("quota_available").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, request, _) = admitted(&store).await;
    limit(&store, &request, 2).await;
    ordinary(&store, &request, "reserved-slot").await;
    let native = Native::default();
    assert!(
        matches!(service(&store, false).run_accepted(&native, &request, "available").await.unwrap(),
        CommitOutcome::Recorded(work) if work.phase == RecoveryPhase::Completed)
    );
    assert_eq!(native.acknowledges.load(Ordering::SeqCst), 1);
    assert_eq!(live(&store, &request).await, 2);
    let agent = store.get_agent(&request.agent_id).await.unwrap().unwrap();
    let workspace = store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        store
            .admit_agent(agent_admission(&workspace, "overflow", "overflow"))
            .await,
        Err(SandboxStoreError::AgentLimitReached { limit: 2, live: 2 })
    ));
    assert_eq!(live(&store, &request).await, 2);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retained_reservation_refuses_completion_even_with_no_live_agents() {
    let (client, name) = database("quota_retained").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (caller, request, _) = admitted(&store).await;
    limit(&store, &request, 1).await;
    let at = layer2_core::now_rfc3339();
    let claim = claim(&store, &caller, &request, &at).await;
    client
        .database(&name)
        .collection::<Document>("bridge_allocations")
        .insert_one(doc! {
            "_id":"retained-reservation", "record":{"placement": {
                "kind":"reuse_parent", "target":{"sandbox_id":claim.agent.sandbox_id.as_str()}
            }}
        })
        .await
        .unwrap();
    let before = durable(&store).await;
    assert_eq!(
        complete(&store, &caller, &claim, &at).await,
        Err(RecoveryError::Unavailable)
    );
    assert_eq!(durable(&store).await, before);
    assert_eq!(live(&store, &request).await, 0);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn available_capacity_cannot_override_parent_removal_or_retirement() {
    for retiring in [false, true] {
        let (client, name) = database("quota_parent_intent").await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (caller, request, _) = admitted(&store).await;
        limit(&store, &request, 1).await;
        let at = layer2_core::now_rfc3339();
        let claim = claim(&store, &caller, &request, &at).await;
        if retiring {
            let mut parent = store
                .get_sandbox(&claim.agent.sandbox_id)
                .await
                .unwrap()
                .unwrap();
            parent.status = SandboxStatus::Retiring;
            parent.revision += 1;
            store
                .sandboxes
                .update_one(
                    doc! {"_id":parent.id.as_str()},
                    doc! {"$set":
                    crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap()},
                )
                .await
                .unwrap();
        } else {
            let mut parent = store
                .get_workspace(&claim.agent.workspace_id)
                .await
                .unwrap()
                .unwrap();
            parent.status = WorkspaceStatus::Removing;
            parent.revision += 1;
            store
                .workspaces
                .update_one(
                    doc! {"_id":parent.id.as_str()},
                    doc! {"$set":
                    crate::store::workspaces::workspace_replacement_fields(&parent).unwrap()},
                )
                .await
                .unwrap();
        }
        let before = durable(&store).await;
        assert_eq!(
            complete(&store, &caller, &claim, &at).await,
            Err(RecoveryError::Conflict)
        );
        assert_eq!(durable(&store).await, before);
        assert_eq!(live(&store, &request).await, 0);
        client.database(&name).drop().await.unwrap();
    }
}
