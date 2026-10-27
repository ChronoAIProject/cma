//! The production transfer/publication seam; Mongo separately proves owner-only proof selection.
use super::*;
use crate::sandbox::workspace_worker::{
    WorkspaceWorker, WorkspaceWorkerConfig, WorkspaceWorkerDependencies,
};
use crate::store::native_prepared::NativePreparedWorkspaceProof;
use layer2_core::sandbox::store::{
    WorkspacePreparationCompletion, WorkspaceStore, WorkspaceWorkerStore,
};
use layer2_core::sandbox::{
    Workspace, WorkspaceOrigin, WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep,
};

async fn owned(
    harness: &Harness,
) -> (
    Arc<Pools>,
    Sandbox,
    Workspace,
    WorkspacePreparationCompletion,
    SystemTime,
) {
    let pools = pools(harness, &[]);
    let mut prepared = reservation(harness).await;
    let mut proof: Option<NativePreparedWorkspaceProof> = None;
    for _ in 0..20 {
        (prepared, proof) = crate::sandbox::native_prepared::prepare_step_with_proof(
            adapter(harness, pools.clone()).as_ref(),
            "test",
            &prepared,
            &home_tests::home_policy(),
            proof,
        )
        .await
        .unwrap();
        prepared = persist(harness, prepared).await;
        if prepared.state == WarmSandboxState::Ready {
            break;
        }
    }
    let proof = proof.unwrap();
    let mut parent = harness.stored(&prepared.id).await;
    let previous = parent.revision;
    parent.profile = prepared.plan.profile.clone();
    parent.runtime = prepared.runtime.clone();
    parent.lifecycle.home = prepared.home.clone();
    parent.lifecycle.runtime_generation_high_water = 1;
    parent.work.execution = Some(prepared.plan.execution);
    parent.status = SandboxStatus::Installing;
    parent.work.step = SandboxWorkStep::MintInitialCredential;
    parent.revision += 1;
    harness
        .store
        .save_sandbox(previous, parent.clone())
        .await
        .unwrap();
    let now = start_time(&parent) + Duration::from_secs(1);
    let worker = harness.worker_with_adapter("owner", adapter(harness, pools.clone()));
    worker.run_one(&parent.id, now).await.unwrap();
    worker.run_one(&parent.id, now).await.unwrap();
    parent = harness.stored(&parent.id).await;
    assert!(parent.credential.delivered);
    let mut workspace = crate::store::agent_access_fixtures::workspace(&parent, "transfer");
    workspace.id = prepared.workspaces[0].id.clone();
    workspace.profile = prepared.plan.workspaces[0].profile.clone();
    workspace.origin = WorkspaceOrigin::Manual;
    workspace.name = "workspace".into();
    workspace.path = "/home/cma/workspace".into();
    workspace.source = layer2_core::sandbox::WorkspaceSource::Empty;
    workspace.status = WorkspaceStatus::Preparing;
    workspace.resolved_commit = None;
    workspace.ready_at = None;
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, rfc3339_at(now));
    workspace.work.execution = Some(prepared.plan.execution);
    harness
        .store
        .create_workspace(workspace.clone())
        .await
        .unwrap();
    (
        pools,
        parent,
        workspace,
        WorkspacePreparationCompletion {
            prepare: proof.prepare,
            setup: proof.setup.unwrap(),
        },
        now,
    )
}

async fn claim(
    harness: &Harness,
    workspace: &Workspace,
    holder: &str,
    now: SystemTime,
) -> Workspace {
    harness
        .store
        .claim_workspace_work(
            &workspace.id,
            holder,
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap()
}

fn worker(harness: &Harness, pools: Arc<Pools>, holder: &str) -> Arc<WorkspaceWorker> {
    WorkspaceWorker::new(
        WorkspaceWorkerDependencies {
            store: harness.store.clone(),
            adapter: adapter(harness, pools),
        },
        WorkspaceWorkerConfig::new(holder, "test"),
    )
}

#[tokio::test]
async fn workspace_transfer_publishes_once_after_one_inspection_without_replays() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let (pools, parent, workspace, completed, now) = owned(&harness).await;
    let claimed = claim(&harness, &workspace, "transfer", now).await;
    let before = harness.backend.requests().await.len();
    worker(&harness, pools.clone(), "transfer")
        .complete_prepared(&claimed, &parent, completed.clone(), now)
        .await
        .unwrap();
    let ready = harness
        .store
        .get_workspace(&workspace.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready.status, WorkspaceStatus::Ready);
    assert_eq!(
        ready.resolved_commit.as_deref(),
        Some(completed.prepare.resolved_commit.as_str())
    );
    assert_eq!(ready.work.step, WorkspaceWorkStep::None);
    assert!(ready.work.lease.is_none());
    let requests = harness.backend.requests().await;
    assert_eq!(
        requests[before..]
            .iter()
            .filter(|(_, r)| matches!(r, RequestFrame::RuntimeStatus(_)))
            .count(),
        1
    );
    assert!(!requests[before..].iter().any(|(_, r)| matches!(
        r,
        RequestFrame::PrepareWorkspace(_) | RequestFrame::WorkspaceSetup(_)
    )));
    // A process restart sees the persisted terminal work state and does no further work.
    let restarted = worker(&harness, pools, "restarted");
    assert!(!restarted.run_one(&workspace.id, now).await.unwrap());
    let events = harness
        .store
        .sandbox_events(&parent.id, 0, 100)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, SandboxEventKind::WorkspaceReady { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn workspace_transfer_refuses_replaced_runtime_and_stale_workspace_lease() {
    for replace_runtime in [false, true] {
        let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let (pools, parent, workspace, completed, now) = owned(&harness).await;
        let claimed = claim(&harness, &workspace, "stale", now).await;
        if replace_runtime {
            harness
                .backend
                .restart_persistent_process(&RuntimeHandle::new(
                    &parent.runtime.as_ref().unwrap().runtime_id,
                ))
                .await
                .unwrap();
        } else {
            claim(
                &harness,
                &workspace,
                "successor",
                now + Duration::from_secs(60),
            )
            .await;
        }
        let result = worker(&harness, pools, "stale")
            .complete_prepared(&claimed, &parent, completed, now)
            .await;
        if !replace_runtime {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
        }
        assert_ne!(
            harness
                .store
                .get_workspace(&workspace.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            WorkspaceStatus::Ready
        );
    }
}
