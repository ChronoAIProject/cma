use super::*;
use crate::sandbox::store::{
    SandboxLifecycleAgentScope, SandboxLifecycleMutation, SandboxMutationGuard,
};

const HOLDER: &str = "allocation-worker";
const END: &str = "2026-09-05T01:00:00Z";

async fn failed_allocation() -> (Arc<InMemorySandboxStore>, SandboxId) {
    let store = Arc::new(InMemorySandboxStore::default());
    let mut current = native();
    current.recover_runtime_loss(&[], true, NOW.into()).unwrap();
    let source = current.retained_home_recovery().unwrap().clone();
    let fence = current.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    current
        .complete_lifecycle_home_verification(&source, HOLDER, fence, NOW.into())
        .unwrap();
    let fence = current.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    current
        .reserve_lifecycle_runtime_generation(2, HOLDER, fence, NOW.into())
        .unwrap();
    let fence = current.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    store
        .lock()
        .sandboxes
        .insert(current.id.clone(), current.clone());
    let workspace = pending_workspace(&current);
    store
        .lock()
        .workspaces
        .insert(workspace.id.clone(), workspace);
    let mut failed = current.clone();
    let failure = failed
        .fail_lifecycle("runtime startup failed".into(), HOLDER, fence, NOW.into())
        .unwrap();
    assert!(store
        .save_sandbox(current.revision, failed.clone())
        .await
        .is_err());
    store
        .commit_sandbox_lifecycle(SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(&current),
            sandbox: failed.clone(),
            agents: vec![],
            agent_scope: SandboxLifecycleAgentScope::Touched,
            appended_at: NOW.into(),
            events: vec![SandboxEventKind::Failed { failure }],
        })
        .await
        .unwrap();
    assert_eq!(store.get_sandbox(&current.id).await.unwrap(), Some(failed));
    (store, current.id)
}

fn pending_workspace(sandbox: &Sandbox) -> crate::sandbox::Workspace {
    use crate::profiles::{WorkspaceLocation, WorkspaceProfileContent, WorkspaceProfileSnapshot};
    use crate::sandbox::{
        WorkspaceOrigin, WorkspaceSource, WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep,
    };
    crate::sandbox::Workspace {
        id: crate::WorkspaceId("wks_retained_pending".into()),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: crate::WorkspaceProfileId("wkp_retained".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "project".into(),
                },
                name: "project".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: "project".into(),
        path: "/home/cma/project".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: None,
        status: WorkspaceStatus::Preparing,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: None,
        failed: None,
        removed_at: None,
        revision: 1,
    }
}

struct VerifyStopped;
#[async_trait]
impl SandboxUnpublishedRecoveryVerifier for VerifyStopped {
    async fn verify(
        &self,
        snapshot: &SandboxUnpublishedRecoverySnapshot,
    ) -> Result<SandboxUnpublishedRecoveryProof, SandboxStoreError> {
        snapshot.validate()?;
        let binding = snapshot.sandbox.unpublished_recovery_binding().unwrap();
        let plan = serde_json::json!({"fixture": "stopped-original-allocation"});
        Ok(SandboxUnpublishedRecoveryProof {
            sandbox_id: snapshot.sandbox.id.clone(),
            runtime_id: binding.runtime_id.clone(),
            runtime_generation: snapshot.runtime_generation()?,
            reserved_provision: snapshot.recovery_provision_reservation()?.cloned(),
            observed_provision_generation: None,
            home_identity: snapshot.home_identity()?,
            boot_id: None,
            runtime_process_absent: true,
            requires_native_quiescence: true,
            provider_plan_sha256: SandboxUnpublishedRecoveryProof::provider_plan_digest(&plan)
                .unwrap(),
            provider_plan: plan,
            conversations: vec![],
        })
    }
}

#[tokio::test]
async fn failed_provision_transaction_and_owner_update_preserve_original_home_and_authority() {
    for operator in [false, true] {
        let (store, id) = failed_allocation().await;
        let before = store.get_sandbox(&id).await.unwrap().unwrap();
        let workspaces = store.lock().workspaces.clone();
        let service = SandboxRuntimeUpdateService::new(store.clone(), "image-latest".into())
            .with_unpublished_recovery(Arc::new(VerifyStopped));
        assert!(service.can_update(&before));
        let foreign = crate::Caller {
            subject: crate::SubjectId("another-owner".into()),
            authorization: crate::Authorization::new([crate::Capability::UseOwnSandboxes]),
        };
        assert!(service
            .request(&foreign, &id, "image-latest")
            .await
            .is_err());
        assert_eq!(store.get_sandbox(&id).await.unwrap(), Some(before.clone()));
        let caller = crate::Caller {
            subject: if operator {
                crate::SubjectId("operator".into())
            } else {
                before.owner.clone()
            },
            authorization: crate::Authorization::new([if operator {
                crate::Capability::ControlAnySandbox
            } else {
                crate::Capability::UseOwnSandboxes
            }]),
        };
        let after = if operator {
            service
                .request_as_operator(&caller, &id, "image-latest")
                .await
                .unwrap()
        } else {
            service.request(&caller, &id, "image-latest").await.unwrap()
        };
        assert_eq!(after.status, SandboxStatus::Rotating);
        assert_eq!(after.lifecycle.home, before.lifecycle.home);
        assert_eq!(
            after.lifecycle.suspended_runtime,
            before.lifecycle.suspended_runtime
        );
        assert_eq!(after.lifecycle.runtime_generation_high_water, 2);
        assert_eq!(after.retained_home_recovery().unwrap().source_generation, 1);
        assert!(after.lifecycle.failed_provision.is_none());
        assert_eq!(
            after
                .lifecycle
                .unpublished_recovery
                .as_ref()
                .unwrap()
                .proof
                .reserved_provision
                .as_ref(),
            Some(
                &before
                    .lifecycle
                    .failed_provision
                    .as_ref()
                    .unwrap()
                    .reservation
            )
        );
        assert!(after.require_unpublished_runtime_quiescence().is_err());
        assert!(store.lock().agents.is_empty());
        assert_eq!(store.lock().workspaces, workspaces);
        assert_eq!(store.lock().sandbox_events[&id].len(), 2);
    }
}

#[tokio::test]
async fn failed_provision_new_child_after_inspection_fences_admission() {
    let (store, id) = failed_allocation().await;
    let snapshot = store
        .unpublished_runtime_recovery_snapshot(&id)
        .await
        .unwrap();
    let proof = VerifyStopped.verify(&snapshot).await.unwrap();
    let original = snapshot.sandbox.clone();
    let unexpected = agent("agt_new_child", "native-original", 0);
    store
        .lock()
        .agents
        .insert(unexpected.id.clone(), unexpected);
    assert!(store
        .admit_unpublished_runtime_recovery(SandboxUnpublishedRecoveryRequest {
            snapshot,
            proof,
            target_runtime_version: "image-latest".into(),
            released_runtime: None,
            automatic: false,
            requested_at: NOW.into(),
        })
        .await
        .is_err());
    assert_eq!(store.get_sandbox(&id).await.unwrap(), Some(original));
    assert_eq!(store.lock().sandbox_events[&id].len(), 1);
}
