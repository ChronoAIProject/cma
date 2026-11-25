use super::*;

#[tokio::test(start_paused = true)]
async fn bootstrap_stop_preempts_the_workspace_readiness_lease() {
    for reason in [AgentStopReason::Owner, AgentStopReason::SandboxRetired] {
        let fixture = BootstrapHarness::new().await;
        let h = &fixture.harness;
        let agent = fixture.admit(None).await;
        let lease_end = h.now + Duration::from_secs(18 * 60);
        let claimed = h
            .store
            .claim_agent_work(
                &agent.id,
                "workspace-waiter",
                rfc3339_at(h.now),
                rfc3339_at(lease_end),
            )
            .await
            .unwrap()
            .unwrap();
        let stop_at = h.now + Duration::from_secs(1);
        h.store
            .schedule_agent_stop(AgentStopRequest {
                target: AgentControlTarget::from(&claimed),
                reason: reason.clone(),
                requested_at: rfc3339_at(stop_at),
            })
            .await
            .unwrap();
        assert!(h.worker.run_one(&agent.id, stop_at).await.unwrap());
        let closing = h.stored(&agent.id).await;
        assert_eq!(closing.work.step, AgentWorkStep::CloseProcess);
        assert_eq!(closing.work.ready_at, rfc3339_at(stop_at));

        // A waiter that resumes after fencing cannot advance startup, even if
        // the parents become ready within its original lease.
        fixture.ready().await;
        let requests_before = h.backend.requests().await.len();
        h.worker
            .execute_claimed_for_test(claimed, stop_at)
            .await
            .unwrap();
        assert_eq!(h.stored(&agent.id).await, closing);
        assert!(h.worker.run_one(&agent.id, stop_at).await.unwrap());
        assert!(h.worker.run_one(&agent.id, stop_at).await.unwrap());
        let stopped = h.stored(&agent.id).await;
        assert_eq!(stopped.status, AgentStatus::Stopped);
        assert_eq!(stopped.stop_reason, Some(reason));
        assert_eq!(stopped.stopped_at, Some(rfc3339_at(stop_at)));
        assert_eq!(h.backend.requests().await.len(), requests_before);
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn bootstrap_mongo_workspace_claim_defers_without_mutation_until_parent_install_ready() {
    use layer2_core::sandbox::store::WorkspaceWorkerStore;
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable local Mongo required");
    assert!(uri.starts_with("mongodb://127.0.0.1:") || uri.starts_with("mongodb://localhost:"));
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!(
        "cma_bootstrap_workspace_claim_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let store = crate::store::MongoSandboxStore::open(&client, db.name())
        .await
        .unwrap();
    let fixture = BootstrapHarness::new().await;
    let h = &fixture.harness;
    let parent = h
        .store
        .get_sandbox(&h.workspace.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    store
        .admit_sandbox(
            SandboxAdmission::new(
                parent.clone(),
                SandboxAdmissionName::Exact(parent.name.clone()),
                4,
                rfc3339_at(h.now),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store.create_workspace(h.workspace.clone()).await.unwrap();
    let now = rfc3339_at(h.now);
    let expires = rfc3339_at(std::time::SystemTime::now() + Duration::from_secs(30));
    assert!(store
        .claim_workspace_work(
            &h.workspace.id,
            "workspace-worker",
            now.clone(),
            expires.clone()
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store.get_workspace(&h.workspace.id).await.unwrap().unwrap(),
        h.workspace
    );
    let mut installing = parent.clone();
    installing.status = SandboxStatus::Installing;
    installing.runtime = fixture.ready_parent.runtime.clone();
    installing.lifecycle.runtime_generation_high_water =
        fixture.ready_parent.lifecycle.runtime_generation_high_water;
    installing.revision += 1;
    store
        .save_sandbox(parent.revision, installing.clone())
        .await
        .unwrap();
    assert!(store
        .claim_workspace_work(
            &h.workspace.id,
            "workspace-worker",
            now.clone(),
            expires.clone()
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store.get_workspace(&h.workspace.id).await.unwrap().unwrap(),
        h.workspace
    );
    let revision = installing.revision;
    installing.credential = fixture.ready_parent.credential.clone();
    installing.revision += 1;
    store.save_sandbox(revision, installing).await.unwrap();
    let claimed = store
        .claim_workspace_work(&h.workspace.id, "workspace-worker", now, expires)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.work.step, WorkspaceWorkStep::Prepare);
    assert_eq!(claimed.work.attempts, 0);
    assert_eq!(claimed.revision, h.workspace.revision + 1);
    assert!(claimed.failed.is_none());
    db.drop().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn bootstrap_profile_waits_for_same_claimed_workspace_without_duplicate_effects() {
    use crate::sandbox::workspace_worker::{
        WorkspaceWorker, WorkspaceWorkerConfig, WorkspaceWorkerDependencies,
    };
    use layer2_core::sandbox::store::WorkspaceWorkerStore;
    for failed in [false, true] {
        let fixture = BootstrapHarness::new().await;
        let h = &fixture.harness;
        let mut parent = fixture.ready_parent.clone();
        parent.status = SandboxStatus::Installing;
        parent.revision += 1;
        h.store.save_sandbox(1, parent.clone()).await.unwrap();
        let claimed = h
            .store
            .claim_workspace_work(
                &h.workspace.id,
                "independent-scheduler",
                rfc3339_at(h.now),
                rfc3339_at(h.now + Duration::from_secs(30)),
            )
            .await
            .unwrap()
            .unwrap();
        let worker = WorkspaceWorker::new(
            WorkspaceWorkerDependencies {
                store: h.store.clone(),
                adapter: h.adapter.clone(),
            },
            WorkspaceWorkerConfig::new("profile-worker", DEPLOYMENT_ID),
        );
        let before = h.backend.requests().await.len();
        let (result, ()) = tokio::join!(
            worker.run_until_ready_observing_bootstrap(&h.workspace.id, &parent, h.now),
            async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let mut published = fixture.ready_workspace.clone();
                published.revision = claimed.revision + 1;
                if failed {
                    published.status = WorkspaceStatus::Failed;
                    published.ready_at = None;
                    published.failed = Some(TypedFailure {
                        class: "fixture_workspace_failure".into(),
                        detail: "retained terminal workspace failure".into(),
                    });
                }
                h.store
                    .save_workspace(claimed.revision, published)
                    .await
                    .unwrap();
            }
        );
        let result = result.unwrap();
        assert_eq!(result.workspace.id, h.workspace.id);
        assert_eq!(
            result.workspace.status,
            if failed {
                WorkspaceStatus::Failed
            } else {
                WorkspaceStatus::Ready
            }
        );
        assert_eq!(result.workspace.revision, claimed.revision + 1);
        assert!(!result.executed_work);
        assert_eq!(h.backend.requests().await.len(), before);
    }
}

#[tokio::test(start_paused = true)]
async fn bootstrap_profile_observation_is_bounded_and_keeps_the_other_workers_lease() {
    use crate::sandbox::workspace_worker::{
        WorkspaceWorker, WorkspaceWorkerConfig, WorkspaceWorkerDependencies,
    };
    use layer2_core::sandbox::store::WorkspaceWorkerStore;
    let fixture = BootstrapHarness::new().await;
    let h = &fixture.harness;
    let mut parent = fixture.ready_parent.clone();
    parent.status = SandboxStatus::Installing;
    parent.revision += 1;
    h.store.save_sandbox(1, parent.clone()).await.unwrap();
    let claimed = h
        .store
        .claim_workspace_work(
            &h.workspace.id,
            "independent-scheduler",
            rfc3339_at(h.now),
            rfc3339_at(h.now + Duration::from_secs(30)),
        )
        .await
        .unwrap()
        .unwrap();
    let worker = WorkspaceWorker::new(
        WorkspaceWorkerDependencies {
            store: h.store.clone(),
            adapter: h.adapter.clone(),
        },
        WorkspaceWorkerConfig::new("profile-worker", DEPLOYMENT_ID),
    );
    let before = h.backend.requests().await.len();
    let started = tokio::time::Instant::now();
    let result = worker
        .run_until_ready_observing_bootstrap(&h.workspace.id, &parent, h.now)
        .await
        .unwrap();
    assert_eq!(started.elapsed(), Duration::from_secs(10));
    assert_eq!(result.workspace, claimed);
    assert!(!result.executed_work);
    assert_eq!(h.backend.requests().await.len(), before);
}
