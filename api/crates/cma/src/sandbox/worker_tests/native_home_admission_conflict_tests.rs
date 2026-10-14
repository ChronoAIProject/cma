use super::*;
use layer2_core::profiles::{
    content_digest, AgentProfileContent, AgentProfileSnapshot, PermissionLevel, ThinkingEffort,
    WorkspaceProfileSnapshot,
};
use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionStore, WorkspaceAdmission, WorkspaceAdmissionKind,
    WorkspaceAdmissionStore,
};
use layer2_core::sandbox::{
    AgentOrigin, AgentWorkStep, Workspace, WorkspaceOrigin, WorkspaceSource, WorkspaceStatus,
    WorkspaceWorkState, WorkspaceWorkStep,
};
use layer2_core::{AgentId, AgentProfileId, RuntimeCapabilities, WorkspaceId, WorktreeId};

async fn admit_early_agent(harness: &Harness, parent: &Sandbox, at: String) {
    let reference = &parent.profile.content.workspace_profiles[0];
    let content = WorkspaceProfileContent {
        name: "project".into(),
        location: WorkspaceLocation::Directory {
            name: "project".into(),
        },
        setup_commands: vec![],
        agent_profiles: vec![],
    };
    let workspace = Workspace {
        id: WorkspaceId::generate(),
        sandbox_id: parent.id.clone(),
        owner: parent.owner.clone(),
        origin: WorkspaceOrigin::SandboxProfile { index: 0 },
        profile: WorkspaceProfileSnapshot {
            id: reference.id.clone(),
            revision: reference.revision,
            digest: content_digest(&content).unwrap(),
            content,
        },
        name: "project".into(),
        path: "/home/cma/project".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: None,
        status: WorkspaceStatus::Preparing,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, at.clone()),
        next_worktree_number: 1,
        created_at: at.clone(),
        ready_at: None,
        failed: None,
        removed_at: None,
        revision: 1,
    };
    harness
        .store
        .admit_workspace(WorkspaceAdmission {
            binding: None,
            workspace: workspace.clone(),
            kind: WorkspaceAdmissionKind::SandboxProfile,
            event_at: at.clone(),
        })
        .await
        .unwrap();
    let content = AgentProfileContent {
        isolated_agent: false,
        model: Some("gpt-test".into()),
        thinking_effort: ThinkingEffort::High,
        permission_level: PermissionLevel::Autonomous,
        skills: vec![],
        system_prompt: String::new(),
    };
    let origin = AgentOrigin::Lifecycle {
        operation_id: "bootstrap-home-race".into(),
    };
    let result = harness
        .store
        .admit_agent(AgentAdmission {
            binding: None,
            created_by: None,
            owner: parent.owner.clone(),
            workspace_id: workspace.id,
            agent_id: AgentId::generate(),
            worktree_id: WorktreeId::generate(),
            profile: AgentProfileSnapshot {
                id: AgentProfileId::generate(),
                revision: 1,
                digest: content_digest(&content).unwrap(),
                content,
            },
            capabilities: RuntimeCapabilities::default(),
            title: None,
            first_message: None,
            creation_key: format!("lifecycle:{}", content_digest(&origin).unwrap()),
            origin,
            event_at: at,
        })
        .await
        .unwrap();
    assert_eq!(
        result.creation().agent.work.step,
        AgentWorkStep::WaitForWorkspace
    );
}

#[tokio::test]
async fn native_home_receipt_survives_concurrent_early_agent_admission_without_reallocating() {
    let harness = Harness::new(vec![], 1, Duration::from_secs(60)).await;
    let mut created = harness.create().await;
    let revision = created.revision;
    created.origin = SandboxOrigin::Lifecycle {
        operation_id: "bootstrap-home-race".into(),
    };
    created.revision += 1;
    harness
        .store
        .save_sandbox(revision, created.clone())
        .await
        .unwrap();
    let pools = pools(&harness, &[]);
    pools.delay_claim_reply.store(true, Ordering::SeqCst);
    let now = start_time(&created);
    let running = worker(&harness, pools.clone());
    let id = created.id.clone();
    let job = tokio::spawn(async move { running.run_one(&id, now).await });
    tokio::time::timeout(Duration::from_secs(2), pools.entered.notified())
        .await
        .unwrap();
    let original_claim = pools.claim.lock().unwrap().clone().unwrap();
    let before = harness.stored(&created.id).await;
    assert!(before
        .lifecycle
        .home
        .as_ref()
        .unwrap()
        .native_pool
        .as_ref()
        .unwrap()
        .claim
        .is_none());
    let admission_at = rfc3339_at(now + Duration::from_secs(1));
    admit_early_agent(&harness, &before, admission_at.clone()).await;
    let admitted = harness.stored(&created.id).await;
    assert!(admitted.revision > before.revision);
    assert_eq!(admitted.last_activity_at, admission_at);
    pools.resume.notify_one();
    assert!(job.await.unwrap().unwrap());
    let provisioned = harness.stored(&created.id).await;
    assert_eq!(provisioned.status, SandboxStatus::Installing);
    assert_eq!(provisioned.work.step, SandboxWorkStep::RunInstall);
    assert!(provisioned.work.lease.is_none());
    assert_eq!(provisioned.work.attempts, 0);
    assert_eq!(provisioned.last_activity_at, admission_at);
    let home = provisioned.lifecycle.home.unwrap();
    let claim = home.native_pool.unwrap().claim.unwrap();
    assert_eq!(claim.uid, original_claim.uid);
    assert_eq!(claim.request, original_claim.request);
    assert_eq!(home.receipt.unwrap().storage_generation, "pvc-uid");
    let calls = pools.calls.lock().unwrap();
    assert_eq!(calls.iter().filter(|call| **call == "claim").count(), 1);
    assert_eq!(calls.iter().filter(|call| **call == "select").count(), 1);
    assert_eq!(calls.iter().filter(|call| **call == "adopt").count(), 1);
}

#[tokio::test]
async fn native_home_receipt_conflict_preserves_successor_lease_and_retirement() {
    for retiring in [false, true] {
        let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let created = harness.create().await;
        let pools = pools(&harness, &[]);
        pools.delay_claim_reply.store(true, Ordering::SeqCst);
        let now = start_time(&created);
        let running = worker(&harness, pools.clone());
        let id = created.id.clone();
        let job = tokio::spawn(async move { running.run_one(&id, now).await });
        tokio::time::timeout(Duration::from_secs(2), pools.entered.notified())
            .await
            .unwrap();
        if retiring {
            harness
                .service
                .delete(
                    &caller("owner", [Capability::DeleteOwnSandbox]),
                    &created.id,
                )
                .await
                .unwrap();
        } else {
            let current = harness.stored(&created.id).await;
            harness
                .store
                .claim_sandbox_work(
                    &created.id,
                    "successor",
                    current.work.lease.unwrap().expires_at,
                    rfc3339_at(now + Duration::from_secs(7200)),
                )
                .await
                .unwrap()
                .unwrap();
        }
        let changed = harness.stored(&created.id).await;
        pools.resume.notify_one();
        assert_eq!(job.await.unwrap(), Err(SandboxStoreError::Conflict));
        assert_eq!(harness.stored(&created.id).await, changed);
        assert!(pools.claim.lock().unwrap().is_some());
        assert!(pools.disk.lock().unwrap().is_none());
        assert!(!pools.calls.lock().unwrap().contains(&"adopt"));
    }
}
