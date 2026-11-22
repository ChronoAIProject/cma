use super::*;
use layer2_core::sandbox::store::AgentAdmissionStore;

mod races;
mod retry;
mod support;

async fn limit(store: &MongoSandboxStore, request: &RecoveryRequest, maximum: u8) {
    let agent = store.get_agent(&request.agent_id).await.unwrap().unwrap();
    let mut parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    parent.limits.max_agents_per_workspace = maximum;
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
    let mut workspace = store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    // The retained-home fixture creates wt-1 directly, outside ordinary admission.
    if workspace.next_worktree_number == 1 {
        workspace.next_worktree_number = 2;
        workspace.revision += 1;
        store
            .workspaces
            .update_one(
                doc! {"_id":workspace.id.as_str()},
                doc! {"$set":
                crate::store::workspaces::workspace_replacement_fields(&workspace).unwrap()},
            )
            .await
            .unwrap();
    }
}

async fn ordinary(store: &MongoSandboxStore, request: &RecoveryRequest, key: &str) -> Agent {
    let recovered = store.get_agent(&request.agent_id).await.unwrap().unwrap();
    let workspace = store
        .get_workspace(&recovered.workspace_id)
        .await
        .unwrap()
        .unwrap();
    created_agent(
        store
            .admit_agent(agent_admission(&workspace, key, key))
            .await
            .unwrap(),
    )
    .agent
}

async fn live(store: &MongoSandboxStore, request: &RecoveryRequest) -> u64 {
    let agent = store.get_agent(&request.agent_id).await.unwrap().unwrap();
    store
        .agents
        .count_documents(doc! {"workspace_id":agent.workspace_id.as_str(),
        "status":{"$in":crate::store::agents::LIVE_STATUSES}})
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn full_workspace_refuses_recovery_completion_without_acknowledging() {
    let (client, name) = database("recovery_quota_full").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (caller, request, original) = admitted(&store).await;
    limit(&store, &request, 1).await;
    let other = ordinary(&store, &request, "last-slot").await;
    assert_eq!(other.status, AgentStatus::Starting);
    let before = snapshot(&store, &request).await;
    let native = Native::default();
    let service = service(&store, false);
    let outcome = service.run_accepted(&native, &request, "recovery").await;
    let after = snapshot(&store, &request).await;
    let count = live(&store, &request).await;
    // Clean synthetic data even on the baseline's expected assertion failure.
    let recorded = service.lookup(&caller, &request).await.unwrap();
    client.database(&name).drop().await.unwrap();
    assert_eq!(
        count, 1,
        "completion must count Starting reservations before publishing live"
    );
    assert_eq!(
        outcome,
        Err(RecoveryError::WorkspaceCapacityFull { limit: 1, live: 1 })
    );
    assert_eq!(native.attaches.load(Ordering::SeqCst), 1);
    assert_eq!(native.acknowledges.load(Ordering::SeqCst), 0);
    assert_eq!(after.agent.status, AgentStatus::Failed);
    assert_eq!(after.agent.failure, before.agent.failure);
    assert_eq!(after.agent.conversation, before.agent.conversation);
    assert_eq!(
        after.publications,
        before.publications + 1,
        "only the claim commits"
    );
    let RecoveryLookup::Recorded(work) = recorded else {
        panic!("accepted work remains discoverable")
    };
    assert_eq!(work.phase, RecoveryPhase::Attaching);
    assert_eq!(work.request, request);
    assert_eq!(work.target, original.target);
    assert!(work.completed_at.is_none());
    assert!(work.lease.is_some());
}
